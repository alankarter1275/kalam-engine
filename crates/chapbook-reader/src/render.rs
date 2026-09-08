//! Rasterizing the current page with the bundled CPU backend: into a
//! caller-owned buffer or a fresh pixmap, panel rotation and pixel
//! format applied.

use chapbook_core::{PanelRect, PixelFormat, Rotation};
use chapbook_render_tinyskia::tiny_skia;

use crate::Session;

impl Session {
    /// Convert rendered pages for a panel that can't show full color —
    /// 16-level grey or 1-bit e-ink. [`Session::render`] applies it; a
    /// shell rasterizing a frame itself calls
    /// `mezzotint::encode::quantize_for` at the same point — panel policy
    /// belongs to the target, not to whichever rasterizer produced the
    /// pixels.
    ///
    /// This changes pixels, not ops, so it does not disturb the frame
    /// record: the display list is identical either way.
    pub fn set_pixel_format(&mut self, format: PixelFormat) {
        self.pixel_format = format;
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// The device-pixel size [`render`](Self::render) and
    /// [`render_into`](Self::render_into) produce, rotation included.
    ///
    /// A host allocating its own surface needs this before it can ask for
    /// one; `None` until metrics are set.
    pub fn render_size(&self) -> Option<(u32, u32)> {
        let metrics = self.metrics?;
        let scale = metrics.dpi_scale;
        let (w, h) = (
            (metrics.size.w * scale) as u32,
            (metrics.size.h * scale) as u32,
        );
        Some(if metrics.rotation.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        })
    }

    /// Rasterize the current page straight into a buffer the caller owns.
    ///
    /// Returns whether anything was drawn: `false` for a size that does not
    /// match [`render_size`](Self::render_size), a `stride` narrower than a
    /// row, a buffer too short, or nothing to render.
    ///
    /// This exists because every host platform already owns the memory it
    /// wants the page in — Android's `AndroidBitmap_lockPixels` hands back
    /// the bitmap's own backing store, iOS has a `CGBitmapContext`, a WASM
    /// build has the `Uint8ClampedArray` behind an `ImageData` — and
    /// [`render`](Self::render) allocating a fresh `Pixmap` per page turn
    /// meant a copy into that memory on every one.
    ///
    /// Pixels are premultiplied RGBA8888, which is what
    /// `Bitmap.Config.ARGB_8888` holds natively, so the common Android path
    /// is a straight rasterize with no conversion at either end.
    ///
    /// **What is actually free.** An unrotated page whose `stride` is
    /// exactly `width * 4` is rasterized directly into `dst`: no
    /// allocation, no copy. A rotated page, or a padded stride, still goes
    /// through an intermediate — rotation cannot be done in place and
    /// tiny-skia cannot target a strided buffer — and is copied row by row.
    /// Both are correct; only the first is free, and it is the one every
    /// named platform hits.
    pub fn render_into(&mut self, dst: &mut [u8], width: u32, height: u32, stride: usize) -> bool {
        let Some((want_w, want_h)) = self.render_size() else {
            return false;
        };
        if width != want_w || height != want_h {
            return false;
        }
        let row = match (width as usize).checked_mul(4) {
            Some(row) if stride >= row => row,
            _ => return false,
        };
        if dst.len() < stride.saturating_mul(height as usize) {
            return false;
        }

        let Some(metrics) = self.metrics else {
            return false;
        };
        // The free path: the engine's output and the host's buffer are the
        // same shape, so rasterize into it and stop.
        if metrics.rotation == Rotation::None && stride == row {
            let Some(mut target) = tiny_skia::PixmapMut::from_bytes(dst, width, height) else {
                return false;
            };
            return self.render_page_into(&mut target).is_some();
        }

        let Some(pixmap) = self.render() else {
            return false;
        };
        for (y, line) in pixmap.data().chunks_exact(row).enumerate() {
            let at = y * stride;
            dst[at..at + row].copy_from_slice(line);
        }
        true
    }

    /// Rasterize and quantize into `target`, which must already be the
    /// unrotated page size. Shared by both entry points.
    fn render_page_into(&mut self, target: &mut tiny_skia::PixmapMut<'_>) -> Option<()> {
        let metrics = self.metrics?;
        let dl = self.frame()?.list;
        let spine = self.spine;
        let scale = metrics.dpi_scale;
        // `render_size` derives from the metrics and this derives from the
        // display list. They agree — the page is laid out to the metrics —
        // but a caller's buffer is sized from the first and rasterized
        // against the second, so disagreeing must fail rather than
        // misdraw. `render_size_matches_what_render_produces` is the test
        // that keeps them honest.
        if target.width() != (dl.size.w * scale) as u32
            || target.height() != (dl.size.h * scale) as u32
        {
            return None;
        }
        // Field accesses rather than `unit_images`: the renderer borrows
        // `self.fonts` mutably alongside this, so the compiler must see
        // disjoint fields, which a method call would hide.
        let images = self
            .units
            .get(&spine)
            .and_then(|unit| unit.images.as_ref())
            .unwrap_or(&self.empty_images);
        self.renderer
            .render(&dl, &mut self.fonts, images, scale, target);
        // Panel policy is backend-neutral: the same conversion a GPU shell
        // would apply to its own pixels. Dithering is scoped to where the
        // page has images, because diffusing error through body text
        // stipples every glyph's edge and not diffusing it through a
        // photograph flattens the photograph.
        let (w, h) = (target.width(), target.height());
        let dithered = dl.dither_regions(scale);
        let format = self.pixel_format;
        mezzotint::encode::quantize_for(
            target.data_mut(),
            w,
            h,
            PanelRect::full(w, h),
            format,
            &dithered,
        );
        Some(())
    }

    /// Rasterize the current page at the current metrics with the bundled
    /// CPU backend — the convenience path over [`Session::frame`]. The
    /// result is in panel orientation and panel color: the metrics'
    /// rotation and the session's pixel format are both applied here.
    /// `None` under the same conditions as a frame.
    pub fn render(&mut self) -> Option<tiny_skia::Pixmap> {
        let metrics = self.metrics?;
        // Allocate the unrotated shape and let the shared path fill it —
        // one rasterize-and-quantize path, however the pixels leave.
        // `render_size` is panel-oriented, so undo the axis swap here.
        let (w, h) = self.render_size()?;
        let (uw, uh) = if metrics.rotation.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        };
        let mut pixmap = tiny_skia::Pixmap::new(uw, uh)?;
        self.render_page_into(&mut pixmap.as_mut())?;
        if metrics.rotation == Rotation::None {
            return Some(pixmap);
        }
        let turned = chapbook_paint::rotate(pixmap.data(), uw, uh, metrics.rotation);
        tiny_skia::Pixmap::from_vec(turned.into_owned(), tiny_skia::IntSize::from_wh(w, h)?)
    }
}
