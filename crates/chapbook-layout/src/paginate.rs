//! The streaming page cursor: block flow + cosmic-text inline layout +
//! CSS-fragmentation page breaking.
//!
//! Break rules honored in v1: forced `break-before/after: page` (+ legacy
//! aliases), `break-inside: avoid` (retry on a fresh page, else break
//! anyway), widows/orphans (default 2/2), margins collapsed between
//! siblings and discarded at page boundaries. `avoid` on before/after
//! (keep-with-next/previous) is parsed but not yet enforced. A line taller
//! than a page is placed alone and may overflow (clipped at paint).

use app_units::Au;
use cosmic_text::{Buffer, FontSystem, Metrics, Shaping, Wrap};
use style::properties::ComputedValues;
use style::values::computed::LengthPercentage;

use chapbook_core::{PageMetrics, Point, Rect, Size};
use chapbook_paint::{Decoration, Fragment, FragmentKind, Glyph, GlyphRun, LineFragment, Page};

use crate::boxtree::{BlockBox, BlockKind, InlineContent};
use crate::fragmentation::BreakRule;
use crate::style_to_attrs::{align_for, attrs_for, font_size_px, line_height_px, text_color};

/// One shaped visual line, ready to be placed on a page.
struct ShapedLine {
    height: f32,
    baseline: f32,
    width: f32,
    /// Alignment-induced left offset of the line box within its block.
    x_indent: f32,
    runs: Vec<GlyphRun>,
    decorations: Vec<Decoration>,
    text: String,
    locator_start: u32,
}

pub(crate) struct Paginator<'f> {
    fonts: &'f mut FontSystem,
    page: PageMetrics,
    content: Rect,
    pages: Vec<Page>,
    /// Cursor within the current page's content box.
    y: f32,
    /// Fragments placed on the current page.
    placed_on_page: usize,
    /// Collapsed margin waiting to be committed before the next content.
    pending_margin: f32,
    /// A forced break-after is pending: next content starts a new page.
    force_break: bool,
    /// Per page: smallest locator offset placed on it (u32::MAX = none yet).
    page_locators: Vec<u32>,
}

impl<'f> Paginator<'f> {
    pub fn new(fonts: &'f mut FontSystem, page: PageMetrics) -> Self {
        let content = Rect::new(
            page.margins.left,
            page.margins.top,
            page.content_width(),
            page.content_height(),
        );
        let mut p = Paginator {
            fonts,
            page,
            content,
            pages: Vec::new(),
            y: 0.0,
            placed_on_page: 0,
            pending_margin: 0.0,
            force_break: false,
            page_locators: Vec::new(),
        };
        p.new_page();
        p
    }

    pub fn finish(mut self) -> (Vec<Page>, Vec<u32>) {
        // Backfill locator starts for pages that carried no text.
        let mut last = 0u32;
        for loc in &mut self.page_locators {
            if *loc == u32::MAX {
                *loc = last;
            } else {
                last = *loc;
            }
        }
        (self.pages, self.page_locators)
    }

    fn new_page(&mut self) {
        self.pages.push(Page {
            size: self.page.size,
            content: self.content,
            fragments: Vec::new(),
        });
        self.page_locators.push(u32::MAX);
        self.y = 0.0;
        self.placed_on_page = 0;
        // Margins are discarded at fragmentainer boundaries.
        self.pending_margin = 0.0;
    }

    fn at_page_top(&self) -> bool {
        self.placed_on_page == 0
    }

    fn remaining(&self) -> f32 {
        self.content.size.h - self.y
    }

    /// Lay out one block box into the page stream.
    ///
    /// `x` is the left inset of the block's content box within the page
    /// content box; `width` its content width.
    pub fn place_block(&mut self, block: &BlockBox, x: f32, width: f32) {
        let style = &block.style;
        let cw = width;

        let (margin_top, margin_bottom, margin_left, margin_right) = if block.anonymous {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            let m = style.get_margin();
            (
                resolve_margin(&m.margin_top, cw),
                resolve_margin(&m.margin_bottom, cw),
                resolve_margin(&m.margin_left, cw),
                resolve_margin(&m.margin_right, cw),
            )
        };
        let (pad_top, pad_bottom, pad_left, pad_right) = if block.anonymous {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            let p = style.get_padding();
            (
                resolve_padding(&p.padding_top, cw),
                resolve_padding(&p.padding_bottom, cw),
                resolve_padding(&p.padding_left, cw),
                resolve_padding(&p.padding_right, cw),
            )
        };

        if !block.anonymous {
            if block.frag.break_before == BreakRule::Page && !self.at_page_top() {
                self.new_page();
            }
            if self.force_break && !self.at_page_top() {
                self.new_page();
            }
            self.force_break = false;
        }

        self.pending_margin = self.pending_margin.max(margin_top);

        let inner_x = x + margin_left + pad_left;
        let inner_w = (width - margin_left - margin_right - pad_left - pad_right).max(1.0);

        if pad_top > 0.0 {
            self.commit_margin();
            self.y += pad_top;
        }

        match &block.kind {
            BlockKind::Container(children) => {
                for child in children {
                    self.place_block(child, inner_x, inner_w);
                }
            }
            BlockKind::Inline(inline) => {
                let lines = self.shape_inline(inline, style, inner_w);
                let tag = chapbook_dom::node_tag(block.node);
                self.place_lines(lines, block, inner_x, tag);
            }
            BlockKind::Image { width, height } => {
                self.place_image(block, *width, *height, inner_x, inner_w);
            }
            BlockKind::Rule => {
                self.commit_margin();
                let color = text_color(style);
                let rect = Rect {
                    origin: Point::new(
                        self.content.origin.x + inner_x,
                        self.content.origin.y + self.y,
                    ),
                    size: Size::new(inner_w, 1.0),
                };
                self.pages.last_mut().unwrap().fragments.push(Fragment {
                    rect,
                    kind: FragmentKind::Rule { color },
                    tag: chapbook_dom::node_tag(block.node),
                });
                self.y += 1.0;
                self.placed_on_page += 1;
            }
        }

        if pad_bottom > 0.0 {
            self.y += pad_bottom;
        }
        self.pending_margin = self.pending_margin.max(margin_bottom);
        if !block.anonymous && block.frag.break_after == BreakRule::Page {
            self.force_break = true;
        }
    }

    /// Place a replaced image block: scaled to fit the content width and
    /// page height, kept whole (an image never splits across pages), and
    /// centered horizontally.
    fn place_image(&mut self, block: &BlockBox, width: u32, height: u32, x: f32, inner_w: f32) {
        if width == 0 || height == 0 {
            return;
        }
        self.commit_margin();
        let mut w = (width as f32).min(inner_w);
        let mut h = height as f32 * w / width as f32;
        let max_h = self.content.size.h;
        if h > max_h {
            w *= max_h / h;
            h = max_h;
        }
        if h > self.remaining() + 0.01 && !self.at_page_top() {
            self.new_page();
        }
        let x_center = x + (inner_w - w) / 2.0;
        let rect = Rect {
            origin: Point::new(
                self.content.origin.x + x_center,
                self.content.origin.y + self.y,
            ),
            size: Size::new(w, h),
        };
        self.pages.last_mut().unwrap().fragments.push(Fragment {
            rect,
            kind: FragmentKind::Image {
                resource: chapbook_dom::node_tag(block.node),
            },
            tag: chapbook_dom::node_tag(block.node),
        });
        self.y += h;
        self.placed_on_page += 1;
    }

    fn commit_margin(&mut self) {
        if !self.at_page_top() {
            self.y += self.pending_margin;
        }
        self.pending_margin = 0.0;
    }

    fn shape_inline(
        &mut self,
        inline: &InlineContent,
        block_style: &ComputedValues,
        width: f32,
    ) -> Vec<ShapedLine> {
        if inline.runs.is_empty() {
            return Vec::new();
        }
        let metrics = Metrics {
            font_size: font_size_px(block_style),
            line_height: line_height_px(block_style),
        };
        let mut buffer = Buffer::new(self.fonts, metrics);
        buffer.set_size(Some(width), None);
        buffer.set_wrap(Wrap::WordOrGlyph);

        let default_attrs = attrs_for(block_style, 0);
        let spans = inline
            .runs
            .iter()
            .enumerate()
            .map(|(i, run)| (run.text.as_str(), attrs_for(&run.style, i)));
        buffer.set_rich_text(
            spans,
            &default_attrs,
            Shaping::Advanced,
            align_for(block_style),
        );
        buffer.shape_until_scroll(self.fonts, false);

        // Byte→locator mapping of the buffer's text, split per buffer line
        // ('\n' from <br> starts a new line and is not part of any line's
        // text). Spans concatenate in order, so buffer byte order == run
        // concatenation order.
        let mut line_maps: Vec<Vec<(usize, u32)>> = vec![Vec::new()];
        let mut byte_in_line = 0usize;
        for run in &inline.runs {
            let mut offsets = run.offsets.iter();
            for ch in run.text.chars() {
                let locator = offsets.next().copied().unwrap_or(0);
                if ch == '\n' {
                    line_maps.push(Vec::new());
                    byte_in_line = 0;
                } else {
                    line_maps.last_mut().unwrap().push((byte_in_line, locator));
                    byte_in_line += ch.len_utf8();
                }
            }
        }
        let locator_at = |line_i: usize, byte: usize| -> u32 {
            let map = match line_maps.get(line_i) {
                Some(m) if !m.is_empty() => m,
                _ => return 0,
            };
            let idx = map.partition_point(|(b, _)| *b <= byte);
            map[idx.saturating_sub(1)].1
        };

        let fallback_color = text_color(block_style);
        let mut lines = Vec::new();
        for run in buffer.layout_runs() {
            let mut glyph_runs: Vec<GlyphRun> = Vec::new();
            let mut text_start = usize::MAX;
            let mut text_end = 0usize;
            for glyph in run.glyphs {
                text_start = text_start.min(glyph.start);
                text_end = text_end.max(glyph.end);
                let color = glyph
                    .color_opt
                    .map(|c| chapbook_core::Rgba::new(c.r(), c.g(), c.b(), c.a()))
                    .unwrap_or(fallback_color);
                let g = Glyph {
                    id: glyph.glyph_id,
                    x: glyph.x + glyph.x_offset,
                    y: glyph.y - glyph.y_offset,
                    advance: glyph.w,
                };
                match glyph_runs.last_mut() {
                    Some(last)
                        if last.font == glyph.font_id
                            && last.font_size == glyph.font_size
                            && last.font_weight == glyph.font_weight.0
                            && last.color == color =>
                    {
                        last.glyphs.push(g);
                    }
                    _ => glyph_runs.push(GlyphRun {
                        font: glyph.font_id,
                        font_size: glyph.font_size,
                        font_weight: glyph.font_weight.0,
                        color,
                        glyphs: vec![g],
                    }),
                }
            }
            let text = if text_start <= text_end && text_end <= run.text.len() {
                run.text[text_start..text_end].to_string()
            } else {
                String::new()
            };
            let locator_start = run
                .glyphs
                .first()
                .map(|g| locator_at(run.line_i, g.start))
                .unwrap_or(0);
            // Alignment (center/right/justify) shifts glyph x's within the
            // buffer; normalize so glyphs are relative to the line box and
            // the fragment rect reports the actual visual extent.
            let x_min = glyph_runs
                .iter()
                .flat_map(|r| r.glyphs.iter().map(|g| g.x))
                .fold(f32::INFINITY, f32::min);
            let x_min = if x_min.is_finite() { x_min } else { 0.0 };
            for run_mut in &mut glyph_runs {
                for g in &mut run_mut.glyphs {
                    g.x -= x_min;
                }
            }

            // Decorations: underline/strikethrough spans with font-derived
            // offsets and thickness (EM units scaled by the span font size).
            let baseline = run.line_y - run.line_top;
            let mut decorations = Vec::new();
            for span in run.decorations {
                let span_glyphs = &run.glyphs[span.glyph_range.clone()];
                let Some(first) = span_glyphs.first() else {
                    continue;
                };
                let last = span_glyphs.last().unwrap();
                let x0 = first.x - x_min;
                let x1 = last.x + last.w - x_min;
                let color = span
                    .color_opt
                    .map(|c| chapbook_core::Rgba::new(c.r(), c.g(), c.b(), c.a()))
                    .unwrap_or(fallback_color);
                // Font size lives on the span, not the metrics data.
                let size = span.font_size;
                let mut push = |metrics: cosmic_text::DecorationMetrics| {
                    let thickness = (metrics.thickness * size).max(1.0);
                    decorations.push(Decoration {
                        x: x0,
                        width: (x1 - x0).max(0.0),
                        y: baseline - metrics.offset * size,
                        thickness,
                        color,
                    });
                };
                if span.data.text_decoration.underline != cosmic_text::UnderlineStyle::None {
                    push(span.data.underline_metrics);
                }
                if span.data.text_decoration.strikethrough {
                    push(span.data.strikethrough_metrics);
                }
            }

            lines.push(ShapedLine {
                height: run.line_height,
                baseline,
                width: run.line_w,
                x_indent: x_min,
                runs: glyph_runs,
                decorations,
                text,
                locator_start,
            });
        }
        lines
    }

    fn place_lines(&mut self, lines: Vec<ShapedLine>, block: &BlockBox, x: f32, tag: u64) {
        if lines.is_empty() {
            return;
        }
        let orphans = block.frag.orphans.max(1) as usize;
        let widows = block.frag.widows.max(1) as usize;
        let total: usize = lines.len();
        let total_height: f32 = lines.iter().map(|l| l.height).sum();

        self.commit_margin();

        // break-inside: avoid — move the whole box to a fresh page when it
        // would otherwise split and can fit on one page.
        if block.frag.break_inside_avoid
            && !self.at_page_top()
            && total_height > self.remaining()
            && total_height <= self.content.size.h
        {
            self.new_page();
        }

        let mut i = 0usize;
        while i < total {
            // How many of the remaining lines fit on this page?
            let mut fits = 0usize;
            let mut used = 0.0f32;
            for line in &lines[i..] {
                if self.y + used + line.height > self.content.size.h + 0.01 {
                    break;
                }
                used += line.height;
                fits += 1;
            }
            let rest_after_page = total - i - fits;

            if rest_after_page == 0 {
                self.emit_lines(&lines[i..], x, tag);
                break;
            }

            let mut take = fits;
            // Orphans: too few lines would open the split on this page.
            if take < orphans {
                if !self.at_page_top() {
                    self.new_page();
                    continue;
                }
                // Already at page top: force progress, overflow allowed.
                take = take.max(1);
            }
            // Widows: too few lines would land after the break.
            let rest = total - i - take;
            if rest > 0 && rest < widows {
                let deficit = widows - rest;
                if take > deficit && take - deficit >= orphans {
                    take -= deficit;
                } else if !self.at_page_top() {
                    self.new_page();
                    continue;
                }
            }

            self.emit_lines(&lines[i..i + take], x, tag);
            i += take;
            if i < total {
                self.new_page();
            }
        }
    }

    fn emit_lines(&mut self, lines: &[ShapedLine], x: f32, tag: u64) {
        for line in lines {
            let rect = Rect {
                origin: Point::new(
                    self.content.origin.x + x + line.x_indent,
                    self.content.origin.y + self.y,
                ),
                size: Size::new(line.width, line.height),
            };
            let page_loc = self.page_locators.last_mut().unwrap();
            *page_loc = (*page_loc).min(line.locator_start);
            self.pages.last_mut().unwrap().fragments.push(Fragment {
                rect,
                kind: FragmentKind::Line(LineFragment {
                    baseline: line.baseline,
                    runs: line.runs.clone(),
                    decorations: line.decorations.clone(),
                    text: line.text.clone(),
                    locator_start: line.locator_start,
                }),
                tag,
            });
            self.y += line.height;
            self.placed_on_page += 1;
        }
    }
}

fn resolve_lp(lp: &LengthPercentage, containing: f32) -> f32 {
    lp.to_used_value(Au::from_f64_px(containing as f64))
        .to_f64_px() as f32
}

fn resolve_margin(margin: &style::values::computed::length::Margin, containing: f32) -> f32 {
    use style::values::generics::length::GenericMargin;
    match margin {
        GenericMargin::LengthPercentage(lp) => resolve_lp(lp, containing),
        _ => 0.0, // auto and anchor forms: v1 treats as zero
    }
}

fn resolve_padding(
    padding: &style::values::computed::NonNegativeLengthPercentage,
    containing: f32,
) -> f32 {
    resolve_lp(&padding.0, containing)
}
