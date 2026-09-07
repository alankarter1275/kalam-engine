namespace Chapbook;

/// <summary>
/// The current page arranged the way an accessibility tree wants it:
/// one string, and the word and line boundaries inside it.
/// </summary>
/// <remarks>
/// <para>
/// Every assistive stack asks a differently shaped question of the
/// engine's text surface — AT-SPI wants text at a granularity around an
/// offset, UI Automation wants a range object that moves its own endpoints
/// by unit — but the arithmetic underneath is the same everywhere, and
/// doing it once here is what keeps a WinUI peer and a WPF peer from being
/// a hundred lines of identical derivation with different type names on
/// top.
/// </para>
/// <para>
/// Offsets are character offsets into <see cref="Text"/>, which is the
/// space <see cref="WordSpan"/> already carries. Locator offsets appear
/// only in <see cref="LocatorRange"/> and
/// <see cref="Session.RangeRects"/>, which is where geometry comes from.
/// </para>
/// </remarks>
public sealed class PageText
{
    private readonly IReadOnlyList<WordSpan> _words;

    private PageText(string text, IReadOnlyList<WordSpan> words, IReadOnlyList<Span> lines)
    {
        Text = text;
        _words = words;
        Words = [.. words.Select(w => new Span(w.TextStart, w.TextEnd))];
        Lines = lines;
    }

    /// <summary>A range of the page, in character offsets.</summary>
    public readonly record struct Span(uint Start, uint End);

    /// <summary>The whole page as one string, whitespace collapsed.</summary>
    public string Text { get; }

    /// <summary>Word boundaries, in reading order and never overlapping.</summary>
    public IReadOnlyList<Span> Words { get; }

    /// <summary>
    /// Visual line boundaries.
    /// </summary>
    /// <remarks>
    /// A line runs from its own first word to the <i>next</i> line's first
    /// word, so every character belongs to exactly one line — including the
    /// punctuation and spacing between words, which no word span covers and
    /// which a reader asked to read a line still expects to hear.
    /// </remarks>
    public IReadOnlyList<Span> Lines { get; }

    /// <summary>Character count, which is what a range clamps against.</summary>
    public uint Length => (uint)Text.Length;

    /// <summary>
    /// Read the current page, or <c>null</c> when there is nothing laid out
    /// to read — a session with no metrics, a page still decoding, or a
    /// comic, which has no text layer at all.
    /// </summary>
    public static PageText? Of(Session session)
    {
        ArgumentNullException.ThrowIfNull(session);
        if (session.SpeakablePage() is not { } page || page.Text.Length == 0)
        {
            return null;
        }

        // Lines come from the layout's own visual lines; which words fall
        // inside a line's locator range decides where its text begins.
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

        var lines = new List<Span>(starts.Count);
        if (starts.Count > 0)
        {
            // The first line owns anything before its first word, and the
            // last owns everything after its own.
            starts[0] = 0;
            for (int i = 0; i < starts.Count; i++)
            {
                lines.Add(new Span(
                    starts[i],
                    i + 1 < starts.Count ? starts[i + 1] : (uint)page.Text.Length));
            }
        }
        return new PageText(page.Text, page.Words, lines);
    }

    /// <summary>A character range of the page, clamped to it.</summary>
    public string Slice(uint start, uint end)
    {
        start = Math.Min(start, Length);
        end = Math.Clamp(end, start, Length);
        return Text[(int)start..(int)end];
    }

    /// <summary>
    /// The span of <paramref name="unit"/> containing an offset, or
    /// <c>null</c> for a unit this page cannot answer.
    /// </summary>
    /// <remarks>
    /// <see cref="TextGranularity.Paragraph"/> is deliberately absent from
    /// what a page can answer: the speakable text collapses whitespace and
    /// carries no paragraph structure, and the only way to recover one
    /// would be to call a gap in locator offsets a break, which is a
    /// threshold dressed as a fact. A caller resolves it to the whole page,
    /// which is UI Automation's own documented fallback — a provider that
    /// cannot honour a unit uses the next larger one it can.
    /// </remarks>
    public Span? Around(uint offset, TextGranularity unit) => unit switch
    {
        TextGranularity.Character =>
            new Span(Math.Min(offset, Length), Math.Min(offset + 1, Length)),
        TextGranularity.Word => Containing(Words, offset),
        TextGranularity.Line => Containing(Lines, offset),
        _ => null,
    };

    private static Span? Containing(IReadOnlyList<Span> spans, uint offset)
    {
        foreach (Span span in spans)
        {
            if (span.Start <= offset && offset < span.End)
            {
                return span;
            }
        }
        // Past the last one: the last span is where the caret has ended up.
        return spans.Count > 0 ? spans[^1] : null;
    }

    /// <summary>
    /// Every boundary of a unit across the page, which is what a client
    /// stepping by unit walks along.
    /// </summary>
    public IReadOnlyList<Span> Boundaries(TextGranularity unit) => unit switch
    {
        TextGranularity.Character =>
            [.. Enumerable.Range(0, (int)Length).Select(i => new Span((uint)i, (uint)i + 1))],
        TextGranularity.Word => Words,
        TextGranularity.Line => Lines,
        // Anything larger than a line is the page, so there is one of them
        // and nowhere to step.
        _ => [new Span(0, Length)],
    };

    /// <summary>Which unit an offset falls in, for stepping from it.</summary>
    public static int IndexOf(IReadOnlyList<Span> units, uint offset)
    {
        int at = 0;
        for (int i = 0; i < units.Count; i++)
        {
            if (units[i].Start <= offset)
            {
                at = i;
            }
        }
        return at;
    }

    /// <summary>
    /// Character range → locator range, through the word table — the join
    /// that turns an offset into geometry, because
    /// <see cref="Session.RangeRects"/> speaks locator space.
    /// </summary>
    public Span? LocatorRange(uint start, uint end)
    {
        Span? range = null;
        foreach (WordSpan word in _words)
        {
            if (word.TextStart < end && start < word.TextEnd)
            {
                range = range is { } r
                    ? new Span(
                        Math.Min(r.Start, word.LocatorStart), Math.Max(r.End, word.LocatorEnd))
                    : new Span(word.LocatorStart, word.LocatorEnd);
            }
        }
        return range;
    }

    /// <summary>Locator range → character range, the same way round.</summary>
    public Span TextRange(uint lo, uint hi)
    {
        Span? range = null;
        foreach (WordSpan word in _words)
        {
            if (word.LocatorStart < hi && lo < word.LocatorEnd)
            {
                range = range is { } r
                    ? new Span(Math.Min(r.Start, word.TextStart), Math.Max(r.End, word.TextEnd))
                    : new Span(word.TextStart, word.TextEnd);
            }
        }
        return range ?? new Span(0, 0);
    }
}

/// <summary>
/// The units an accessibility client navigates by, named once so a peer
/// can map its own platform's vocabulary onto them.
/// </summary>
public enum TextGranularity
{
    Character,
    Word,
    Line,

    /// <summary>
    /// Anything larger than a line. <see cref="PageText.Around"/> answers
    /// <c>null</c> for it, and a caller resolves that to the whole page.
    /// </summary>
    Page,
}
