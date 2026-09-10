//! The line between two chapters in scrolled mode.
//!
//! Kalam's WebKit reader drew one between every pair of chapters in its
//! continuous stream: a hairline across the column with the chapter's
//! title on it in a small uppercase pill — page-coloured, so it sits on
//! the line rather than under it. This is that divider, painted onto the
//! strip's frame with the engine's own tools: the title is shaped by
//! cosmic-text in the bundled Literata and rasterized by a
//! `chapbook-render-tinyskia` renderer of the widget's own (the session's
//! is private to it), and the line and the pill are tiny-skia paths. No
//! CSS is involved, but the numbers below *are* the stylesheet's
//! (`.kalam-chapter-divider` and `.kalam-chapter-divider-title`), in its
//! units, so the seam looks as it did — with `rem` read as Kalam's
//! `font_px` and `currentColor` as the theme's ink.
//!
//! Where a divider goes is the strip's business (`Strip::visible_dividers`
//! in `scroll.rs`); this module only knows how to draw one at a given
//! widget y.

use chapbook_core::{Point, Rgba, Size};
use chapbook_reader::chapbook_paint::{DisplayList, DisplayOp, Glyph, ImageStore};
use chapbook_reader::chapbook_render_tinyskia::Renderer;
use chapbook_reader::cosmic_text::{
    fontdb, Attrs, Buffer, Family, FontSystem, LetterSpacing, Metrics, Shaping, Weight, Wrap,
};
use chapbook_reader::tiny_skia::{
    self, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};

use crate::prefs::BODY_FONT;

// The stylesheet, in em of Kalam's base font size (its `rem`).
/// `font-size: 0.85rem`.
const TITLE_SIZE_EM: f32 = 0.85;
/// The line box inside the pill, in em: Kalam's default `line-height`.
/// In WebKit the pill took the reader's line-height preference along
/// with every other element; here it keeps the default, so the seam's
/// height (`DIVIDER_BAND` in `scroll.rs`) does not move with a
/// preference.
const TITLE_LINE_HEIGHT: f32 = 1.8;
/// `padding: 0.4rem 1.6rem`.
const PAD_X_EM: f32 = 1.6;
const PAD_Y_EM: f32 = 0.4;
/// `letter-spacing: 0.08em`.
const LETTER_SPACING_EM: f32 = 0.08;
/// The hairline runs from 6 % to 94 % of the column (`left: 6%; right: 6%`).
const RULE_INSET: f32 = 0.06;
/// `currentColor` at 22 % (the line), 20 % (the pill's border) and 65 %
/// (the title).
const RULE_ALPHA: u8 = 56;
const BORDER_ALPHA: u8 = 51;
const TITLE_ALPHA: u8 = 166;

/// Kalam's ink, paper and base size — everything the stylesheet read
/// from the theme.
pub(crate) struct DividerStyle {
    pub font_px: f32,
    pub foreground: Rgba,
    pub background: Rgba,
}

/// Where one divider goes, in the widget's CSS px.
pub(crate) struct DividerPlace {
    /// Widget y of the divider's centre line.
    pub center_y: f32,
    /// The widget's width.
    pub width: f32,
    /// Where the text column starts and how wide it is.
    pub column_x: f32,
    pub column_w: f32,
    /// Device pixels per CSS px.
    pub scale: f32,
}

/// Paints dividers. Holds a rasterizer (a glyph-mask cache, in effect)
/// so the same title costs nothing to draw twice.
#[derive(Default)]
pub(crate) struct DividerPainter {
    renderer: Renderer,
    /// Empty; the renderer wants one to resolve image ops against, and a
    /// divider has none.
    images: ImageStore,
}

impl DividerPainter {
    /// Draw one divider, titled `title`, onto `out` (device pixels).
    pub(crate) fn paint(
        &mut self,
        out: &mut Pixmap,
        fonts: &mut FontSystem,
        style: &DividerStyle,
        title: &str,
        place: &DividerPlace,
    ) {
        let scale = place.scale;
        let em = style.font_px;
        let size = TITLE_SIZE_EM * em;
        let (pad_x, pad_y) = (PAD_X_EM * em, PAD_Y_EM * em);
        let (fg, bg) = (style.foreground, style.background);
        // Whole device pixels, in CSS units.
        let snap = |v: f32| (v * scale).round() / scale;
        let to_device = Transform::from_scale(scale, scale);

        // The hairline: 6 % in from either edge of the column, one CSS
        // pixel, on the pixel grid so it is a line and not a smear.
        let rule_x = place.column_x + place.column_w * RULE_INSET;
        let rule_w = place.column_w * (1.0 - 2.0 * RULE_INSET);
        let rule_y = snap(place.center_y - 0.5);
        let mut paint = Paint::default();
        paint.anti_alias = false;
        paint.set_color_rgba8(fg.r, fg.g, fg.b, RULE_ALPHA);
        if let Some(rect) = tiny_skia::Rect::from_xywh(rule_x, rule_y, rule_w, 1.0) {
            out.fill_rect(rect, &paint, to_device, None);
        }

        // The title, uppercase, cut to the column with an ellipsis if it
        // must be — a title never wraps inside its pill.
        let max_text_w = place.column_w - 2.0 * pad_x - 2.0;
        let mut text = title.to_uppercase();
        let Some(mut shaped) = shape(fonts, &text, size) else {
            return;
        };
        for _ in 0..6 {
            let count = text.chars().count();
            if shaped.width <= max_text_w.max(0.0) || shaped.width <= 0.0 || count <= 2 {
                break;
            }
            let keep = ((count as f32 * max_text_w / shaped.width) as usize).saturating_sub(1);
            let head: String = text.chars().take(keep.max(1)).collect();
            text = format!("{}…", head.trim_end());
            match shape(fonts, &text, size) {
                Some(again) => shaped = again,
                None => return,
            }
        }

        // The pill: the text in a line box of Kalam's default line height,
        // padded, fully rounded, centred on the column and on the rule.
        // The path is the border box inset by half the border, so the
        // one-pixel stroke lands on whole pixels with the fill meeting it
        // from inside.
        let pill_w = snap(shaped.width + 2.0 * pad_x);
        let pill_h = snap(TITLE_LINE_HEIGHT * size + 2.0 * pad_y);
        let pill_x = snap(place.column_x + (place.column_w - pill_w) / 2.0);
        let pill_y = snap(place.center_y - pill_h / 2.0);
        let (bx, by) = (pill_x + 0.5, pill_y + 0.5);
        let Some(path) = pill_path(bx, by, pill_w - 1.0, pill_h - 1.0) else {
            return;
        };
        paint.anti_alias = true;
        paint.set_color_rgba8(bg.r, bg.g, bg.b, 255);
        out.fill_path(&path, &paint, FillRule::Winding, to_device, None);
        paint.set_color_rgba8(fg.r, fg.g, fg.b, BORDER_ALPHA);
        let stroke = Stroke {
            width: 1.0,
            ..Stroke::default()
        };
        out.stroke_path(&path, &paint, &stroke, to_device, None);

        // The glyphs, through the same kind of rasterizer as the page: a
        // one-line display list whose baseline puts the middle of the
        // text's ascent-plus-descent box on the rule.
        let baseline = place.center_y + (shaped.ascent - shaped.descent) / 2.0;
        let origin = Point::new(pill_x + pad_x, baseline);
        let color = Rgba::new(fg.r, fg.g, fg.b, TITLE_ALPHA);
        let ops: Vec<DisplayOp> = shaped
            .runs
            .into_iter()
            .map(|run| DisplayOp::GlyphRun {
                font: run.font,
                font_size: size,
                font_weight: run.weight,
                color,
                origin,
                glyphs: run.glyphs,
            })
            .collect();
        let list = DisplayList {
            size: Size::new(place.width, out.height() as f32 / scale),
            ops,
        };
        self.renderer
            .render(&list, fonts, &self.images, scale, &mut out.as_mut());
    }
}

/// A shaped title: its glyphs grouped by face, and its measure.
struct Shaped {
    runs: Vec<Run>,
    width: f32,
    ascent: f32,
    descent: f32,
}

struct Run {
    font: fontdb::ID,
    weight: u16,
    glyphs: Vec<Glyph>,
}

/// Shape `text` on one unwrapped line at `size` px: Literata semibold
/// (the bundled bold face answers for it), tracked 0.08 em, as the
/// stylesheet said. `None` for an empty string or a face with nothing.
fn shape(fonts: &mut FontSystem, text: &str, size: f32) -> Option<Shaped> {
    let mut attrs = Attrs::new()
        .family(Family::Name(BODY_FONT))
        .weight(Weight::SEMIBOLD);
    attrs.letter_spacing_opt = Some(LetterSpacing(LETTER_SPACING_EM));
    let mut buffer = Buffer::new(fonts, Metrics::new(size, TITLE_LINE_HEIGHT * size));
    buffer.set_wrap(Wrap::None);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fonts, false);
    let line = buffer.lines.first()?.layout_opt()?.first()?;

    let mut runs: Vec<Run> = Vec::new();
    for glyph in &line.glyphs {
        // Offsets are in em, as cosmic-text's own `LayoutGlyph::physical`
        // reads them.
        let placed = Glyph {
            id: glyph.glyph_id,
            x: glyph.x + glyph.x_offset * glyph.font_size,
            y: glyph.y - glyph.y_offset * glyph.font_size,
            advance: glyph.w,
            locator: 0,
            rtl: glyph.level.is_rtl(),
        };
        match runs.last_mut() {
            Some(run) if run.font == glyph.font_id && run.weight == glyph.font_weight.0 => {
                run.glyphs.push(placed);
            }
            _ => runs.push(Run {
                font: glyph.font_id,
                weight: glyph.font_weight.0,
                glyphs: vec![placed],
            }),
        }
    }
    if runs.is_empty() {
        return None;
    }
    Some(Shaped {
        runs,
        width: line.w,
        ascent: line.max_ascent,
        descent: line.max_descent,
    })
}

/// A stadium: a rectangle whose short sides are semicircles
/// (`border-radius: 999px`). Cubic arcs, the usual approximation.
fn pill_path(x: f32, y: f32, w: f32, h: f32) -> Option<Path> {
    const K: f32 = 0.552_28;
    let rx = (w / 2.0).min(h / 2.0);
    let ry = h / 2.0;
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }
    let (x1, y1) = (x + w, y + h);
    let cy = y + ry;
    let mut pb = PathBuilder::new();
    pb.move_to(x + rx, y);
    pb.line_to(x1 - rx, y);
    pb.cubic_to(x1 - rx + K * rx, y, x1, cy - K * ry, x1, cy);
    pb.cubic_to(x1, cy + K * ry, x1 - rx + K * rx, y1, x1 - rx, y1);
    pb.line_to(x + rx, y1);
    pb.cubic_to(x + rx - K * rx, y1, x, cy + K * ry, x, cy);
    pb.cubic_to(x, cy - K * ry, x + rx - K * rx, y, x + rx, y);
    pb.close();
    pb.finish()
}
