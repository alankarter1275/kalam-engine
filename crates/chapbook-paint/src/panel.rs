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

use chapbook_core::{PixelFormat, Rotation};

/// Convert a rasterized page for a panel that cannot show full color.
///
/// Luminance is Rec. 709 over the sRGB values, quantized to the format's
/// levels; with `dither` on, the quantization error is diffused
/// Floyd–Steinberg so a 16-level panel still shows a gradient instead of
/// banding. Alpha is untouched, and [`PixelFormat::Rgba`] is a no-op.
pub fn quantize(rgba: &mut [u8], width: u32, height: u32, format: PixelFormat) {
    let PixelFormat::Grey { levels, dither } = format else {
        return;
    };
    let levels = f32::from(levels.clamp(2, 16));
    let (w, h) = (width as usize, height as usize);

    // One row of forward error plus the next, so diffusion needs no full
    // second buffer.
    let mut error = vec![0.0f32; w + 2];
    let mut next = vec![0.0f32; w + 2];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b) = (
                f32::from(rgba[i]),
                f32::from(rgba[i + 1]),
                f32::from(rgba[i + 2]),
            );
            let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b + error[x + 1];
            let step = 255.0 / (levels - 1.0);
            let quantized = (lum / step).round().clamp(0.0, levels - 1.0) * step;
            let value = quantized as u8;
            rgba[i] = value;
            rgba[i + 1] = value;
            rgba[i + 2] = value;
            if !dither {
                continue;
            }
            // Floyd–Steinberg: 7/16 right, 3/16 down-left, 5/16 down,
            // 1/16 down-right.
            let residual = lum - quantized;
            error[x + 2] += residual * 7.0 / 16.0;
            next[x] += residual * 3.0 / 16.0;
            next[x + 1] += residual * 5.0 / 16.0;
            next[x + 2] += residual / 16.0;
        }
        std::mem::swap(&mut error, &mut next);
        next.iter_mut().for_each(|e| *e = 0.0);
    }
}

/// Turn a rasterized page for a panel mounted in a different orientation,
/// clockwise. Quarter turns swap the page's dimensions, so the returned
/// rows are `height` wide; [`Rotation::None`] copies.
///
/// The inverse for input is [`chapbook_core::PageMetrics::panel_to_page`],
/// so a shell that turns its output here reads pointer coordinates back
/// through that.
pub fn rotate(rgba: &[u8], width: u32, height: u32, rotation: Rotation) -> Vec<u8> {
    if rotation == Rotation::None {
        return rgba.to_vec();
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
    out
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
        let turned = rotate(&pair(), 2, 1, Rotation::Quarter);
        // Clockwise: the left pixel goes to the top of a 1x2 page.
        assert_eq!(at(&turned, 1, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&turned, 1, 0, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn a_half_turn_keeps_the_shape_and_reverses_it() {
        let turned = rotate(&pair(), 2, 1, Rotation::Half);
        assert_eq!(at(&turned, 2, 0, 0), [0, 0, 255, 255]);
        assert_eq!(at(&turned, 2, 1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn four_quarter_turns_come_back() {
        let original = pair();
        let mut turned = original.clone();
        let (mut w, mut h) = (2u32, 1u32);
        for _ in 0..4 {
            turned = rotate(&turned, w, h, Rotation::Quarter);
            std::mem::swap(&mut w, &mut h);
        }
        assert_eq!(turned, original);
    }

    #[test]
    fn none_is_a_copy() {
        let original = pair();
        assert_eq!(rotate(&original, 2, 1, Rotation::None), original);
    }
}
