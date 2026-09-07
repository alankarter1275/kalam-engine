using System.Runtime.InteropServices;

namespace Chapbook;

public sealed partial class Session
{
    /// <summary>
    /// The buffer size the current page needs, in device pixels.
    /// </summary>
    /// <remarks>
    /// It accounts for rotation — a quarter turn swaps the axes — which is
    /// why a host asks rather than computing it from the metrics it set.
    /// </remarks>
    public (uint Width, uint Height) RenderSize()
    {
        ChapbookException.Check(
            Interop.cb_session_render_size(Live(), out uint width, out uint height),
            nameof(RenderSize));
        return (width, height);
    }

    /// <summary>
    /// Draw the current page straight into a buffer the caller owns.
    /// </summary>
    /// <param name="pixels">
    /// Premultiplied RGBA8888, exactly <paramref name="stride"/> ×
    /// <paramref name="height"/> bytes.
    /// </param>
    /// <param name="stride">Bytes per row. Usually width × 4.</param>
    /// <remarks>
    /// <para>
    /// The size must match <see cref="RenderSize"/>: the engine refuses
    /// rather than misdraws when it does not. An unrotated page with a
    /// tight stride costs no allocation and no copy; a rotated page or a
    /// padded stride is correct but goes through an intermediate.
    /// </para>
    /// <para>
    /// <b>Pixels are premultiplied RGBA, not BGRA.</b> Windows imaging is
    /// mostly BGRA — a <c>WriteableBitmap</c>'s back buffer certainly is —
    /// so a host either swizzles or asks its surface for an RGBA format.
    /// The failure mode of getting it wrong is a page that reads cold blue
    /// where the sepia theme should be warm, which is a colour bug people
    /// see rather than an error anything reports.
    /// </para>
    /// </remarks>
    public void RenderInto(nint pixels, long length, uint width, uint height, long stride) =>
        ChapbookException.Check(
            Interop.cb_session_render_into(
                Live(), pixels, (nuint)length, width, height, (nuint)stride),
            nameof(RenderInto));

    /// <summary>
    /// Draw into managed memory. Convenience over
    /// <see cref="RenderInto(nint, long, uint, uint, long)"/>; the buffer
    /// is pinned for the call and nothing is copied.
    /// </summary>
    public void RenderInto(Span<byte> pixels, uint width, uint height, long stride)
    {
        unsafe
        {
            fixed (byte* p = pixels)
            {
                RenderInto((nint)p, pixels.Length, width, height, stride);
            }
        }
    }

    /// <summary>
    /// Draw the page into a freshly allocated array, sized from
    /// <see cref="RenderSize"/>.
    /// </summary>
    /// <remarks>
    /// The simple path, and the one that allocates per frame. A host
    /// drawing continuously should keep its own buffer — or better, hand
    /// over the one its surface already has — and use the overloads above.
    /// </remarks>
    public (byte[] Pixels, uint Width, uint Height) Render()
    {
        (uint width, uint height) = RenderSize();
        byte[] pixels = new byte[(long)width * height * 4];
        RenderInto(pixels, width, height, (long)width * 4);
        return (pixels, width, height);
    }
}
