//! Panel policy: what a rasterized page has to become before it reaches a
//! particular screen.
//!
//! These deliberately live above the backends rather than inside one.
//! Greyscale conversion, dithering, and orientation are properties of the
//! target, not of the rasterizer that drew the page — a shell on an e-ink
//! device gets the same answer whether its pages come from the CPU
//! backend or a GPU one, and two shells on the same device agree with each
//! other.
//!
//! Both operate on rows of RGBA8, top-left origin, as every backend
//! produces. Pages paint over an opaque background, so premultiplied and
//! straight values coincide here and neither function has to care which it
//! was handed.

use std::borrow::Cow;

use chapbook_core::{PixelFormat, Rotation};

/// Convert a rasterized page for a panel that cannot show full color.
///
/// Luminance is Rec. 709 over the sRGB values, quantized to the format's
/// levels; with `dither` on, the quantization error is diffused
/// Floyd–Steinberg so a 16-level panel still shows a gradient instead of
/// banding. Alpha is untouched, and [`PixelFormat::Rgba`] is a no-op —
/// including on a colour e-ink panel, which resolves RGB through a filter
/// array and wants nothing done to it here.
pub fn quantize(rgba: &mut [u8], width: u32, height: u32, format: PixelFormat) {
    let PixelFormat::Grey { levels, dither } = format else {
        return;
    };
    // Floored, not capped: two levels is where the arithmetic stops
    // meaning anything, while an upper bound would be a guess about which
    // panels exist — and the guess used to be applied silently, rewriting
    // a deeper request rather than honouring it.
    let levels = f32::from(levels.max(2));
    let (w, h) = (width as usize, height as usize);
    // Loop-invariant: the step between adjacent output levels, and the
    // index of the highest one.
    let step = 255.0 / (levels - 1.0);
    let top = levels - 1.0;

    if !dither {
        // Undithered, every pixel is independent of its neighbours, so
        // this path wants none of the diffusion machinery below — not the
        // two error rows, and in particular not the clear-per-row that
        // keeping them costs, which is a second full-width pass over every
        // row of the page to zero values nothing will read.
        for px in rgba.chunks_exact_mut(4).take(w * h) {
            let value = ((luminance(px) / step).round().clamp(0.0, top) * step) as u8;
            px[0] = value;
            px[1] = value;
            px[2] = value;
        }
        return;
    }

    // Measured on 1448x1072 (Clara geometry), this loop is close to its
    // practical floor and further micro-optimization was tried and
    // rejected — don't redo it:
    //
    //   exact luminance lookup tables    +0.3%  (no gain; the int->float
    //                                           converts were never the
    //                                           bottleneck)
    //   multiply by 1/step, not divide   -8%    (not bit-exact: an ulp can
    //                                           flip a round at .5)
    //   rolling carry, no indexed error  -13%   (not bit-exact either;
    //                                           re-associating the sum
    //                                           changes the dither, and
    //                                           diffusion amplifies it)
    //
    // What remains is a serial dependency: every pixel needs the previous
    // pixel's residual, so there is no instruction-level parallelism left
    // to find and no SIMD to reach for. Making this genuinely cheaper
    // means doing less of it — scoping the work to the damaged region, or
    // letting an EPDC dither in hardware and asking for `Rgba` — not
    // shaving the inner loop.
    //
    // One row of forward error plus the next, so diffusion needs no full
    // second buffer.
    //
    // Walked as rows of pixels rather than by computed index: `(y * w + x)
    // * 4` made every one of the six reads and writes below a separately
    // bounds-checked access into the whole page, and the optimizer cannot
    // discharge those from the arithmetic alone. Chunking states the shape
    // it could not infer.
    let mut error = vec![0.0f32; w + 2];
    let mut next = vec![0.0f32; w + 2];
    for row in rgba.chunks_exact_mut(w * 4).take(h) {
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let lum = luminance(px) + error[x + 1];
            let quantized = (lum / step).round().clamp(0.0, top) * step;
            let value = quantized as u8;
            px[0] = value;
            px[1] = value;
            px[2] = value;
            // Floyd–Steinberg: 7/16 right, 3/16 down-left, 5/16 down,
            // 1/16 down-right.
            let residual = lum - quantized;
            error[x + 2] += residual * 7.0 / 16.0;
            next[x] += residual * 3.0 / 16.0;
            next[x + 1] += residual * 5.0 / 16.0;
            next[x + 2] += residual / 16.0;
        }
        std::mem::swap(&mut error, &mut next);
        // `fill` is a memset; the element-wise loop this replaces was a
        // second full-width pass over every row.
        next.fill(0.0);
    }
}

/// Rec. 709 luminance of one RGBA pixel's colour channels.
///
/// Pages paint over an opaque background, so this reads the same whether
/// the pixel is premultiplied or straight.
fn luminance(px: &[u8]) -> f32 {
    0.2126 * f32::from(px[0]) + 0.7152 * f32::from(px[1]) + 0.0722 * f32::from(px[2])
}

/// Turn a rasterized page for a panel mounted in a different orientation,
/// clockwise. Quarter turns swap the page's dimensions, so the returned
/// rows are `height` wide.
///
/// [`Rotation::None`] borrows: an unrotated shell is the common case — a
/// device is mounted the way it is mounted — and it was paying a full-page
/// allocation and copy every frame to be handed back what it already had.
/// Callers that genuinely need to own the pixels say `into_owned()`, which
/// costs nothing on the turned paths because those already allocated.
///
/// The inverse for input is [`chapbook_core::PageMetrics::panel_to_page`],
/// so a shell that turns its output here reads pointer coordinates back
/// through that.
pub fn rotate(rgba: &[u8], width: u32, height: u32, rotation: Rotation) -> Cow<'_, [u8]> {
    if rotation == Rotation::None {
        return Cow::Borrowed(rgba);
    }
    let (w, h) = (width as usize, height as usize);
    let dw = if rotation.swaps_axes() { h } else { w };
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = match rotation {
                Rotation::None => (x, y),
                Rotation::Quarter => (h - 1 - y, x),
                Rotation::Half => (w - 1 - x, h - 1 - y),
                Rotation::ThreeQuarter => (y, w - 1 - x),
            };
            let (from, to) = ((y * w + x) * 4, (dy * dw + dx) * 4);
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows filled with one opaque grey level.
    fn flat(value: u8, w: u32, h: u32) -> Vec<u8> {
        [value, value, value, 255].repeat((w * h) as usize)
    }

    fn mean(rgba: &[u8]) -> f32 {
        let total: f32 = rgba.chunks_exact(4).map(|px| f32::from(px[0])).sum();
        total / (rgba.len() / 4) as f32
    }

    /// The undithered path takes a shortcut that is only sound because
    /// quantization without diffusion is a pure per-pixel function. Prove
    /// it: quantizing the whole page must equal quantizing it a row at a
    /// time, which cannot hold if any state crosses a pixel boundary.
    #[test]
    fn without_dithering_no_state_crosses_a_pixel() {
        let format = PixelFormat::Grey {
            levels: 16,
            dither: false,
        };
        // A ramp that is neither flat nor aligned to the 16 output steps,
        // so rounding lands both ways.
        let (w, h) = (17u32, 5u32);
        let page: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = ((i * 37) % 256) as u8;
                [v, v.wrapping_add(11), v.wrapping_sub(7), 255]
            })
            .collect();

        let mut whole = page.clone();
        quantize(&mut whole, w, h, format);

        let mut row_at_a_time = page.clone();
        for row in row_at_a_time.chunks_exact_mut((w * 4) as usize) {
            quantize(row, w, 1, format);
        }

        assert_eq!(whole, row_at_a_time);
    }

    #[test]
    fn rgba_is_untouched() {
        let mut page = flat(120, 4, 4);
        let before = page.clone();
        quantize(&mut page, 4, 4, PixelFormat::Rgba);
        assert_eq!(page, before);
    }

    #[test]
    fn one_bit_leaves_only_black_and_white() {
        // A horizontal ramp, so every level is represented.
        let mut page: Vec<u8> = (0..32 * 32)
            .flat_map(|i| {
                let v = ((i % 32) * 8) as u8;
                [v, v, v, 255]
            })
            .collect();
        quantize(
            &mut page,
            32,
            32,
            PixelFormat::Grey {
                levels: 2,
                dither: true,
            },
        );
        for px in page.chunks_exact(4) {
            assert!(px[0] == 0 || px[0] == 255, "not 1-bit: {}", px[0]);
            assert_eq!((px[1], px[2]), (px[0], px[0]), "grey");
            assert_eq!(px[3], 255, "alpha survives");
        }
    }

    #[test]
    fn dithering_holds_the_average_a_flat_quantization_would_lose() {
        // Mid-grey is exactly between the two 1-bit levels: without
        // dithering it collapses to one of them; with it, the error
        // diffuses into a mix that averages back to the original.
        let mut flat_result = flat(128, 64, 64);
        quantize(
            &mut flat_result,
            64,
            64,
            PixelFormat::Grey {
                levels: 2,
                dither: false,
            },
        );
        let m = mean(&flat_result);
        assert!(m == 0.0 || m == 255.0, "undithered mid-grey collapses");

        let mut dithered = flat(128, 64, 64);
        quantize(
            &mut dithered,
            64,
            64,
            PixelFormat::Grey {
                levels: 2,
                dither: true,
            },
        );
        assert!(
            (mean(&dithered) - 128.0).abs() < 12.0,
            "dithered mean drifted: {}",
            mean(&dithered)
        );
    }

    #[test]
    fn sixteen_levels_land_on_the_steps() {
        let mut page: Vec<u8> = (0..16 * 16)
            .flat_map(|i| {
                let v = (i % 256) as u8;
                [v, v, v, 255]
            })
            .collect();
        quantize(
            &mut page,
            16,
            16,
            PixelFormat::Grey {
                levels: 16,
                dither: true,
            },
        );
        let step = 255.0 / 15.0;
        for px in page.chunks_exact(4) {
            let level = f32::from(px[0]) / step;
            assert!(
                (level - level.round()).abs() < 0.01,
                "{} is not a 16-level step",
                px[0]
            );
        }
    }

    #[test]
    fn a_panel_deeper_than_sixteen_levels_is_honoured() {
        // The cap used to be 16, applied with a silent clamp, so a deeper
        // request was quietly rewritten into a shallower picture.
        let ramp = |levels: u8| {
            let mut page: Vec<u8> = (0..256u32)
                .flat_map(|i| {
                    let v = i as u8;
                    [v, v, v, 255]
                })
                .collect();
            quantize(
                &mut page,
                256,
                1,
                PixelFormat::Grey {
                    levels,
                    dither: false,
                },
            );
            let mut seen: Vec<u8> = page.chunks_exact(4).map(|px| px[0]).collect();
            seen.sort_unstable();
            seen.dedup();
            seen.len()
        };
        assert_eq!(ramp(16), 16);
        assert_eq!(ramp(64), 64, "a 64-level panel was flattened");
        assert_eq!(ramp(255), 255);
    }

    #[test]
    fn fewer_than_two_levels_is_still_a_picture() {
        // The floor is arithmetic, not policy: `levels - 1` divides.
        for levels in [0, 1, 2] {
            let mut page = flat(200, 4, 4);
            quantize(
                &mut page,
                4,
                4,
                PixelFormat::Grey {
                    levels,
                    dither: false,
                },
            );
            assert!(
                page.chunks_exact(4).all(|px| px[0] == 0 || px[0] == 255),
                "levels {levels} produced something other than black and white"
            );
        }
    }

    /// A 2x1 page: red at (0,0), blue at (1,0).
    fn pair() -> Vec<u8> {
        vec![255, 0, 0, 255, 0, 0, 255, 255]
    }

    fn at(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn a_quarter_turn_swaps_the_axes() {
        let page = pair();
        let turned = rotate(&page, 2, 1, Rotation::Quarter);
        // Clockwise: the left pixel goes to the top of a 1x2 page.
        assert_eq!(at(&turned, 1, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&turned, 1, 0, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn a_half_turn_keeps_the_shape_and_reverses_it() {
        let page = pair();
        let turned = rotate(&page, 2, 1, Rotation::Half);
        assert_eq!(at(&turned, 2, 0, 0), [0, 0, 255, 255]);
        assert_eq!(at(&turned, 2, 1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn four_quarter_turns_come_back() {
        let original = pair();
        let mut turned = original.clone();
        let (mut w, mut h) = (2u32, 1u32);
        for _ in 0..4 {
            turned = rotate(&turned, w, h, Rotation::Quarter).into_owned();
            std::mem::swap(&mut w, &mut h);
        }
        assert_eq!(turned, original);
    }

    #[test]
    fn none_hands_back_the_same_pixels_without_copying() {
        let original = pair();
        let turned = rotate(&original, 2, 1, Rotation::None);
        assert_eq!(turned.as_ref(), original.as_slice());
        // And it is the caller's own buffer, not a duplicate of it: an
        // unrotated shell allocates nothing per frame here.
        assert!(matches!(turned, Cow::Borrowed(_)));
        assert!(std::ptr::eq(turned.as_ref().as_ptr(), original.as_ptr()));
    }

    #[test]
    fn a_turn_owns_what_it_builds() {
        let original = pair();
        let turned = rotate(&original, 2, 1, Rotation::Quarter);
        assert!(matches!(turned, Cow::Owned(_)));
    }
}
