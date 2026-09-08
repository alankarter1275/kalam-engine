using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Automation.Provider;
using System.Windows.Media;

namespace Chapbook.Wpf;

/// <summary>
/// The page behind UI Automation, so Narrator can read it.
/// </summary>
/// <remarks>
/// <para>
/// The fourth implementation of the engine's text surface against a real
/// assistive stack — after GTK's <c>AccessibleText</c>, the Win32 shell's
/// raw <c>ITextProvider</c>, and the WinUI peer. The arithmetic is
/// <see cref="PageText"/>'s and lives in the binding, so what is left here
/// is only the shape WPF asks in.
/// </para>
/// <para>
/// Two differences from the WinUI peer are worth naming, because they are
/// the ones that bear on choosing between the two frameworks.
/// <c>GetBoundingRectangles</c> returns its array here and takes an
/// <c>out</c> parameter there. And WPF has
/// <see cref="AutomationElementIdentifiers.NotSupported"/> — the sentinel
/// for "this control does not report that attribute" — which WinUI exposes
/// no equivalent of, so the WinUI peer answers <c>null</c> and cannot say
/// anything more specific.
/// </para>
/// </remarks>
internal sealed class PagePeer(SessionView view)
    : FrameworkElementAutomationPeer(view), ITextProvider
{
    private SessionView View => view;

    private PageText? Page => view.Page;

    protected override AutomationControlType GetAutomationControlTypeCore() =>
        // A book's page is a document, which is what makes Narrator offer
        // its reading commands rather than treat this as a nameless canvas.
        AutomationControlType.Document;

    protected override string GetClassNameCore() => nameof(SessionView);

    protected override string GetNameCore() =>
        view.Reading?.Title is { Length: > 0 } title ? title : base.GetNameCore();

    public override object GetPattern(PatternInterface pattern) =>
        pattern == PatternInterface.Text ? this : base.GetPattern(pattern);

    // ---- ITextProvider ----

    public ITextRangeProvider DocumentRange => new PageRange(this, 0, Length);

    public SupportedTextSelection SupportedTextSelection =>
        // One selection, matching what the session has. Nothing here
        // pretends to support several.
        SupportedTextSelection.Single;

    public ITextRangeProvider[] GetSelection() =>
        // No selection crosses the C ABI, so there is none to report. An
        // empty array means "none right now"; null would mean "this control
        // does not do selections", which is a different and wrong answer.
        [];

    public ITextRangeProvider[] GetVisibleRanges() =>
        // A page is exactly what is visible. That is what paginating rather
        // than scrolling buys, showing up here as a simplification.
        [DocumentRange];

    public ITextRangeProvider? RangeFromChild(IRawElementProviderSimple childElement) =>
        // No child elements: nothing embedded, no images exposed as objects.
        null;

    public ITextRangeProvider RangeFromPoint(Point screenLocation)
    {
        if (view.Reading is not { } session || Page is not { } page)
        {
            return new PageRange(this, 0, 0);
        }
        Point local = view.PointFromScreen(screenLocation);

        // Word precision: the word under the point answers with its own
        // range, which is what word-oriented review reads. A degenerate
        // range at the nearest character would be more literal and less
        // useful.
        if (session.WordAt((float)local.X, (float)local.Y) is not { } locator)
        {
            return new PageRange(this, 0, 0);
        }
        PageText.Span span = page.TextRange(locator.Start, locator.End);
        return new PageRange(this, span.Start, span.End);
    }

    // ---- What the range provider needs ----

    internal uint Length => Page?.Length ?? 0;

    internal string Slice(uint start, uint end) => Page?.Slice(start, end) ?? string.Empty;

    internal IReadOnlyList<PageText.Span> Boundaries(TextGranularity unit) =>
        Page?.Boundaries(unit) ?? [];

    internal PageText.Span? Around(uint offset, TextGranularity unit) =>
        Page?.Around(offset, unit);

    internal IRawElementProviderSimple Element => ProviderFromPeer(this);

    /// <summary>
    /// Screen rectangles for a character range — one per line it touches,
    /// which is what UIA specifies and what a screen reader's highlight
    /// draws.
    /// </summary>
    internal double[] Rectangles(uint start, uint end)
    {
        if (view.Reading is not { } session
            || Page is not { } page
            || page.LocatorRange(start, end) is not { } locators)
        {
            return [];
        }
        var flat = new List<double>();
        foreach (PageRect rect in session.RangeRects(locators.Start, locators.End))
        {
            // Page space is the view's own DIP space — the metrics came
            // from its bounds and nothing rotates — so this is a transform
            // to the screen and nothing else.
            Point origin = view.PointToScreen(new Point(rect.X, rect.Y));
            Point far = view.PointToScreen(new Point(rect.X + rect.Width, rect.Y + rect.Height));
            flat.Add(origin.X);
            flat.Add(origin.Y);
            flat.Add(far.X - origin.X);
            flat.Add(far.Y - origin.Y);
        }
        return [.. flat];
    }

    internal void Select(uint start, uint end)
    {
        // Selection does not cross the C ABI, so a client's request is
        // honoured as far as it can be, which is not at all. Saying so here
        // beats a silent no-op somewhere deeper.
        _ = (start, end);
        _ = View;
    }
}
