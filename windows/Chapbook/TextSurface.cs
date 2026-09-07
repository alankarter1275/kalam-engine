namespace Chapbook;

/// <summary>A rectangle in page space, logical pixels.</summary>
public readonly record struct PageRect(float X, float Y, float Width, float Height)
{
    internal static PageRect From(NativeRect r) => new(r.X, r.Y, r.W, r.H);
}

/// <summary>
/// One visual line: its text as shaped, where it sits, and the locator
/// range it covers.
/// </summary>
/// <remarks>
/// <see cref="Text"/> is the shaped line — whitespace collapsed, soft
/// hyphens stripped, generated marks included — so its character count is
/// <b>not</b> the locator span's width. Use the locator range for geometry
/// and selection; use the text for reading.
/// </remarks>
public readonly record struct TextRun(
    string Text, PageRect Rect, uint LocatorStart, uint LocatorEnd);

/// <summary>
/// One word: where it sits in the speakable string, and in locator space.
/// </summary>
/// <remarks>
/// The two spaces are the whole point. A speech engine reports progress as
/// offsets into the string it was handed; the locator range is how that
/// progress comes back to the page as geometry or a selection.
/// </remarks>
public readonly record struct WordSpan(
    uint TextStart, uint TextEnd, uint LocatorStart, uint LocatorEnd);

/// <summary>
/// The current page as speech and accessibility want it: one collapsed
/// string plus the word table that maps back into locator space.
/// </summary>
public sealed record SpeakablePage(string Text, IReadOnlyList<WordSpan> Words);

public sealed partial class Session
{
    /// <summary>
    /// The current page's lines, in reading order, or <c>null</c> until it
    /// is laid out.
    /// </summary>
    /// <remarks>
    /// <para>
    /// Empty rather than null for a laid-out page with nothing to read — a
    /// comic, an image-only page. The two are a different answer on
    /// purpose, and a host building an accessibility tree needs both.
    /// </para>
    /// <para>
    /// <b>Setting the metrics is not laying out.</b> Layout is lazy, so a
    /// session that has only been told how big a page is has no page yet
    /// and every accessor here answers <c>null</c>. Rendering lays it out;
    /// so does asking for <see cref="PageCount"/>, which is the cheap way
    /// to force it when a host wants the text before it wants the pixels.
    /// </para>
    /// </remarks>
    public IReadOnlyList<TextRun>? PageTextRuns()
    {
        Status counting = Interop.cb_session_page_text_run_count(Live(), out nuint count);
        if (counting == Status.Unavailable)
        {
            return null;
        }
        ChapbookException.Check(counting, nameof(PageTextRuns));

        var runs = new List<TextRun>((int)count);
        for (nuint i = 0; i < count; i++)
        {
            nuint index = i;
            ChapbookException.Check(
                Interop.cb_session_page_text_run(Live(), index, out NativeTextRun run),
                nameof(PageTextRuns));
            string text = Strings.Read((byte[]? b, nuint c, out nuint n) =>
                Interop.cb_session_page_text_run_text(Live(), index, b, c, out n),
                nameof(PageTextRuns)) ?? string.Empty;
            runs.Add(new TextRun(text, PageRect.From(run.Rect), run.LocatorStart, run.LocatorEnd));
        }
        return runs;
    }

    /// <summary>
    /// The page as one string plus its word table, or <c>null</c> until it
    /// is laid out.
    /// </summary>
    public SpeakablePage? SpeakablePage()
    {
        string? text = Strings.Read((byte[]? b, nuint c, out nuint n) =>
            Interop.cb_session_page_speakable_text(Live(), b, c, out n), nameof(SpeakablePage));
        if (text is null)
        {
            return null;
        }
        ChapbookException.Check(
            Interop.cb_session_page_word_count(Live(), out nuint count), nameof(SpeakablePage));
        var words = new List<WordSpan>((int)count);
        for (nuint i = 0; i < count; i++)
        {
            ChapbookException.Check(
                Interop.cb_session_page_word(Live(), i, out NativeWordSpan span),
                nameof(SpeakablePage));
            words.Add(new WordSpan(
                span.TextStart, span.TextEnd, span.LocatorStart, span.LocatorEnd));
        }
        return new SpeakablePage(text, words);
    }

    /// <summary>
    /// Page-space rects covering a locator range — one per line the range
    /// touches.
    /// </summary>
    /// <remarks>
    /// One per line rather than one spanning rect, because a logically
    /// contiguous range need not be visually contiguous: select through a
    /// Latin word embedded in an Arabic line and the two selected pieces
    /// sit at opposite ends with unselected letters between them.
    /// </remarks>
    public IReadOnlyList<PageRect> RangeRects(uint start, uint end)
    {
        Status sizing = Interop.cb_session_range_rects(Live(), start, end, null, 0, out nuint needed);
        if (sizing == Status.Unavailable || needed == 0)
        {
            return [];
        }
        if (sizing != Status.BufferTooSmall)
        {
            ChapbookException.Check(sizing, nameof(RangeRects));
        }
        var buffer = new NativeRect[needed];
        ChapbookException.Check(
            Interop.cb_session_range_rects(Live(), start, end, buffer, needed, out _),
            nameof(RangeRects));
        return Array.ConvertAll(buffer, PageRect.From);
    }

    /// <summary>
    /// The word under a point, as a locator range — a dictionary tap's
    /// question. <c>null</c> off text, and on whitespace or bare
    /// punctuation: a tap on a comma looks nothing up.
    /// </summary>
    /// <remarks>
    /// The point is in the logical units <see cref="SetMetrics"/> was
    /// given, and the engine undoes any rotation itself.
    /// </remarks>
    public (uint Start, uint End)? WordAt(float x, float y)
    {
        Status status = Interop.cb_session_word_at(Live(), x, y, out uint start, out uint end);
        if (status == Status.Unavailable)
        {
            return null;
        }
        ChapbookException.Check(status, nameof(WordAt));
        return (start, end);
    }
}
