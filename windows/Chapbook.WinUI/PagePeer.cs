using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Automation.Provider;
using Microsoft.UI.Xaml.Automation.Text;
using Microsoft.UI.Xaml.Media;
using Windows.Foundation;

namespace Chapbook.WinUI;

/// <summary>
/// The page behind UI Automation, so Narrator can read it.
/// </summary>
/// <remarks>
/// <para>
/// The third implementation of the engine's text surface against a real
/// assistive stack, after GTK's <c>AccessibleText</c> and the Win32
/// shell's own <c>ITextProvider</c>. The accessors are the same three —
/// <see cref="Session.PageTextRuns"/> for the lines,
/// <see cref="Session.SpeakablePage"/> for the words and the string they
/// sit in, <see cref="Session.RangeRects"/> for the geometry — and each
/// platform asks a differently shaped question of them, which is what
/// makes the accessor a seam rather than one platform's tree.
/// </para>
/// <para>
/// This one is far less code than the Win32 provider next door, and the
/// reason is worth stating: WinUI implements the COM half. There is no
/// vtable, no <c>SAFEARRAY</c>, no reference counting, and — because the
/// framework marshals to the UI thread — no snapshot. The Win32 provider
/// keeps one because UI Automation calls a raw server-side provider from
/// its own threads and <c>Session</c> is not shareable; here the peer runs
/// where the session already lives, so it reads it directly.
/// </para>
/// <para>
/// Offsets on this boundary are character offsets into the speakable
/// string, the same space every other implementation uses.
/// </para>
/// </remarks>
internal sealed class PagePeer(SessionView owner)
    : FrameworkElementAutomationPeer(owner), ITextProvider
{
    private SessionView View => owner;

    private Session? Reading => owner.Reading;

    protected override AutomationControlType GetAutomationControlTypeCore() =>
        // A book's page is a document, which is what makes Narrator offer
        // its reading commands rather than treat this as a nameless canvas.
        AutomationControlType.Document;

    protected override string GetClassNameCore() => nameof(SessionView);

    protected override string GetNameCore()
    {
        string? title = Reading?.Title;
        return string.IsNullOrEmpty(title) ? base.GetNameCore() : title;
    }

    protected override object GetPatternCore(PatternInterface pattern) =>
        pattern == PatternInterface.Text ? this : base.GetPatternCore(pattern);

    // ---- ITextProvider ----

    public ITextRangeProvider DocumentRange => new PageRange(this, 0, Length);

    public Microsoft.UI.Xaml.Automation.SupportedTextSelection SupportedTextSelection =>
        // The session has one selection and no caret of its own; a UIA
        // client asking to select gets it, and nothing here pretends to
        // support several.
        Microsoft.UI.Xaml.Automation.SupportedTextSelection.Single;

    public ITextRangeProvider[] GetSelection() =>
        // No selection API crosses the C ABI, so there is nothing to
        // report. An empty array is "none right now"; a null one would say
        // "this control does not do selections", which is a different and
        // wrong answer.
        [];

    public ITextRangeProvider[] GetVisibleRanges() =>
        // A page is exactly what is visible — the reason this engine
        // paginates rather than scrolls, showing up here as a
        // simplification.
        [DocumentRange];

    public ITextRangeProvider? RangeFromChild(IRawElementProviderSimple childElement) =>
        // No child elements: no embedded controls, no images exposed as
        // objects.
        null;

    public ITextRangeProvider? RangeFromPoint(Point screenLocation)
    {
        if (Reading is not { } session)
        {
            return null;
        }
        // Screen to the view's own logical space, which is the space the
        // metrics were given.
        GeneralTransform transform = View.TransformToVisual(null);
        Point origin = transform.TransformPoint(new Point(0, 0));
        double scale = View.XamlRoot?.RasterizationScale ?? 1.0;
        double x = (screenLocation.X / scale) - origin.X;
        double y = (screenLocation.Y / scale) - origin.Y;

        // Word precision: the word under the point answers with its own
        // range, which is what word-oriented review reads. A degenerate
        // range at the nearest character would be more literal and less
        // useful.
        if (session.WordAt((float)x, (float)y) is not { } locator)
        {
            return new PageRange(this, 0, 0);
        }
        (uint start, uint end) = TextRangeOf(locator.Start, locator.End);
        return new PageRange(this, start, end);
    }

    // ---- What the range provider needs ----

    internal SpeakablePage? Page => Reading?.SpeakablePage();

    internal uint Length => (uint)(Page?.Text.Length ?? 0);

    internal string Slice(uint start, uint end)
    {
        string text = Page?.Text ?? string.Empty;
        start = Math.Min(start, (uint)text.Length);
        end = Math.Clamp(end, start, (uint)text.Length);
        return text[(int)start..(int)end];
    }

    /// <summary>Locator range → character range, through the word table.</summary>
    internal (uint Start, uint End) TextRangeOf(uint lo, uint hi)
    {
        (uint, uint)? range = null;
        foreach (WordSpan word in Page?.Words ?? [])
        {
            if (word.LocatorStart < hi && lo < word.LocatorEnd)
            {
                range = range is { } r
                    ? (Math.Min(r.Item1, word.TextStart), Math.Max(r.Item2, word.TextEnd))
                    : (word.TextStart, word.TextEnd);
            }
        }
        return range ?? (0, 0);
    }

    /// <summary>Character range → locator range, the same way round.</summary>
    internal (uint Start, uint End)? LocatorRangeOf(uint start, uint end)
    {
        (uint, uint)? range = null;
        foreach (WordSpan word in Page?.Words ?? [])
        {
            if (word.TextStart < end && start < word.TextEnd)
            {
                range = range is { } r
                    ? (Math.Min(r.Item1, word.LocatorStart), Math.Max(r.Item2, word.LocatorEnd))
                    : (word.LocatorStart, word.LocatorEnd);
            }
        }
        return range;
    }

    /// <summary>
    /// The word boundaries across the page, as character offsets.
    /// </summary>
    internal IReadOnlyList<(uint Start, uint End)> Words =>
        Page?.Words.Select(w => (w.TextStart, w.TextEnd)).ToList() ?? [];

    /// <summary>
    /// The line boundaries, derived the way every other implementation of
    /// this derives them: a line runs from its first word to the next
    /// line's first word, so every character belongs to exactly one line
    /// including the punctuation between words that no word span covers.
    /// </summary>
    internal IReadOnlyList<(uint Start, uint End)> Lines
    {
        get
        {
            if (Reading is not { } session || Page is not { } page)
            {
                return [];
            }
            var starts = new List<uint>();
            foreach (TextRun run in session.PageTextRuns() ?? [])
            {
                foreach (WordSpan word in page.Words)
                {
                    if (word.LocatorStart >= run.LocatorStart && word.LocatorStart < run.LocatorEnd)
                    {
                        starts.Add(word.TextStart);
                        break;
                    }
                }
            }
            if (starts.Count == 0)
            {
                return [];
            }
            starts[0] = 0;
            var lines = new List<(uint, uint)>(starts.Count);
            for (int i = 0; i < starts.Count; i++)
            {
                lines.Add((starts[i], i + 1 < starts.Count ? starts[i + 1] : Length));
            }
            return lines;
        }
    }

    /// <summary>
    /// Screen rectangles for a character range — one per line it touches,
    /// which is what UIA specifies and what a screen reader's highlight
    /// draws.
    /// </summary>
    internal double[] RectanglesFor(uint start, uint end)
    {
        if (Reading is not { } session || LocatorRangeOf(start, end) is not { } locators)
        {
            return [];
        }
        double scale = View.XamlRoot?.RasterizationScale ?? 1.0;
        Point origin = View.TransformToVisual(null).TransformPoint(new Point(0, 0));

        var flat = new List<double>();
        foreach (PageRect rect in session.RangeRects(locators.Start, locators.End))
        {
            flat.Add((origin.X + rect.X) * scale);
            flat.Add((origin.Y + rect.Y) * scale);
            flat.Add(rect.Width * scale);
            flat.Add(rect.Height * scale);
        }
        return [.. flat];
    }

    internal IRawElementProviderSimple Element => ProviderFromPeer(this);

    /// <summary>Ask the view to select a character range.</summary>
    internal void Select(uint start, uint end)
    {
        // Nothing to do: selection does not cross the C ABI, so a client's
        // request is honoured as far as it can be, which is not at all.
        // Saying so here beats a silent no-op somewhere deeper.
        _ = (start, end);
    }
}
