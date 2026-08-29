import CChapbook
import CoreGraphics
import Foundation

extension Session {
    /// The device-pixel surface size for the current metrics, rotation
    /// included. Allocate exactly this; never compute it yourself — the
    /// logical-to-device round trip does not always land on the pixel it
    /// started from, and the engine refuses a mismatched buffer rather
    /// than misdrawing into it.
    public func renderSize() throws -> (width: Int, height: Int) {
        var w: UInt32 = 0
        var h: UInt32 = 0
        try check(cb_session_render_size(raw, &w, &h))
        return (Int(w), Int(h))
    }

    /// Rasterize the current page and wrap it as a `CGImage` **without
    /// copying**: the image is built directly over the engine-filled
    /// buffer through a `CGDataProvider` whose release callback owns the
    /// deallocation. Measured on a simulator, the copying alternative
    /// (`CGBitmapContext` + `CGBitmapContextCreateImage`) costs 5.5 ms a
    /// frame at @3x against this path's microsecond, so this is the only
    /// form the library offers.
    ///
    /// Hand the result to `CALayer.contents` or a `UIImage` and let ARC
    /// manage the rest — the buffer lives exactly as long as CoreGraphics
    /// still holds the image.
    public func renderImage() throws -> CGImage {
        var w: UInt32 = 0
        var h: UInt32 = 0
        try check(cb_session_render_size(raw, &w, &h))
        let stride = Int(w) * 4
        let length = stride * Int(h)

        let buffer = UnsafeMutableRawPointer.allocate(byteCount: length, alignment: 4)
        let status = cb_session_render_into(
            raw, buffer.assumingMemoryBound(to: UInt8.self), length, w, h, stride)
        guard status == C.ok else {
            buffer.deallocate()
            throw ChapbookError.last(status)
        }

        // Premultiplied RGBA8888 is what the engine writes and what this
        // label names; the channel order was settled with the sepia
        // theme, whose warm paper cannot be mistaken for a BGRA swap.
        let bitmapInfo = CGBitmapInfo(
            rawValue: CGImageAlphaInfo.premultipliedLast.rawValue
                | CGBitmapInfo.byteOrder32Big.rawValue)
        guard
            let provider = CGDataProvider(
                dataInfo: buffer, data: buffer, size: length,
                releaseData: { info, _, _ in info?.deallocate() })
        else {
            buffer.deallocate()
            throw ChapbookError(status: nil, message: "CGDataProvider construction failed")
        }
        // From here the provider owns the buffer, whatever happens.
        guard
            let image = CGImage(
                width: Int(w), height: Int(h), bitsPerComponent: 8, bitsPerPixel: 32,
                bytesPerRow: stride, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: bitmapInfo, provider: provider, decode: nil,
                shouldInterpolate: false, intent: .defaultIntent)
        else {
            throw ChapbookError(status: nil, message: "CGImage construction failed")
        }
        return image
    }
}
