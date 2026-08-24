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
    /// A raster image scaled into `dest`, resolved via the `ImageStore`.
    Image {
        resource: u64,
        dest: Rect,
    },
}

/// A selection to highlight: locator range plus fill color, painted per
/// line under the text.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub start: u32,
    pub end: u32,
    pub color: Rgba,
}

/// Flatten a laid-out page into draw ops. `background` becomes the first op
/// (a full-page fill), so themes (night mode) are a color choice here, not a
/// renderer concern. A `selection` paints its highlight rect immediately
/// before each line it touches — over earlier backgrounds, under the text.
pub fn build_display_list(
    page: &Page,
    background: Rgba,
    selection: Option<Selection>,
) -> DisplayList {
    let mut ops = vec![DisplayOp::FillRect {
        rect: Rect::new(0.0, 0.0, page.size.w, page.size.h),
        color: background,
    }];

    for fragment in &page.fragments {
        match &fragment.kind {
            // Hidden text paints only its selection highlight — the pixels
            // are in the raster underneath.
            FragmentKind::HiddenText(line) => {
                if let Some(sel) = selection {
                    push_selection_rect(&mut ops, fragment, line, sel);
                }
            }
            FragmentKind::Line(line) => {
                if let Some(sel) = selection {
                    push_selection_rect(&mut ops, fragment, line, sel);
                }
                let origin = Point::new(
                    fragment.rect.origin.x,
                    fragment.rect.origin.y + line.baseline,
                );
                // Decorations paint under the glyphs, like browsers do.
                for deco in &line.decorations {
                    ops.push(DisplayOp::FillRect {
                        rect: Rect::new(
                            fragment.rect.origin.x + deco.x,
                            fragment.rect.origin.y + deco.y,
                            deco.width,
                            deco.thickness,
                        ),
                        color: deco.color,
                    });
                }
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
            FragmentKind::Image { resource } => {
                ops.push(DisplayOp::Image {
                    resource: *resource,
                    dest: fragment.rect,
                });
            }
            FragmentKind::Box(decoration) => {
                push_box_decoration(&mut ops, &fragment.rect, decoration);
            }
        }
    }

    DisplayList {
        size: page.size,
        ops,
    }
}

/// The per-line selection highlight: the union of the selected glyphs'
/// extents, painted before the line's own ops.
fn push_selection_rect(
    ops: &mut Vec<DisplayOp>,
    fragment: &crate::page::Fragment,
    line: &crate::page::LineFragment,
    sel: Selection,
) {
    if sel.end <= sel.start {
        return;
    }
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for run in &line.runs {
        for glyph in &run.glyphs {
            if glyph.locator >= sel.start && glyph.locator < sel.end {
                min_x = min_x.min(glyph.x);
                max_x = max_x.max(glyph.x + glyph.advance);
            }
        }
    }
    if max_x > min_x {
        ops.push(DisplayOp::FillRect {
            rect: Rect::new(
                fragment.rect.origin.x + min_x,
                fragment.rect.origin.y,
                max_x - min_x,
                fragment.rect.size.h,
            ),
            color: sel.color,
        });
    }
}

/// Background fill + solid border edges. Horizontal edges paint only on the
/// slices that carry them; vertical edges paint on every slice.
fn push_box_decoration(
    ops: &mut Vec<DisplayOp>,
    rect: &Rect,
    decoration: &crate::page::BoxDecoration,
) {
    if let Some(background) = decoration.background {
        if !background.is_transparent() {
            ops.push(DisplayOp::FillRect {
                rect: *rect,
                color: background,
            });
        }
    }
    let w = &decoration.border_widths;
    let color = decoration.border_color;
    if color.is_transparent() {
        return;
    }
    let (x, y) = (rect.origin.x, rect.origin.y);
    let (bw, bh) = (rect.size.w, rect.size.h);
    if decoration.first_slice && w.top > 0.0 {
        ops.push(DisplayOp::FillRect {
            rect: Rect::new(x, y, bw, w.top),
            color,
        });
    }
    if decoration.last_slice && w.bottom > 0.0 {
        ops.push(DisplayOp::FillRect {
            rect: Rect::new(x, y + bh - w.bottom, bw, w.bottom),
            color,
        });
    }
    if w.left > 0.0 {
        ops.push(DisplayOp::FillRect {
            rect: Rect::new(x, y, w.left, bh),
            color,
        });
    }
    if w.right > 0.0 {
        ops.push(DisplayOp::FillRect {
            rect: Rect::new(x + bw - w.right, y, w.right, bh),
            color,
        });
    }
}
