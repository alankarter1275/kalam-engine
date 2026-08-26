//! The format-neutral page model: a laid-out page of positioned fragments.
//!
//! Producers are format-specific (chapbook-layout paginates styled XHTML; a
//! future comic producer emits one image fragment per page); consumers (the
//! display-list builder, renderers, the viewer) never know the difference.
//! All coordinates are CSS px in page space, origin at the page's top-left.

use chapbook_core::{EdgeSizes, PageMetrics, Point, Rect, Rgba, Size};

/// One laid-out page.
#[derive(Debug, Clone)]
pub struct Page {
    /// Full page size including reader margins.
    pub size: Size,
    /// The content box the fragments were laid into.
    pub content: Rect,
    pub fragments: Vec<Fragment>,
}

/// A positioned piece of page content.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// Bounding rect in page space.
    pub rect: Rect,
    pub kind: FragmentKind,
    /// Opaque producer-defined tag (e.g. a DOM node key); `0` = untagged.
    /// Deliberately not a document-model type — see ARCHITECTURE.md.
    pub tag: u64,
}

#[derive(Debug, Clone)]
pub enum FragmentKind {
    /// One shaped visual line of text.
    Line(LineFragment),
    /// A horizontal rule.
    Rule { color: Rgba },
    /// A raster image, keyed into the producer's resource store (M5).
    Image { resource: u64 },
    /// A block box's background and borders. A box spanning several pages
    /// emits one slice per page; the slice flags gate the horizontal border
    /// edges so only outer edges paint (CSS box-decoration-break: slice).
    Box(BoxDecoration),
    /// A text line carried for geometry only: the pixels are already in a
    /// raster fragment underneath (a PDF page). Selection hit-testing and
    /// highlight rects read it exactly like [`FragmentKind::Line`]; the
    /// display list never paints its glyphs.
    HiddenText(LineFragment),
}

#[derive(Debug, Clone)]
pub struct BoxDecoration {
    pub background: Option<Rgba>,
    /// Border widths per edge (zero = no edge).
    pub border_widths: EdgeSizes,
    pub border_color: Rgba,
    /// True when this slice contains the box's top edge.
    pub first_slice: bool,
    /// True when this slice contains the box's bottom edge.
    pub last_slice: bool,
}

/// A shaped line: glyphs are final — no re-shaping happens downstream.
#[derive(Debug, Clone)]
pub struct LineFragment {
    /// Baseline offset from the top of the fragment rect.
    pub baseline: f32,
    pub runs: Vec<GlyphRun>,
    /// Underlines/strikethroughs, positioned relative to the fragment rect.
    pub decorations: Vec<Decoration>,
    /// The line's source text (diagnostics, selection, golden dumps).
    pub text: String,
    /// Locator-text char offset of the line start (see `chapbook-core`
    /// locator docs); drives `char_map` and position restore.
    pub locator_start: u32,
}

/// A decoration line (underline, strikethrough) within a line fragment.
#[derive(Debug, Clone, Copy)]
pub struct Decoration {
    /// Left edge, relative to the fragment origin.
    pub x: f32,
    pub width: f32,
    /// Top edge of the stroke, relative to the fragment top.
    pub y: f32,
    pub thickness: f32,
    pub color: Rgba,
}

/// A run of glyphs sharing one font face, size, weight, and color.
#[derive(Debug, Clone)]
pub struct GlyphRun {
    /// Face in the producer's `fontdb` (cosmic-text's database).
    pub font: cosmic_text::fontdb::ID,
    pub font_size: f32,
    /// OpenType weight (rasterizer cache key component for variable fonts
    /// and synthetic bolding).
    pub font_weight: u16,
    pub color: Rgba,
    pub glyphs: Vec<Glyph>,
}

/// One positioned glyph, relative to the line fragment's origin.
#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    pub id: u16,
    pub x: f32,
    /// Offset from the line's baseline (positive = below).
    pub y: f32,
    pub advance: f32,
    /// Locator-text char offset of the character(s) this glyph renders —
    /// what selection hit-testing and highlight geometry key on. Generated
    /// glyphs (markers, hyphens) repeat a neighbor's offset.
    pub locator: u32,
}

impl Page {
    /// Roughly how many heap bytes this page holds.
    ///
    /// Approximate on purpose: it counts the big allocations — glyphs,
    /// line text, fragment and run vectors — and ignores per-allocation
    /// overhead. A cache budget wants to know that a chapter is hundreds
    /// of kilobytes rather than tens of megabytes, and that answer does not
    /// change with the fudge.
    pub fn approx_bytes(&self) -> usize {
        use std::mem::size_of;
        let line_bytes = |line: &LineFragment| {
            line.text.len()
                + line.decorations.capacity() * size_of::<Decoration>()
                + line.runs.capacity() * size_of::<GlyphRun>()
                + line
                    .runs
                    .iter()
                    .map(|run| run.glyphs.capacity() * size_of::<Glyph>())
                    .sum::<usize>()
        };
        self.fragments.capacity() * size_of::<Fragment>()
            + self
                .fragments
                .iter()
                .map(|fragment| match &fragment.kind {
                    FragmentKind::Line(line) | FragmentKind::HiddenText(line) => line_bytes(line),
                    _ => 0,
                })
                .sum::<usize>()
    }

    /// The locator offset nearest to a page-space point: hit-testing for
    /// selection. Considers line fragments whose vertical extent contains
    /// the point; among them, the horizontally nearest wins (a point in
    /// the margin snaps to the line's edge). `None` on textless regions —
    /// and everywhere on image-book pages.
    pub fn offset_at(&self, point: Point) -> Option<u32> {
        let mut best: Option<(f32, u32)> = None;
        for fragment in &self.fragments {
            let (FragmentKind::Line(line) | FragmentKind::HiddenText(line)) = &fragment.kind else {
                continue;
            };
            let r = fragment.rect;
            if point.y < r.origin.y || point.y > r.origin.y + r.size.h {
                continue;
            }
            let local_x = point.x - r.origin.x;
            let Some(offset) = line.offset_at_x(local_x) else {
                continue;
            };
            let distance = if local_x < 0.0 {
                -local_x
            } else {
                (local_x - r.size.w).max(0.0)
            };
            if best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, offset));
            }
        }
        best.map(|(_, offset)| offset)
    }

    /// Like [`Page::offset_at`], but only when the point is actually inside
    /// a line's box — no snapping to the nearest line. What a tap on a link
    /// needs: pressing the margin beside a link is not pressing the link.
    pub fn offset_at_exact(&self, point: Point) -> Option<u32> {
        for fragment in &self.fragments {
            let (FragmentKind::Line(line) | FragmentKind::HiddenText(line)) = &fragment.kind else {
                continue;
            };
            let r = fragment.rect;
            if point.x < r.origin.x
                || point.x > r.origin.x + r.size.w
                || point.y < r.origin.y
                || point.y > r.origin.y + r.size.h
            {
                continue;
            }
            if let Some(offset) = line.offset_at_x(point.x - r.origin.x) {
                return Some(offset);
            }
        }
        None
    }

    /// Highlight rects (page space) covering the locator range
    /// `[start, end)`: one rect per line the range touches.
    pub fn rects_for_range(&self, start: u32, end: u32) -> Vec<Rect> {
        let mut rects = Vec::new();
        if end <= start {
            return rects;
        }
        for fragment in &self.fragments {
            let (FragmentKind::Line(line) | FragmentKind::HiddenText(line)) = &fragment.kind else {
                continue;
            };
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            for run in &line.runs {
                for glyph in &run.glyphs {
                    if glyph.locator >= start && glyph.locator < end {
                        min_x = min_x.min(glyph.x);
                        max_x = max_x.max(glyph.x + glyph.advance);
                    }
                }
            }
            if max_x > min_x {
                let r = fragment.rect;
                rects.push(Rect {
                    origin: Point::new(r.origin.x + min_x, r.origin.y),
                    size: Size::new(max_x - min_x, r.size.h),
                });
            }
        }
        rects
    }
}

impl LineFragment {
    /// Caret offset for a fragment-local x: the nearest glyph boundary.
    fn offset_at_x(&self, x: f32) -> Option<u32> {
        // Glyphs in visual order with their end boundaries.
        let mut result: Option<u32> = None;
        let mut first: Option<(f32, u32)> = None;
        let mut boundaries: Vec<(f32, f32, u32)> = Vec::new();
        for run in &self.runs {
            for glyph in &run.glyphs {
                boundaries.push((glyph.x, glyph.advance, glyph.locator));
                if first.is_none_or(|(fx, _)| glyph.x < fx) {
                    first = Some((glyph.x, glyph.locator));
                }
            }
        }
        for (i, &(gx, advance, locator)) in boundaries.iter().enumerate() {
            if x >= gx {
                result = Some(if x > gx + advance / 2.0 {
                    // Right half: the next boundary.
                    boundaries
                        .get(i + 1)
                        .map(|&(_, _, next)| next.max(locator))
                        .unwrap_or(locator + 1)
                } else {
                    locator
                });
            }
        }
        result.or(first.map(|(_, locator)| locator))
    }
}

/// A whole-page image: the comic pipeline. The image (intrinsic size
/// `width`×`height`, stored under `resource` in the producer's
/// [`crate::ImageStore`]) is scaled to fit the content box and centered on
/// both axes — no DOM, no styling, no shaping involved.
pub fn image_page(metrics: &PageMetrics, width: u32, height: u32, resource: u64) -> Page {
    let content = Rect::new(
        metrics.margins.left,
        metrics.margins.top,
        metrics.content_width(),
        metrics.content_height(),
    );
    let mut fragments = Vec::new();
    if width > 0 && height > 0 && content.size.w > 0.0 && content.size.h > 0.0 {
        let scale = (content.size.w / width as f32)
            .min(content.size.h / height as f32)
            .min(1.0);
        let (w, h) = (width as f32 * scale, height as f32 * scale);
        fragments.push(Fragment {
            rect: Rect {
                origin: Point::new(
                    content.origin.x + (content.size.w - w) / 2.0,
                    content.origin.y + (content.size.h - h) / 2.0,
                ),
                size: Size::new(w, h),
            },
            kind: FragmentKind::Image { resource },
            tag: resource,
        });
    }
    Page {
        size: metrics.size,
        content,
        fragments,
    }
}
