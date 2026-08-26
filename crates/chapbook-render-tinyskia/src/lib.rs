//! CPU rasterization of chapbook display lists into `tiny_skia::Pixmap`s.
//!
//! Glyphs rasterize through cosmic-text's `SwashCache` (alpha masks blended
//! at subpixel-binned positions; color glyphs — emoji — composited as-is).
//! All layout is CSS px; the `scale` factor (hidpi) is applied here and only
//! here, by scaling positions and rasterizing glyphs at `font_size * scale`.

use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache};
use tiny_skia::{Pixmap, PixmapMut, PixmapPaint, PremultipliedColorU8};

// Re-exported so consumers (CLI, viewer) create pixmaps without a direct
// tiny-skia dependency.
pub use tiny_skia;

use chapbook_core::{Rect, Rgba};
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
    ///
    /// Takes a [`PixmapMut`] rather than an owned `Pixmap` so the target
    /// can be memory this crate did not allocate — an Android bitmap's
    /// locked pixels, a `CGBitmapContext`, the buffer behind a WASM
    /// `ImageData`. An owned pixmap still works: pass `pixmap.as_mut()`.
    pub fn render(
        &mut self,
        dl: &DisplayList,
        fonts: &mut FontSystem,
        images: &ImageStore,
        scale: f32,
        pixmap: &mut PixmapMut<'_>,
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
                        // Baselines snap to whole device pixels, as text
                        // rasterizers conventionally do: it keeps stems and
                        // baselines aligned across a line. It is also the
                        // only correct option here — swash renders the
                        // sub-pixel offset from cosmic-text's vertical bin
                        // in the opposite direction, so passing the true
                        // fraction lifts every line up to ~1.6px above the
                        // baseline the layout computed. Horizontal
                        // sub-pixel positioning is kept: that is where it
                        // buys even spacing.
                        let py = ((origin.y + glyph.y) * scale).round();
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
        pixmap: &mut PixmapMut<'_>,
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

fn fill_rect(pixmap: &mut PixmapMut<'_>, rect: &Rect, color: Rgba, scale: f32) {
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

fn draw_image(
    pixmap: &mut PixmapMut<'_>,
    images: &ImageStore,
    resource: u64,
    dest: &Rect,
    scale: f32,
) {
    let Some(stored) = images.get(resource) else {
        return;
    };
    // Premultiply straight RGBA for tiny-skia.
    let mut data = stored.rgba.clone();
    for px in data.as_chunks_mut::<4>().0 {
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
