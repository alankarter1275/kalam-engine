using System.Windows.Automation;
using System.Windows.Automation.Provider;
using System.Windows.Automation.Text;

namespace Chapbook.Wpf;

/// <summary>
/// A range of the page, as a UI Automation client holds it.
/// </summary>
/// <remarks>
/// Endpoints are character offsets into the speakable string, and they move
/// under the client's hand. Every read clamps to the page as it is now, so
/// a range held across a page turn degenerates rather than indexing off the
/// end of a shorter one.
/// </remarks>
internal sealed class PageRange(PagePeer peer, uint start, uint end) : ITextRangeProvider
{
    private uint _start = start;
    private uint _end = end;

    private (uint Start, uint End) Span()
    {
        uint length = peer.Length;
        uint from = Math.Min(_start, length);
        return (from, Math.Clamp(_end, from, length));
    }

    private void SetSpan(uint from, uint to)
    {
        _start = from;
        _end = Math.Max(from, to);
    }

    /// <summary>
    /// A UIA unit in the engine's vocabulary. Character, word and line are
    /// real; everything larger resolves to the page, which is UIA's own
    /// documented fallback — a provider that cannot honour a unit uses the
    /// next larger one it can.
    /// </summary>
    private static TextGranularity Granularity(TextUnit unit) => unit switch
    {
        TextUnit.Character => TextGranularity.Character,
        TextUnit.Word => TextGranularity.Word,
        TextUnit.Line => TextGranularity.Line,
        _ => TextGranularity.Page,
    };

    public ITextRangeProvider Clone()
    {
        (uint from, uint to) = Span();
        return new PageRange(peer, from, to);
    }

    public bool Compare(ITextRangeProvider range) =>
        range is PageRange other && other.Span() == Span();

    public int CompareEndpoints(
        TextPatternRangeEndpoint endpoint,
        ITextRangeProvider targetRange,
        TextPatternRangeEndpoint targetEndpoint)
    {
        if (targetRange is not PageRange other)
        {
            return 0;
        }
        return (int)Endpoint(Span(), endpoint) - (int)Endpoint(other.Span(), targetEndpoint);
    }

    private static uint Endpoint((uint Start, uint End) span, TextPatternRangeEndpoint which) =>
        which == TextPatternRangeEndpoint.End ? span.End : span.Start;

    public void ExpandToEnclosingUnit(TextUnit unit)
    {
        (uint from, _) = Span();
        if (peer.Around(from, Granularity(unit)) is { } bounds)
        {
            SetSpan(bounds.Start, bounds.End);
        }
        else
        {
            SetSpan(0, peer.Length);
        }
    }

    public ITextRangeProvider? FindAttribute(int attribute, object value, bool backward) =>
        // Nothing is reported as an attribute, so there is nothing to
        // search for. Saying so is honest; answering with the document
        // range would be a lie a client acts on.
        null;

    public ITextRangeProvider? FindText(string text, bool backward, bool ignoreCase)
    {
        if (string.IsNullOrEmpty(text))
        {
            return null;
        }
        (uint from, uint to) = Span();
        string haystack = peer.Slice(from, to);
        StringComparison how = ignoreCase
            ? StringComparison.CurrentCultureIgnoreCase
            : StringComparison.CurrentCulture;
        int at = backward ? haystack.LastIndexOf(text, how) : haystack.IndexOf(text, how);
        return at < 0
            ? null
            : new PageRange(peer, from + (uint)at, from + (uint)(at + text.Length));
    }

    public object GetAttributeValue(int attribute) =>
        // Text attributes are not absent from the *engine* — a glyph run
        // knows its face and its size — but they are absent from the text
        // *surface*, which is deliberately a much smaller thing to hold
        // still than the paint vocabulary.
        //
        // This is the sentinel the WinUI peer has no equivalent of, and it
        // is the difference between "no attributes here" and a null a
        // client has to interpret.
        AutomationElementIdentifiers.NotSupported;

    public double[] GetBoundingRectangles()
    {
        (uint from, uint to) = Span();
        return peer.Rectangles(from, to);
    }

    public IRawElementProviderSimple[] GetChildren() => [];

    public IRawElementProviderSimple GetEnclosingElement() => peer.Element;

    public string GetText(int maxLength)
    {
        (uint from, uint to) = Span();
        if (maxLength >= 0)
        {
            to = Math.Min(to, from + (uint)maxLength);
        }
        return peer.Slice(from, to);
    }

    public int Move(TextUnit unit, int count)
    {
        if (count == 0)
        {
            return 0;
        }
        IReadOnlyList<PageText.Span> units = peer.Boundaries(Granularity(unit));
        if (units.Count == 0)
        {
            return 0;
        }
        (uint from, _) = Span();
        int at = PageText.IndexOf(units, from);
        int target = Math.Clamp(at + count, 0, units.Count - 1);
        SetSpan(units[target].Start, units[target].End);
        // The count actually moved, which is not the count asked for when
        // the range ran into an end of the page. A client uses the
        // difference to know it has arrived.
        return target - at;
    }

    public int MoveEndpointByUnit(TextPatternRangeEndpoint endpoint, TextUnit unit, int count)
    {
        if (count == 0)
        {
            return 0;
        }
        IReadOnlyList<PageText.Span> units = peer.Boundaries(Granularity(unit));
        if (units.Count == 0)
        {
            return 0;
        }
        (uint from, uint to) = Span();
        int at = PageText.IndexOf(units, Endpoint((from, to), endpoint));
        int target = Math.Clamp(at + count, 0, units.Count - 1);
        uint moved = endpoint == TextPatternRangeEndpoint.Start
            ? units[target].Start
            : units[target].End;

        // An endpoint pushed past the other takes it along, which is what
        // UIA says a degenerate range is.
        if (endpoint == TextPatternRangeEndpoint.Start)
        {
            SetSpan(moved, Math.Max(to, moved));
        }
        else
        {
            SetSpan(Math.Min(from, moved), moved);
        }
        return target - at;
    }

    public void MoveEndpointByRange(
        TextPatternRangeEndpoint endpoint,
        ITextRangeProvider targetRange,
        TextPatternRangeEndpoint targetEndpoint)
    {
        if (targetRange is not PageRange other)
        {
            return;
        }
        uint target = Endpoint(other.Span(), targetEndpoint);
        (uint from, uint to) = Span();
        if (endpoint == TextPatternRangeEndpoint.Start)
        {
            SetSpan(target, Math.Max(to, target));
        }
        else
        {
            SetSpan(Math.Min(from, target), target);
        }
    }

    public void Select()
    {
        (uint from, uint to) = Span();
        peer.Select(from, to);
    }

    public void AddToSelection()
    {
        // `SupportedTextSelection.Single` already said so; this is the call
        // that has to agree with it.
    }

    public void RemoveFromSelection()
    {
    }

    public void ScrollIntoView(bool alignToTop)
    {
        // Every range this provider hands out is on the page that is on the
        // screen, so there is never anything to scroll — pagination's answer
        // to a question a scrolling document has to work at.
    }
}
