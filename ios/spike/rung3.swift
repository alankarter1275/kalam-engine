// Rung 3 of the iOS ladder: it draws.
//
// Two questions, both of which only this side of the boundary can answer.
// The channel order: sepia paper, not black text, because black on white
// cannot tell RGBA from BGRA. And the pixel path: the naive form
// (CGBitmapContext, then CGBitmapContextCreateImage) against a CGImage
// built directly over the engine's buffer with a CGDataProvider — the
// form whose creation copies nothing, with the buffer's lifetime tied to
// the provider. Both are timed, both are decoded back, and both must
// agree, or one of the two labels is a lie.
//
// Arguments: <book.epub> <fonts-dir> <library-dir> <out-dir>

import CChapbook
import CoreGraphics
import Foundation
import ImageIO
import QuartzCore
import UniformTypeIdentifiers

// The .rawValue bridge, as rung 2 found and recorded.
let OK = Int32(CB_OK.rawValue)
let ROTATION_NONE = UInt32(CB_ROTATION_NONE.rawValue)
let THEME_SEPIA = UInt32(CB_THEME_SEPIA.rawValue)
let SCOPE_THIS_BOOK = UInt32(CB_SCOPE_THIS_BOOK.rawValue)

let args = CommandLine.arguments
guard args.count == 5 else {
    print("usage: rung3 <book.epub> <fonts-dir> <library-dir> <out-dir>")
    exit(64)
}

var failures = 0
@MainActor func expect(_ ok: Bool, _ what: String) {
    print("\(ok ? "ok " : "FAIL") \(what)")
    if !ok { failures += 1 }
}

func ms(_ body: () -> Void) -> Double {
    let start = DispatchTime.now().uptimeNanoseconds
    body()
    return Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
}

// ---- A sepia page from the engine ----

guard let fonts = cb_font_source_embedded(args[2], "Crimson Text"),
      let config = cb_config_new(fonts),
      cb_config_set_library_dir(config, args[3]) == OK,
      let session = cb_session_open_path(args[1], config)
else {
    print("FAIL open")
    exit(1)
}

let metrics = cb_metrics(
    width: 600, height: 800,
    margin_top: 40, margin_right: 40, margin_bottom: 40, margin_left: 40,
    dpi_scale: 3, rotation: ROTATION_NONE
)
expect(cb_session_set_metrics(session, metrics) == OK, "metrics at an iPhone's @3x")

var settings = cb_settings()
expect(cb_session_settings(session, &settings) == OK, "settings read back")
settings.theme = THEME_SEPIA
expect(cb_session_set_settings(session, settings, SCOPE_THIS_BOOK) == OK, "sepia is in force")

var w: UInt32 = 0, h: UInt32 = 0
guard cb_session_render_size(session, &w, &h) == OK else { print("FAIL render size"); exit(1) }
let width = Int(w), height = Int(h), stride = Int(w) * 4

// The engine's buffer, allocated the way the provider path wants it: raw,
// with a lifetime the CGDataProvider release callback will end. Rendering
// into it is the same cb_session_render_into every path shares.
let pixels = UnsafeMutableRawPointer.allocate(byteCount: height * stride, alignment: 16)
let renderMs = ms {
    let rc = cb_session_render_into(session, pixels.assumingMemoryBound(to: UInt8.self),
                                    height * stride, w, h, stride)
    if rc != OK { print("FAIL render_into \(rc)"); exit(1) }
}

// ---- The channel order, read off the raw bytes first ----

// Sepia paper sampled (246, 240, 226) on the Android device: warm,
// R > G > B. In premultiplied RGBA8888 the margin pixel's bytes must
// already say so — a swap here is a bug below CoreGraphics, not in it.
let corner = pixels.assumingMemoryBound(to: UInt8.self) + (10 * stride + 10 * 4)
let (r0, g0, b0, a0) = (corner[0], corner[1], corner[2], corner[3])
print("     raw margin bytes: (\(r0), \(g0), \(b0), \(a0))")
expect(r0 > g0 && g0 > b0 && a0 == 255, "the raw bytes are warm: RGBA, not BGRA")

// ---- Path A: the naive form, which copies on the way out ----

let rgba = CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
let space = CGColorSpaceCreateDeviceRGB()

var imageA: CGImage?
let pathAMs = ms {
    let ctx = CGContext(data: pixels, width: width, height: height,
                        bitsPerComponent: 8, bytesPerRow: stride,
                        space: space, bitmapInfo: rgba)
    imageA = ctx?.makeImage() // CGBitmapContextCreateImage: the copy
}
expect(imageA != nil, String(format: "CGBitmapContext + makeImage: %.2f ms", pathAMs))

// ---- Path B: the engine's buffer *is* the image ----

// The release callback is the lifetime contract: CoreGraphics calls it
// when the last retain on the image goes away, and only then may the
// buffer die. A shell that frees the buffer on its own schedule draws
// garbage with no error, which is why the callback owns the deallocate.
nonisolated(unsafe) var released = false
let provider = CGDataProvider(
    dataInfo: nil, data: pixels, size: height * stride,
    releaseData: { _, data, _ in
        data.deallocate()
        released = true
    })!

var imageB: CGImage?
let pathBMs = ms {
    imageB = CGImage(width: width, height: height,
                     bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: stride,
                     space: space, bitmapInfo: CGBitmapInfo(rawValue: rgba),
                     provider: provider, decode: nil, shouldInterpolate: false,
                     intent: .defaultIntent)
}
expect(imageB != nil, String(format: "CGImage over the engine's buffer: %.3f ms", pathBMs))

// ---- Both paths must decode to the same warm paper ----

// Drawing into a context whose format is *known* is what checks the
// labels: if the bitmapInfo above lied about byte order, the round trip
// comes back cold blue, exactly as the Android check was designed.
@MainActor func decodedCorner(_ image: CGImage, _ what: String) -> (UInt8, UInt8, UInt8) {
    var out = [UInt8](repeating: 0, count: 4 * 32 * 32)
    out.withUnsafeMutableBytes { buf in
        let ctx = CGContext(data: buf.baseAddress, width: 32, height: 32,
                            bitsPerComponent: 8, bytesPerRow: 32 * 4,
                            space: space, bitmapInfo: rgba)!
        // The top-left corner of the page, scaled so (10,10) lands inside.
        ctx.interpolationQuality = .none
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: 32, height: 32))
    }
    _ = what
    let p = (out[0], out[1], out[2])
    return p
}

let (ra, ga, ba) = decodedCorner(imageA!, "A")
let (rb, gb, bb) = decodedCorner(imageB!, "B")
print("     decoded paper: path A (\(ra), \(ga), \(ba)), path B (\(rb), \(gb), \(bb))")
expect(ra > ga && ga > ba, "path A round-trips warm")
expect(rb > gb && gb > bb, "path B round-trips warm")
expect((ra, ga, ba) == (rb, gb, bb), "and the two paths agree pixel for pixel")

// ---- The layer: the engine's pixels as CALayer.contents ----

let layer = CALayer()
layer.frame = CGRect(x: 0, y: 0, width: width, height: height)
layer.contents = imageB
var layerCorner = [UInt8](repeating: 0, count: 4)
layerCorner.withUnsafeMutableBytes { buf in
    let ctx = CGContext(data: buf.baseAddress, width: 1, height: 1,
                        bitsPerComponent: 8, bytesPerRow: 4,
                        space: space, bitmapInfo: rgba)!
    // One device pixel of the layer's top-left, through QuartzCore.
    ctx.translateBy(x: -10, y: -CGFloat(height) + 11)
    layer.render(in: ctx)
}
expect(layerCorner[0] > layerCorner[1] && layerCorner[1] > layerCorner[2],
       "CALayer.contents draws the engine's buffer, still warm")

// ---- A human can look ----

let png = args[4] + "/rung3-sepia.png"
if let dest = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: png) as CFURL, UTType.png.identifier as CFString, 1, nil) {
    CGImageDestinationAddImage(dest, imageB!, nil)
    expect(CGImageDestinationFinalize(dest), "the page is at \(png)")
}

// ---- The lifetime contract, observed ----

expect(!released, "the buffer outlives its images")
imageA = nil
imageB = nil
// The provider still holds the buffer until its own last reference dies;
// CoreGraphics is free to defer the release callback past this line, so
// the assertion above (not-yet) is the only half that is deterministic.

print(String(format: "     render_into %.1f ms for %dx%d; create: A %.2f ms, B %.3f ms",
             renderMs, width, height, pathAMs, pathBMs))
cb_session_close(session)
print(failures == 0 ? "rung 3: it draws" : "rung 3: \(failures) failure(s)")
exit(failures == 0 ? 0 : 1)
