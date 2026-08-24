//! PDF text-layer extraction: a hayro-interpret `Device` that ignores all
//! painting and records where glyphs land.
//!
//! Each glyph event captures its baseline origin and approximate size from
//! the composed affine (glyph outlines live in a unit-em space, so the
//! transform's vertical magnitude *is* the rendered size) and its Unicode
//! text via hayro's ToUnicode/encoding fallback chain. Events are then
//! grouped into lines by baseline (tolerance = half the glyph size),
//! ordered top-to-bottom then left-to-right — PDF content streams can emit
//! text in any order, so reading order is reconstructed geometrically.
//! Glyph advances are estimated from the gap to the next glyph on the
//! line (PDFs don't hand the device an advance).
//!
//! Offsets index the page's extracted text in chars — the per-page locator
//! space image books otherwise leave degenerate.

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::page::Page;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    interpret_page, BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask,
};
use hayro::vello_cpu::kurbo::{Affine, BezPath, Point, Rect};

// The extracted line/glyph model lives in chapbook-core: a reading session
// holds a page's text layer whether or not this producer is compiled in.
pub use chapbook_core::{TextGlyph, TextLine};

struct GlyphEvent {
    x: f32,
    /// Baseline y (top-left origin space).
    y: f32,
    size: f32,
    text: String,
}

#[derive(Default)]
struct TextDevice {
    events: Vec<GlyphEvent>,
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _paint: &Paint<'a>,
        _draw_mode: &GlyphDrawMode,
    ) {
        let Some(unicode) = glyph.as_unicode() else {
            return;
        };
        let text = match unicode {
            hayro::hayro_interpret::hayro_cmap::BfString::Char(c) => c.to_string(),
            hayro::hayro_interpret::hayro_cmap::BfString::String(s) => s,
        };
        if text.chars().all(|c| c.is_control()) {
            return;
        }
        let full = transform * glyph_transform;
        let coeffs = full.as_coeffs();
        let origin = full * Point::ZERO;
        // Glyph space is 1/1000 of text space (PDF 9.2.4, non-Type3
        // conventionally too), so the transform's vertical magnitude is
        // the rendered em size / 1000.
        let size = ((coeffs[2] * coeffs[2] + coeffs[3] * coeffs[3]) as f32).sqrt() * 1000.0;
        if size <= 0.0 {
            return;
        }
        self.events.push(GlyphEvent {
            x: origin.x as f32,
            y: origin.y as f32,
            size,
            text,
        });
    }
}

/// Run the interpreter over a page with the recording device and build
/// reading-ordered lines.
pub(crate) fn extract(page: &Page<'_>, settings: &InterpreterSettings) -> Vec<TextLine> {
    let (width, height) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let mut context = Context::new(
        page.initial_transform(true).to_kurbo(),
        Rect::new(0.0, 0.0, width as f64, height as f64),
        &cache,
        page.xref(),
        settings.clone(),
    );
    let mut device = TextDevice::default();
    interpret_page(page, &mut context, &mut device);
    build_lines(device.events)
}

fn build_lines(mut events: Vec<GlyphEvent>) -> Vec<TextLine> {
    if events.is_empty() {
        return Vec::new();
    }
    // Cluster by baseline: sort by y, start a new line when the baseline
    // moves more than half the running glyph size.
    events.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut clusters: Vec<Vec<GlyphEvent>> = Vec::new();
    for event in events {
        match clusters.last_mut() {
            Some(line) => {
                let last = line.last().unwrap();
                if (event.y - last.y).abs() <= last.size.max(event.size) * 0.5 {
                    line.push(event);
                } else {
                    clusters.push(vec![event]);
                }
            }
            None => clusters.push(vec![event]),
        }
    }

    let mut lines = Vec::new();
    let mut offset = 0u32;
    for mut cluster in clusters {
        cluster.sort_by(|a, b| a.x.total_cmp(&b.x));
        let size = cluster.iter().map(|e| e.size).fold(0.0f32, f32::max);
        let baseline = cluster.iter().map(|e| e.y).fold(0.0f32, f32::max);
        let mut text = String::new();
        let mut glyphs = Vec::new();
        for (i, event) in cluster.iter().enumerate() {
            // Advance ≈ gap to the next glyph; last glyph estimates from
            // its size.
            let width = cluster
                .get(i + 1)
                .map(|next| (next.x - event.x).max(event.size * 0.2))
                .unwrap_or(event.size * 0.55);
            glyphs.push(TextGlyph {
                x: event.x,
                width,
                offset,
            });
            offset += event.text.chars().count() as u32;
            text.push_str(&event.text);
        }
        lines.push(TextLine {
            top: baseline - size * 0.85,
            height: size * 1.15,
            text,
            glyphs,
        });
    }
    lines
}
