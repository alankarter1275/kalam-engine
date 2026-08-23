//! The paint-neutral display list: dumb draw ops in page coordinates
//! (CSS px), executed by any backend without re-shaping or style access.

use chapbook_core::{Point, Rect, Rgba, Size};

use crate::page::{FragmentKind, Glyph, Page};

#[derive(Debug, Clone)]
pub struct DisplayList {
    /// Full page size in CSS px; backends scale to device pixels.
    pub size: Size,
    pub ops: Vec<DisplayOp>,
}

#[derive(Debug, Clone)]
pub enum DisplayOp {
    FillRect {
        rect: Rect,
        color: Rgba,
    },
    /// Positioned glyphs sharing one face/size/weight/color. `origin` is the
    /// baseline origin in page coordinates; glyph offsets are relative to it.
    GlyphRun {
        font: cosmic_text::fontdb::ID,
        font_size: f32,
        font_weight: u16,
        color: Rgba,
        origin: Point,
        glyphs: Vec<Glyph>,
    },
}

/// Flatten a laid-out page into draw ops. `background` becomes the first op
/// (a full-page fill), so themes (night mode) are a color choice here, not a
/// renderer concern.
pub fn build_display_list(page: &Page, background: Rgba) -> DisplayList {
    let mut ops = vec![DisplayOp::FillRect {
        rect: Rect::new(0.0, 0.0, page.size.w, page.size.h),
        color: background,
    }];

    for fragment in &page.fragments {
        match &fragment.kind {
            FragmentKind::Line(line) => {
                let origin = Point::new(
                    fragment.rect.origin.x,
                    fragment.rect.origin.y + line.baseline,
                );
                for run in &line.runs {
                    ops.push(DisplayOp::GlyphRun {
                        font: run.font,
                        font_size: run.font_size,
                        font_weight: run.font_weight,
                        color: run.color,
                        origin,
                        glyphs: run.glyphs.clone(),
                    });
                }
            }
            FragmentKind::Rule { color } => {
                ops.push(DisplayOp::FillRect {
                    rect: fragment.rect,
                    color: *color,
                });
            }
            // Image resources land with M5's resource store.
            FragmentKind::Image { .. } => {}
        }
    }

    DisplayList {
        size: page.size,
        ops,
    }
}
