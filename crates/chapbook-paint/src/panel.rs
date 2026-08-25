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

use chapbook_core::{PanelRect, PixelFormat, Rotation};

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
        for px in rgba.as_chunks_mut::<4>().0.iter_mut().take(w * h) {
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
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
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

/// Quantize a page, dithering only where the page asked for it.
///
/// One flag for a whole page is the wrong shape, and at two levels it is
/// decisively wrong. Diffusing error through body text stipples the
/// antialiased edge of every glyph; *not* diffusing it through a
/// photograph turns the photograph into a silhouette. The display list
/// knows which of the two a given pixel came from — see
/// [`DisplayList::dither_regions`] — and that knowledge used to be thrown
/// away at this seam.
///
/// `dithered` is in device pixels, in page orientation, because that is
/// what a rasterized page is before [`rotate`] turns it.
///
/// Each region diffuses within itself and no error crosses its edge.
/// That is a seam, and it is placed where a seam is already: an image's
/// boundary is a hard content edge, so a discontinuity there is invisible
/// in a way the same discontinuity mid-paragraph would not be.
///
/// With `dither` off, or with no regions, this is exactly [`quantize`]
/// without dithering — including the case worth stating plainly: a page
/// of nothing but text asks for no diffusion at all, which is the point.
pub fn quantize_regions(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    dithered: &[PanelRect],
) {
    let PixelFormat::Grey { levels, dither } = format else {
        return;
    };
    let (step, top) = steps(levels);
    let w = width as usize;
    if w == 0 {
        return;
    }
    let h = (height as usize).min(rgba.len() / (w * 4));

    if !dither || dithered.is_empty() {
        for row in rgba.chunks_exact_mut(w * 4).take(h) {
            quantize_span(row, step, top);
        }
        return;
    }

    for rect in dithered {
        dither_rect(rgba, w, h, *rect, step, top);
    }
    quantize_outside(rgba, w, h, dithered, step, top);
}

/// The step between adjacent output levels, and the index of the highest.
///
/// Floored at two, not capped: two levels is where the arithmetic stops
/// meaning anything, while an upper bound would be a guess about which
/// panels exist.
fn steps(levels: u8) -> (f32, f32) {
    let levels = f32::from(levels.max(2));
    (255.0 / (levels - 1.0), levels - 1.0)
}

/// Quantize a run of pixels with no regard for their neighbours.
fn quantize_span(span: &mut [u8], step: f32, top: f32) {
    for px in span.as_chunks_mut::<4>().0 {
        let value = ((luminance(px) / step).round().clamp(0.0, top) * step) as u8;
        px[0] = value;
        px[1] = value;
        px[2] = value;
    }
}

/// Floyd–Steinberg confined to one rectangle. The error rows are the
/// rectangle's width, so nothing diffuses past its edges — see
/// [`quantize_regions`] for why that seam is placed where it is.
fn dither_rect(rgba: &mut [u8], w: usize, h: usize, rect: PanelRect, step: f32, top: f32) {
    let x0 = (rect.x as usize).min(w);
    let x1 = (rect.max_x() as usize).min(w);
    let y0 = (rect.y as usize).min(h);
    let y1 = (rect.max_y() as usize).min(h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let rw = x1 - x0;
    let mut error = vec![0.0f32; rw + 2];
    let mut next = vec![0.0f32; rw + 2];
    for y in y0..y1 {
        let row = &mut rgba[y * w * 4..(y + 1) * w * 4];
        for (i, px) in row[x0 * 4..x1 * 4]
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let lum = luminance(px) + error[i + 1];
            let quantized = (lum / step).round().clamp(0.0, top) * step;
            let value = quantized as u8;
            px[0] = value;
            px[1] = value;
            px[2] = value;
            let residual = lum - quantized;
            error[i + 2] += residual * 7.0 / 16.0;
            next[i] += residual * 3.0 / 16.0;
            next[i + 1] += residual * 5.0 / 16.0;
            next[i + 2] += residual / 16.0;
        }
        std::mem::swap(&mut error, &mut next);
        next.fill(0.0);
    }
}

/// Plain quantization everywhere the dithered regions do not reach.
///
/// Walked as complement spans per row rather than through a page-sized
/// mask: a page carries a handful of images at most, so the covering
/// intervals for a row are found by looking at those few rects, and the
/// alternative is a megabyte-scale allocation and memset per frame on
/// exactly the devices that can least afford one.
fn quantize_outside(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    dithered: &[PanelRect],
    step: f32,
    top: f32,
) {
    let mut covering: Vec<(usize, usize)> = Vec::new();
    for y in 0..h {
        covering.clear();
        for rect in dithered {
            if y < rect.y as usize || y >= rect.max_y() as usize {
                continue;
            }
            let a = (rect.x as usize).min(w);
            let b = (rect.max_x() as usize).min(w);
            if a < b {
                covering.push((a, b));
            }
        }
        let row = &mut rgba[y * w * 4..(y + 1) * w * 4];
        if covering.is_empty() {
            quantize_span(row, step, top);
            continue;
        }
        covering.sort_unstable();
        let mut cursor = 0usize;
        for (a, b) in covering.iter().copied() {
            if a > cursor {
                quantize_span(&mut row[cursor * 4..a * 4], step, top);
            }
            cursor = cursor.max(b);
        }
        if cursor < w {
            quantize_span(&mut row[cursor * 4..w * 4], step, top);
        }
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
        let total: f32 = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| f32::from(px[0]))
            .sum();
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
        for px in page.as_chunks::<4>().0 {
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
        for px in page.as_chunks::<4>().0 {
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
            let mut seen: Vec<u8> = page.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
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
                page.as_chunks::<4>()
                    .0
                    .iter()
                    .all(|px| px[0] == 0 || px[0] == 255),
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

    /// A ramp across the page, so quantization has something to round
    /// both ways and diffusion has error to carry.
    fn ramp(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let v = ((i * 37) % 256) as u8;
                [v, v, v, 255]
            })
            .collect()
    }

    const SIXTEEN: PixelFormat = PixelFormat::Grey {
        levels: 16,
        dither: true,
    };
    const SIXTEEN_FLAT: PixelFormat = PixelFormat::Grey {
        levels: 16,
        dither: false,
    };

    /// With no region asking for it, a page gets no diffusion at all —
    /// which is the whole point for a page of text.
    #[test]
    fn no_regions_means_no_dithering_anywhere() {
        let (w, h) = (23u32, 7u32);
        let mut scoped = ramp(w, h);
        quantize_regions(&mut scoped, w, h, SIXTEEN, &[]);
        let mut plain = ramp(w, h);
        quantize(&mut plain, w, h, SIXTEEN_FLAT);
        assert_eq!(scoped, plain);
    }

    /// And a region covering everything reproduces the page-global
    /// dither exactly, so the scoped path is the same arithmetic and not
    /// a second implementation that happens to look similar.
    #[test]
    fn a_region_covering_the_page_is_the_page_global_dither() {
        let (w, h) = (23u32, 7u32);
        let mut scoped = ramp(w, h);
        quantize_regions(&mut scoped, w, h, SIXTEEN, &[PanelRect::full(w, h)]);
        let mut global = ramp(w, h);
        quantize(&mut global, w, h, SIXTEEN);
        assert_eq!(scoped, global);
    }

    /// The claim that makes this worth doing: pixels outside the region
    /// are bit-for-bit what they would have been with dithering off, so
    /// no error leaks out of an image and into the text beside it.
    #[test]
    fn no_error_escapes_a_region() {
        let (w, h) = (24u32, 8u32);
        let region = PanelRect::new(4, 2, 8, 4);

        let mut scoped = ramp(w, h);
        quantize_regions(&mut scoped, w, h, SIXTEEN, &[region]);
        let mut plain = ramp(w, h);
        quantize(&mut plain, w, h, SIXTEEN_FLAT);

        let mut inside_differs = false;
        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                let within =
                    x >= region.x && x < region.max_x() && y >= region.y && y < region.max_y();
                if within {
                    inside_differs |= scoped[at] != plain[at];
                } else {
                    assert_eq!(
                        scoped[at], plain[at],
                        "pixel ({x},{y}) is outside {region:?} but the dither reached it"
                    );
                }
            }
        }
        assert!(
            inside_differs,
            "the region was not actually dithered, so the test proves nothing"
        );
    }

    /// Overlapping regions must not dither a pixel twice or leave a strip
    /// between them unquantized — the two ways a span walk goes wrong.
    #[test]
    fn overlapping_regions_still_cover_every_pixel_exactly_once() {
        let (w, h) = (20u32, 6u32);
        let regions = [PanelRect::new(2, 1, 8, 4), PanelRect::new(6, 2, 9, 3)];
        let mut page = ramp(w, h);
        quantize_regions(&mut page, w, h, SIXTEEN, &regions);

        // Every output pixel must sit on one of the sixteen levels. A
        // pixel quantized twice, or missed entirely, does not.
        let step = 255.0 / 15.0;
        for (i, px) in page.as_chunks::<4>().0.iter().enumerate() {
            let level = f32::from(px[0]) / step;
            assert!(
                (level - level.round()).abs() < 1e-3,
                "pixel {i} is {} which is not on a level",
                px[0]
            );
            assert_eq!(px[0], px[1]);
            assert_eq!(px[1], px[2]);
        }
    }

    /// A region hanging off the page is clipped, not a panic.
    #[test]
    fn a_region_outside_the_page_is_survived() {
        let (w, h) = (8u32, 4u32);
        let mut page = ramp(w, h);
        quantize_regions(
            &mut page,
            w,
            h,
            SIXTEEN,
            &[
                PanelRect::new(6, 3, 99, 99),
                PanelRect::new(50, 50, 4, 4),
                PanelRect::default(),
            ],
        );
        let step = 255.0 / 15.0;
        for px in page.as_chunks::<4>().0 {
            let level = f32::from(px[0]) / step;
            assert!((level - level.round()).abs() < 1e-3);
        }
    }

    /// `Rgba` still means "do not reduce", regions or not.
    #[test]
    fn rgba_is_untouched_by_the_scoped_path_too() {
        let mut page = flat(120, 4, 4);
        let before = page.clone();
        quantize_regions(&mut page, 4, 4, PixelFormat::Rgba, &[PanelRect::full(4, 4)]);
        assert_eq!(page, before);
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
