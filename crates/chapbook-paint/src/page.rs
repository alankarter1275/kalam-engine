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
    /// kalam: the reference glyph box around the baseline — the tallest
    /// ascent and deepest descent among the line's fonts, in CSS px. The
    /// line box is taller than `ascent + descent` by the line-height's
    /// leading, split above and below; `baseline - ascent` is where the
    /// glyph box starts within the fragment. What a selection band is
    /// sized by (see [`LineFragment::band_extent`]).
    pub ascent: f32,
    pub descent: f32,
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
    /// Whether this glyph sits in a right-to-left bidi run.
    ///
    /// Nothing paints differently for it — the shaper already placed the
    /// glyph. It is here because *hit-testing* cannot be done without it:
    /// glyphs are stored in logical order while `x` is visual, and in an
    /// RTL run those disagree, so "which side of this glyph did the finger
    /// land on, and does that mean the offset before it or after it" has
    /// the opposite answer. Carried from the shaper's own bidi level
    /// rather than guessed from x ordering, which a one-glyph run cannot
    /// tell you.
    pub rtl: bool,
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
    /// `[start, end)`: one rect per *visually contiguous* piece of it.
    ///
    /// Usually that is one rect per line, and for text in a single
    /// direction it always is. Bidi is the exception, and the reason this
    /// does not simply take the leftmost and rightmost selected glyph on
    /// each line: a logically contiguous range need not be visually
    /// contiguous. Select from the start of an RTL line through the middle
    /// of a Latin word embedded in it and the two selected pieces sit at
    /// opposite ends with unselected letters *between* them — one spanning
    /// rect would paint over those, telling the reader they had selected
    /// text they had not.
    ///
    /// Pieces that merely touch are merged, so an ordinary selection still
    /// comes back as one rect per line and a shell can keep drawing them
    /// naively.
    pub fn rects_for_range(&self, start: u32, end: u32) -> Vec<Rect> {
        let mut rects = Vec::new();
        let mut spans = Vec::new();
        for fragment in &self.fragments {
            let (FragmentKind::Line(line) | FragmentKind::HiddenText(line)) = &fragment.kind else {
                continue;
            };
            line.selected_spans(start, end, &mut spans);
            let r = fragment.rect;
            // kalam: the band, not the line box — the same extent the
            // painter fills, so a shell placing a chip or a grip on these
            // rects lands on the pixels the reader sees.
            let (top, bottom) = line.band_extent(r.size.h);
            rects.extend(spans.iter().map(|&(from, to)| Rect {
                origin: Point::new(r.origin.x + from, r.origin.y + top),
                size: Size::new(to - from, bottom - top),
            }));
        }
        rects
    }
}

/// kalam: breathing room a selection band gets above and below the glyph
/// box, in CSS px — the old reader's `selectionBandPadding`.
pub const BAND_PADDING: f32 = 2.0;

impl LineFragment {
    /// kalam: the vertical extent of a selection band on this line, as
    /// (top, bottom) offsets from the fragment top: the glyph box plus
    /// [`BAND_PADDING`] above and below, kept inside the line box so the
    /// bands of adjacent lines never overlap. A line whose glyph box is
    /// as tall as its box (line-height 1, a formula) gets the whole box.
    ///
    /// The line box is what upstream filled. It reads as a slab: at
    /// Kalam's line-height of 1.8 the tint covered the leading between
    /// lines too, so a two-line selection was one block. The old reader
    /// sized each line's band to its first glyph's height plus 2 px, and
    /// this is that rule in fragment terms.
    pub fn band_extent(&self, line_height: f32) -> (f32, f32) {
        // No glyph box (a producer that left the metrics zero): the
        // whole line box, as before.
        if self.ascent + self.descent <= 0.0 {
            return (0.0, line_height);
        }
        let top = (self.baseline - self.ascent - BAND_PADDING).max(0.0);
        let bottom = (self.baseline + self.descent + BAND_PADDING).min(line_height);
        if bottom > top {
            (top, bottom)
        } else {
            (0.0, line_height)
        }
    }

    /// The visually contiguous pieces of the locator range `[start, end)`
    /// on this line, as fragment-local x spans, left to right. Written
    /// into `out`, which is cleared first — this runs per line per drag
    /// event, so the caller keeps the buffer.
    ///
    /// Usually one span, and for text in a single direction always one.
    /// Bidi is the exception, and the reason this is not just the leftmost
    /// and rightmost selected glyph: a logically contiguous range need not
    /// be visually contiguous. Select from the start of an RTL line
    /// through the middle of a Latin word embedded in it and the two
    /// selected pieces sit at opposite ends with unselected letters
    /// *between* them. One spanning rect would paint over those, telling
    /// the reader they had selected text they had not.
    ///
    /// Pieces that merely touch are merged, so an ordinary selection comes
    /// back as one span and nothing downstream has to care.
    pub(crate) fn selected_spans(&self, start: u32, end: u32, out: &mut Vec<(f32, f32)>) {
        out.clear();
        if end <= start {
            return;
        }
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.locator >= start && glyph.locator < end {
                    out.push((glyph.x, glyph.x + glyph.advance));
                }
            }
        }
        if out.is_empty() {
            return;
        }
        // Into visual order: glyphs arrive logically, which inside an RTL
        // run is right to left.
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Adjacent glyphs mostly share an edge exactly, but a glyph's
        // advance and the next one's position are computed separately and
        // disagree in the last hundredth of a pixel. Splitting there would
        // put a hairline seam between letters, which is exactly the artifact
        // this is supposed to avoid, so absorb it. Half a pixel is two
        // orders of magnitude above that jitter and two below any real gap
        // — the unselected middle of a split bidi range is tens of pixels.
        const TOUCHING: f32 = 0.5;
        let mut write = 0;
        for read in 1..out.len() {
            let (from, to) = out[read];
            if from <= out[write].1 + TOUCHING {
                out[write].1 = out[write].1.max(to);
            } else {
                write += 1;
                out[write] = (from, to);
            }
        }
        out.truncate(write + 1);
    }

    /// Caret offset for a fragment-local x: the nearest glyph boundary.
    ///
    /// Streams the glyphs with one-element lookahead — this runs per line
    /// per drag *event* during selection, so it must not allocate.
    ///
    /// # Why this cannot just walk the list
    ///
    /// Glyphs are stored in **logical** order and positioned in **visual**
    /// order, and bidi is where those stop agreeing. Picking "the last
    /// glyph whose `x` the point is past" reads the list as if later meant
    /// further right; in an RTL run `x` *descends*, so every point on a
    /// Hebrew or Arabic line satisfied that for every glyph and the answer
    /// was always the end of the line. Selecting RTL text was impossible
    /// and a tap reported the wrong word.
    ///
    /// So the glyph is chosen by geometry — whose horizontal span contains
    /// the point, else whichever is nearest — and only then does direction
    /// decide which side of it the caret falls on. In an LTR run the
    /// *right* half means the following offset; in an RTL run the right
    /// half is the *preceding* character, so it is the left half that
    /// advances.
    fn offset_at_x(&self, x: f32) -> Option<u32> {
        let mut best: Option<(f32, u32)> = None;
        let mut glyphs = self.runs.iter().flat_map(|run| &run.glyphs).peekable();
        while let Some(glyph) = glyphs.next() {
            // Zero past the span it covers, otherwise how far outside.
            let distance = (glyph.x - x).max(x - (glyph.x + glyph.advance)).max(0.0);
            if best.is_some_and(|(d, _)| d <= distance) {
                continue;
            }
            let past_midpoint = if glyph.rtl {
                x < glyph.x + glyph.advance / 2.0
            } else {
                x > glyph.x + glyph.advance / 2.0
            };
            let offset = if past_midpoint {
                // The next boundary in *logical* order, which is the next
                // glyph in the list whichever way the run runs.
                glyphs
                    .peek()
                    .map(|next| next.locator.max(glyph.locator))
                    .unwrap_or(glyph.locator + 1)
            } else {
                glyph.locator
            };
            best = Some((distance, offset));
        }
        best.map(|(_, offset)| offset)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(baseline: f32, ascent: f32, descent: f32) -> LineFragment {
        LineFragment {
            baseline,
            ascent,
            descent,
            runs: Vec::new(),
            decorations: Vec::new(),
            text: String::new(),
            locator_start: 0,
        }
    }

    #[test]
    fn band_is_the_glyph_box_plus_padding_inside_the_line_box() {
        // 17 px Literata at line-height 1.8: a 30.6 px line box around a
        // ~21 px glyph box, centred. The band is that box plus 2 px each
        // way, so a gap is left between the bands of two lines.
        let l = line(4.8 + 16.0, 16.0, 5.0);
        let (top, bottom) = l.band_extent(30.6);
        assert!((top - 2.8).abs() < 1e-4, "top {top}");
        assert!((bottom - 27.8).abs() < 1e-4, "bottom {bottom}");
    }

    #[test]
    fn band_never_leaves_the_line_box() {
        // Line-height 1: the glyph box is the line box; the padding is
        // clipped rather than spilling into the neighbours.
        let l = line(16.0, 16.0, 5.0);
        assert_eq!(l.band_extent(21.0), (0.0, 21.0));
        // Nonsense metrics (no ascent at all) still yield the whole box.
        let l = line(0.0, 0.0, 0.0);
        assert_eq!(l.band_extent(10.0), (0.0, 10.0));
    }
}
