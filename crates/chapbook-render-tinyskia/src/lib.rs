//! CPU rasterization of chapbook display lists into `tiny_skia::Pixmap`s.
//!
//! Glyphs rasterize through cosmic-text's `SwashCache` (alpha masks blended
//! at subpixel-binned positions; color glyphs — emoji — composited as-is).
//! All layout is CSS px; the `scale` factor (hidpi) is applied here and only
//! here, by scaling positions and rasterizing glyphs at `font_size * scale`.

use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache};
use tiny_skia::{Pixmap, PixmapPaint, PremultipliedColorU8};

// Re-exported so consumers (CLI, viewer) create pixmaps without a direct
// tiny-skia dependency.
pub use tiny_skia;

use chapbook_core::{PixelFormat, Rect, Rgba, Rotation};
use chapbook_paint::{DisplayList, DisplayOp, ImageStore};

pub struct Renderer {
    swash: SwashCache,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Renderer {
            swash: SwashCache::new(),
        }
    }

    /// Rasterize `dl` into `pixmap` at `scale` device pixels per CSS px.
    /// The pixmap should be at least `dl.size * scale` pixels. `images`
    /// resolves `DisplayOp::Image` resources; pass an empty store for
    /// text-only content.
    pub fn render(
        &mut self,
        dl: &DisplayList,
        fonts: &mut FontSystem,
        images: &ImageStore,
        scale: f32,
        pixmap: &mut Pixmap,
    ) {
        for op in &dl.ops {
            match op {
                DisplayOp::FillRect { rect, color } => {
                    fill_rect(pixmap, rect, *color, scale);
                }
                DisplayOp::GlyphRun {
                    font,
                    font_size,
                    font_weight,
                    color,
                    origin,
                    glyphs,
                } => {
                    for glyph in glyphs {
                        let px = (origin.x + glyph.x) * scale;
                        let py = (origin.y + glyph.y) * scale;
                        let (key, xi, yi) = CacheKey::new(
                            *font,
                            glyph.id,
                            font_size * scale,
                            (px, py),
                            cosmic_text::fontdb::Weight(*font_weight),
                            CacheKeyFlags::empty(),
                        );
                        self.blend_glyph(fonts, key, xi, yi, *color, pixmap);
                    }
                }
                DisplayOp::Image { resource, dest } => {
                    draw_image(pixmap, images, *resource, dest, scale);
                }
            }
        }
    }

    fn blend_glyph(
        &mut self,
        fonts: &mut FontSystem,
        key: CacheKey,
        x: i32,
        y: i32,
        color: Rgba,
        pixmap: &mut Pixmap,
    ) {
        use cosmic_text::SwashContent;

        let Some(image) = self.swash.get_image(fonts, key).as_ref() else {
            return;
        };
        let left = x + image.placement.left;
        let top = y - image.placement.top;
        let width = image.placement.width as i32;
        let height = image.placement.height as i32;
        let pm_w = pixmap.width() as i32;
        let pm_h = pixmap.height() as i32;

        match image.content {
            SwashContent::Mask => {
                let pixels = pixmap.pixels_mut();
                let mut i = 0usize;
                for off_y in 0..height {
                    let ty = top + off_y;
                    for off_x in 0..width {
                        let tx = left + off_x;
                        let mask = image.data[i];
                        i += 1;
                        if mask == 0 || tx < 0 || ty < 0 || tx >= pm_w || ty >= pm_h {
                            continue;
                        }
                        let alpha = (u16::from(mask) * u16::from(color.a) / 255) as u8;
                        if alpha == 0 {
                            continue;
                        }
                        let idx = (ty * pm_w + tx) as usize;
                        pixels[idx] = blend_over(pixels[idx], color, alpha);
                    }
                }
            }
            SwashContent::Color => {
                // Color glyph (emoji): an RGBA image positioned like a mask.
                if let Some(glyph_pixmap) = Pixmap::from_vec(
                    image.data.clone(),
                    tiny_skia::IntSize::from_wh(
                        image.placement.width.max(1),
                        image.placement.height.max(1),
                    )
                    .unwrap(),
                ) {
                    pixmap.draw_pixmap(
                        left,
                        top,
                        glyph_pixmap.as_ref(),
                        &PixmapPaint::default(),
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
            }
            SwashContent::SubpixelMask => {
                // Not produced by swash today; ignore rather than mis-render.
            }
        }
    }
}

/// Source-over blend of `color` at `alpha` onto a premultiplied pixel.
fn blend_over(dst: PremultipliedColorU8, color: Rgba, alpha: u8) -> PremultipliedColorU8 {
    let a = u32::from(alpha);
    let inv = 255 - a;
    let sr = u32::from(color.r) * a / 255;
    let sg = u32::from(color.g) * a / 255;
    let sb = u32::from(color.b) * a / 255;
    let r = (sr + u32::from(dst.red()) * inv / 255).min(255) as u8;
    let g = (sg + u32::from(dst.green()) * inv / 255).min(255) as u8;
    let b = (sb + u32::from(dst.blue()) * inv / 255).min(255) as u8;
    let out_a = (a + u32::from(dst.alpha()) * inv / 255).min(255) as u8;
    PremultipliedColorU8::from_rgba(r.min(out_a), g.min(out_a), b.min(out_a), out_a).unwrap_or(dst)
}

fn fill_rect(pixmap: &mut Pixmap, rect: &Rect, color: Rgba, scale: f32) {
    if color.is_transparent() {
        return;
    }
    let Some(r) = tiny_skia::Rect::from_xywh(
        rect.origin.x * scale,
        rect.origin.y * scale,
        rect.size.w * scale,
        rect.size.h * scale,
    ) else {
        return;
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    pixmap.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
}

fn draw_image(pixmap: &mut Pixmap, images: &ImageStore, resource: u64, dest: &Rect, scale: f32) {
    let Some(stored) = images.get(resource) else {
        return;
    };
    // Premultiply straight RGBA for tiny-skia.
    let mut data = stored.rgba.clone();
    for px in data.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        px[0] = (u16::from(px[0]) * a / 255) as u8;
        px[1] = (u16::from(px[1]) * a / 255) as u8;
        px[2] = (u16::from(px[2]) * a / 255) as u8;
    }
    let Some(size) = tiny_skia::IntSize::from_wh(stored.width, stored.height) else {
        return;
    };
    let Some(src) = Pixmap::from_vec(data, size) else {
        return;
    };
    let sx = dest.size.w * scale / stored.width as f32;
    let sy = dest.size.h * scale / stored.height as f32;
    let transform = tiny_skia::Transform::from_scale(sx, sy)
        .post_translate(dest.origin.x * scale, dest.origin.y * scale);
    let paint = PixmapPaint {
        quality: tiny_skia::FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    // draw_pixmap positions via the transform; the x/y args stay zero.
    pixmap.draw_pixmap(0, 0, src.as_ref(), &paint, transform, None);
}

/// Convert a rasterized page for a panel that cannot show full color.
///
/// Luminance is Rec. 709 over the sRGB values, quantized to the format's
/// levels; with `dither` on, the quantization error is diffused
/// Floyd–Steinberg so a 16-level panel still shows a gradient instead of
/// banding. Alpha is untouched, and [`PixelFormat::Rgba`] is a no-op.
///
/// Pages paint over an opaque background, so premultiplied and straight
/// values coincide here; this operates on the stored premultiplied bytes
/// directly.
pub fn quantize(pixmap: &mut Pixmap, format: PixelFormat) {
    let PixelFormat::Grey { levels, dither } = format else {
        return;
    };
    let levels = f32::from(levels.clamp(2, 16));
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let data = pixmap.data_mut();

    // One row of forward error plus the next, so diffusion needs no full
    // second buffer.
    let mut error = vec![0.0f32; w + 2];
    let mut next = vec![0.0f32; w + 2];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b) = (
                f32::from(data[i]),
                f32::from(data[i + 1]),
                f32::from(data[i + 2]),
            );
            let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b + error[x + 1];
            let step = 255.0 / (levels - 1.0);
            let quantized = (lum / step).round().clamp(0.0, levels - 1.0) * step;
            let value = quantized as u8;
            data[i] = value;
            data[i + 1] = value;
            data[i + 2] = value;
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
/// clockwise. Quarter turns swap the pixmap's dimensions;
/// [`Rotation::None`] copies. `None` only if the turned size is degenerate.
///
/// The inverse for input is [`chapbook_core::PageMetrics::panel_to_page`],
/// so a shell that turns its output here reads pointer coordinates back
/// through that.
pub fn rotate(pixmap: &Pixmap, rotation: Rotation) -> Option<Pixmap> {
    if rotation == Rotation::None {
        return Some(pixmap.clone());
    }
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let (dw, dh) = if rotation.swaps_axes() {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = Pixmap::new(dw as u32, dh as u32)?;
    let src = pixmap.data();
    let dst = out.data_mut();
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = match rotation {
                Rotation::None => (x, y),
                Rotation::Quarter => (h - 1 - y, x),
                Rotation::Half => (w - 1 - x, h - 1 - y),
                Rotation::ThreeQuarter => (y, w - 1 - x),
            };
            let (from, to) = ((y * w + x) * 4, (dy * dw + dx) * 4);
            dst[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pixmap filled with one opaque grey level.
    fn flat(value: u8, w: u32, h: u32) -> Pixmap {
        let mut pixmap = Pixmap::new(w, h).unwrap();
        for px in pixmap.data_mut().chunks_exact_mut(4) {
            px.copy_from_slice(&[value, value, value, 255]);
        }
        pixmap
    }

    fn mean(pixmap: &Pixmap) -> f32 {
        let data = pixmap.data();
        let total: f32 = data.chunks_exact(4).map(|px| f32::from(px[0])).sum();
        total / (data.len() / 4) as f32
    }

    #[test]
    fn rgba_is_untouched() {
        let mut pixmap = flat(120, 4, 4);
        let before = pixmap.data().to_vec();
        quantize(&mut pixmap, PixelFormat::Rgba);
        assert_eq!(pixmap.data(), &before[..]);
    }

    #[test]
    fn one_bit_leaves_only_black_and_white() {
        let mut pixmap = Pixmap::new(32, 32).unwrap();
        // A horizontal ramp, so every level is represented.
        for (i, px) in pixmap.data_mut().chunks_exact_mut(4).enumerate() {
            let v = ((i % 32) * 8) as u8;
            px.copy_from_slice(&[v, v, v, 255]);
        }
        quantize(
            &mut pixmap,
            PixelFormat::Grey {
                levels: 2,
                dither: true,
            },
        );
        for px in pixmap.data().chunks_exact(4) {
            assert!(px[0] == 0 || px[0] == 255, "not 1-bit: {}", px[0]);
            assert_eq!((px[0], px[1], px[2]), (px[0], px[0], px[0]), "grey");
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
            PixelFormat::Grey {
                levels: 2,
                dither: false,
            },
        );
        assert!(
            mean(&flat_result) == 0.0 || mean(&flat_result) == 255.0,
            "undithered mid-grey collapses"
        );

        let mut dithered = flat(128, 64, 64);
        quantize(
            &mut dithered,
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
        let mut pixmap = Pixmap::new(16, 16).unwrap();
        for (i, px) in pixmap.data_mut().chunks_exact_mut(4).enumerate() {
            let v = (i % 256) as u8;
            px.copy_from_slice(&[v, v, v, 255]);
        }
        quantize(
            &mut pixmap,
            PixelFormat::Grey {
                levels: 16,
                dither: true,
            },
        );
        let step = 255.0 / 15.0;
        for px in pixmap.data().chunks_exact(4) {
            let level = f32::from(px[0]) / step;
            assert!(
                (level - level.round()).abs() < 0.01,
                "{} is not a 16-level step",
                px[0]
            );
        }
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    /// A 2x1 pixmap: red at (0,0), blue at (1,0).
    fn pair() -> Pixmap {
        let mut pixmap = Pixmap::new(2, 1).unwrap();
        let data = pixmap.data_mut();
        data[0..4].copy_from_slice(&[255, 0, 0, 255]);
        data[4..8].copy_from_slice(&[0, 0, 255, 255]);
        pixmap
    }

    fn at(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * pixmap.width() + x) * 4) as usize;
        pixmap.data()[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn a_quarter_turn_swaps_the_axes() {
        let turned = rotate(&pair(), Rotation::Quarter).unwrap();
        assert_eq!((turned.width(), turned.height()), (1, 2));
        // Clockwise: the left pixel goes to the top.
        assert_eq!(at(&turned, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&turned, 0, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn a_half_turn_keeps_the_shape_and_reverses_it() {
        let turned = rotate(&pair(), Rotation::Half).unwrap();
        assert_eq!((turned.width(), turned.height()), (2, 1));
        assert_eq!(at(&turned, 0, 0), [0, 0, 255, 255]);
        assert_eq!(at(&turned, 1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn four_quarter_turns_come_back() {
        let original = pair();
        let mut turned = original.clone();
        for _ in 0..4 {
            turned = rotate(&turned, Rotation::Quarter).unwrap();
        }
        assert_eq!(turned.data(), original.data());
    }

    #[test]
    fn none_is_a_copy() {
        let original = pair();
        assert_eq!(
            rotate(&original, Rotation::None).unwrap().data(),
            original.data()
        );
    }
}
