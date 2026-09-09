//! kalam: the pieces a *scrolling* shell composes a continuous view from.
//!
//! The session reads a book one page at a time: `frame()` paints the
//! current page, `offset_at` hit-tests it, `next_page` moves off it.
//! A scroll view shows several pages at once, none of them "current" in
//! that sense — the reader is looking at the bottom of one and the top
//! of the next — so it needs the same things *by page*, without the
//! session's position moving underneath:
//!
//! - the size of each page's content once the empty tail a break left
//!   is cut off ([`Session::page_extent`]), so pages glue together as
//!   one flow instead of a stack of sheets with blank bands between;
//! - the display list for any page, with highlights and the live
//!   selection painted on it ([`Session::page_frame`]);
//! - hit-testing and text geometry against any page
//!   ([`Session::offset_at_page`], [`Session::word_at_page`],
//!   [`Session::link_at_page`], [`Session::host_highlight_at_page`],
//!   [`Session::range_rects_on_page`]);
//! - a way to tell the session where the reader is once they have
//!   scrolled there ([`Session::set_position`]), so `layered_locator`,
//!   `unit_fraction` and `PositionChanged` keep meaning what they mean.
//!
//! What this module deliberately does not do is own the scroll: how tall
//! the strip is, which pages are visible, what the scrollbar says while a
//! chapter is not yet laid out — that is the shell's, because it lives in
//! the shell's coordinate space and its widget toolkit. A chapter's
//! height before layout is an estimate the shell makes from
//! [`Session::chapter_char_count`] and corrects on layout; the engine has
//! nothing truer to offer without laying the chapter out.
//!
//! Two things the paged loop gets for free need saying out loud here.
//! A jump (`goto`, `goto_layered`, `follow_link`, `goto_toc`,
//! `goto_host_highlight`) *lands* on the next `frame()`, which a scroll
//! shell never takes: it calls [`Session::settle`] instead, then reads
//! `position()` and scrolls there. And a font or metrics change drops
//! every cached layout, which the paged loop notices through the frame's
//! `Relayout` intent; a scroll shell watches
//! [`Session::layout_generation`] and rebuilds its strip when it moves.
//! While several chapters are on screen the shell names them with
//! [`Session::pin_units`], so the cache budget does not evict a chapter
//! between measuring it and drawing it.
//!
//! Everything answers in the *page's* coordinate space (CSS px, origin
//! at the page's top-left, margins included), the same space `frame()`
//! and `offset_at` use. The shell adds the page's offset in the strip.
//! The image-book zoom view is not applied here — a scroll of a comic is
//! not this round's problem — so these calls are for text books.

use chapbook_core::{Locator, Point, Rect, Rgba};
use chapbook_paint::{DisplayList, FrameIntent, Selection};
use chapbook_render_tinyskia::tiny_skia;

use crate::{Highlight, Session};

/// One page's contribution to a continuous strip — see
/// [`Session::page_extent`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageExtent {
    /// Flow space (CSS px) the shell should leave *above* this page's
    /// content: the block margin the page break discarded. `0.0` for the
    /// first page of a chapter.
    pub gap_before: f32,
    /// Height of the content actually placed on the page, from the top of
    /// its content box to the bottom of its lowest fragment, CSS px. The
    /// shell shows the page from `content.origin.y` down to
    /// `content.origin.y + used_height` and nothing below.
    pub used_height: f32,
    /// The page's content box in page space — where `used_height` is
    /// measured from, and what to clip to horizontally.
    pub content: Rect,
    /// Locator offset the page starts at — `char_map[page]`.
    pub start_offset: u32,
}

impl Session {
    /// How many pages `spine` has once laid out, laying it out if need be.
    /// `None` until metrics are set or when the unit cannot be read.
    ///
    /// The scroll shell's per-chapter question. Laying out is the cost
    /// [`Session::page_count`] already pays for the current unit; this
    /// pays it for any, and the result is cached under the same budget.
    pub fn page_count_of(&mut self, spine: usize) -> Option<usize> {
        Some(self.layout_unit(spine)?.pages.len())
    }

    /// Whether `spine` is laid out *right now*, without laying it out.
    ///
    /// A shell estimating a strip's height wants to know which chapters
    /// it can measure and which it must guess, and asking must not be the
    /// thing that makes them all real.
    pub fn is_laid_out(&self, spine: usize) -> bool {
        self.layout(spine).is_some()
    }

    /// The trimmed extent of one page: what it adds to a continuous strip.
    /// Lays the unit out if need be. `None` for a page that does not exist.
    pub fn page_extent(&mut self, spine: usize, page: usize) -> Option<PageExtent> {
        let layout = self.layout_unit(spine)?;
        let content = layout.pages.get(page)?.content;
        Some(PageExtent {
            gap_before: layout.gap_before(page),
            used_height: layout.used_height(page),
            content,
            start_offset: layout.char_map.get(page).copied().unwrap_or(0),
        })
    }

    /// The extents of every page of `spine`, in order — one layout, one
    /// call, for the shell rebuilding a chapter's slot in its strip.
    pub fn page_extents(&mut self, spine: usize) -> Vec<PageExtent> {
        let Some(layout) = self.layout_unit(spine) else {
            return Vec::new();
        };
        (0..layout.pages.len())
            .map(|page| PageExtent {
                gap_before: layout.gap_before(page),
                used_height: layout.used_height(page),
                content: layout.pages[page].content,
                start_offset: layout.char_map.get(page).copied().unwrap_or(0),
            })
            .collect()
    }

    /// Display list for any page, with the host's highlights and — when
    /// the page is in the unit the selection lives in — the live selection
    /// painted under the text, exactly as [`Session::frame`] paints the
    /// current page. Does not move the reader, does not touch the frame
    /// record (intent and damage stay whatever they were).
    ///
    /// The first op is the page ground at full page size; a scroll shell
    /// clips to the extent it shows. `None` for a page that does not exist
    /// or a unit that cannot lay out.
    pub fn page_frame(&mut self, spine: usize, page: usize) -> Option<DisplayList> {
        self.layout_unit(spine)?;
        let palette = self.settings.palette();
        let highlight_color = palette.highlight;
        let paint = |h: &Highlight| Selection {
            start: h.start,
            end: h.end,
            color: h
                .color
                .as_deref()
                .and_then(|hex| Rgba::from_hex(hex, highlight_color.a))
                .unwrap_or(highlight_color),
        };
        let mut selections: Vec<Selection> =
            self.host_highlights(spine).iter().map(paint).collect();
        if spine == self.spine {
            if let Some((start, end)) = self.selected_range() {
                selections.push(Selection {
                    start,
                    end,
                    color: palette.selection,
                });
            }
        }
        let page = self.layout(spine)?.pages.get(page)?;
        Some(chapbook_paint::build_display_list(
            page,
            palette.background,
            &selections,
        ))
    }

    /// Rasterize any page with the bundled CPU backend, at the session's
    /// metrics, into a fresh pixmap the size of the page. The scroll
    /// shell's convenience over [`Session::page_frame`] +
    /// [`Session::paint_resources`], for one that does not rasterize
    /// itself. No rotation, no panel quantization: a desktop scroll view.
    pub fn render_page(&mut self, spine: usize, page: usize) -> Option<tiny_skia::Pixmap> {
        let scale = self.metrics?.dpi_scale;
        let dl = self.page_frame(spine, page)?;
        let (w, h) = ((dl.size.w * scale) as u32, (dl.size.h * scale) as u32);
        let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
        // Field accesses: the renderer borrows `fonts` mutably alongside
        // the unit's image store, which a method call would hide.
        let images = self
            .units
            .get(&spine)
            .and_then(|unit| unit.images.as_ref())
            .unwrap_or(&self.empty_images);
        self.renderer
            .render(&dl, &mut self.fonts, images, scale, &mut pixmap.as_mut());
        Some(pixmap)
    }

    /// Locator offset nearest a point on any page, in that page's space —
    /// [`Session::offset_at`]'s hit rule (a point in the margin snaps to
    /// the nearest line), for a page that need not be current. Lays the
    /// unit out if need be. `None` off text.
    pub fn offset_at_page(&mut self, spine: usize, page: usize, x: f32, y: f32) -> Option<u32> {
        self.layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at(Point::new(x, y))
    }

    /// The word under a point on any page, only when the point is *on*
    /// its line — [`Session::word_at_exact`] for a page that need not be
    /// current. The tap-to-look-up test.
    pub fn word_at_page(
        &mut self,
        spine: usize,
        page: usize,
        x: f32,
        y: f32,
    ) -> Option<(u32, u32)> {
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(x, y))?;
        let speakable = self.speakable_unit_page(spine, page)?;
        speakable
            .words
            .iter()
            .find(|word| word.locator_start <= offset && offset < word.locator_end)
            .map(|word| (word.locator_start, word.locator_end))
    }

    /// The link under a point on any page — [`Session::link_at`] for a
    /// page that need not be current.
    pub fn link_at_page(&mut self, spine: usize, page: usize, x: f32, y: f32) -> Option<String> {
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(x, y))?;
        self.unit(spine)?
            .links
            .as_ref()?
            .iter()
            .find(|link| offset >= link.start && offset < link.end)
            .map(|link| link.href.clone())
    }

    /// The host's highlight under a point on any page —
    /// [`Session::host_highlight_at`] for a page that need not be current.
    pub fn host_highlight_at_page(
        &mut self,
        spine: usize,
        page: usize,
        x: f32,
        y: f32,
    ) -> Option<i64> {
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(x, y))?;
        self.host_highlights(spine)
            .iter()
            .find(|h| offset >= h.start && offset < h.end)
            .map(|h| h.id)
    }

    /// Page-space rects covering a locator range on any page —
    /// [`Session::range_rects`] for a page that need not be current.
    /// Empty when the page is not laid out or the range lies elsewhere.
    pub fn range_rects_on_page(
        &self,
        spine: usize,
        page: usize,
        start: u32,
        end: u32,
    ) -> Vec<Rect> {
        self.layout(spine)
            .and_then(|layout| layout.pages.get(page))
            .map(|page| page.rects_for_range(start, end))
            .unwrap_or_default()
    }

    /// Begin a selection at a point on any page. The selection lives in
    /// that page's unit, so the reader's position moves there first (a
    /// selection is where the reader is looking). Returns whether the
    /// point hit text.
    pub fn selection_begin_on_page(&mut self, spine: usize, page: usize, x: f32, y: f32) -> bool {
        self.set_position(spine, page);
        self.selection = None;
        self.mark(FrameIntent::Selection);
        let Some(offset) = self.offset_at_page(spine, page, x, y) else {
            return false;
        };
        self.selection = Some((offset, offset));
        true
    }

    /// Extend the selection to a point on any page *of the selection's
    /// unit*. A drag that crosses into another chapter is ignored: a
    /// selection is one unit's locator range, and the strip shows the two
    /// chapters end to end only visually.
    pub fn selection_drag_on_page(&mut self, spine: usize, page: usize, x: f32, y: f32) {
        if spine != self.spine {
            return;
        }
        let Some((anchor, _)) = self.selection else {
            return;
        };
        if let Some(offset) = self.offset_at_page(spine, page, x, y) {
            self.selection = Some((anchor, offset));
            self.mark(FrameIntent::Selection);
        }
    }

    /// Tell the session where the reader is, as a page the shell has
    /// scrolled to. What [`Session::next_page`] does for a paged shell, a
    /// scroll shell does here from its own scroll position.
    ///
    /// Clears the selection only when the unit changes (a selection
    /// belongs to one unit; scrolling within it keeps it). Lays the unit
    /// out if it is not — the reader is looking at it — and clamps to the
    /// pages it has. Returns whether the position moved.
    pub fn set_position(&mut self, spine: usize, page: usize) -> bool {
        if spine >= self.spine_len() {
            return false;
        }
        let before = self.position();
        let page = match self.layout_unit(spine) {
            Some(layout) => page.min(layout.pages.len().saturating_sub(1)),
            None => 0,
        };
        if spine != self.spine {
            self.selection = None;
        }
        self.spine = spine;
        self.page = page;
        // A pending landing describes the unit the reader was in when the
        // jump was made; scrolling somewhere else supersedes it.
        self.pending_offset = None;
        self.pending_anchor = None;
        self.mark(if before.spine == spine {
            FrameIntent::PageTurn
        } else {
            FrameIntent::UnitChange
        });
        self.position() != before
    }

    /// Land any pending jump now, without painting — what `frame()` does
    /// first for a paged shell. A jump made with `goto`, `goto_layered`,
    /// `follow_link`, `goto_toc` or `goto_host_highlight` records where
    /// to land and resolves it against the unit's layout on the next
    /// frame; a scroll shell takes no frames, so it calls this, then
    /// reads [`Session::position`] and scrolls there. Returns the
    /// position, landed. Harmless when nothing is pending.
    pub fn settle(&mut self) -> crate::Position {
        self.land_pending();
        if let Some(layout) = self.layout(self.spine) {
            if !layout.pages.is_empty() {
                self.page = self.page.min(layout.pages.len() - 1);
            }
        }
        self.position()
    }

    /// Turn a pending jump into a page, once its unit has laid out.
    ///
    /// Both kinds of landing are dropped rather than deferred once the
    /// reader has left the unit they were captured in. A pending landing
    /// is a statement about one unit, and the reader having navigated
    /// away supersedes it — carrying it along would resolve an offset
    /// from one chapter against the pages of another.
    pub(crate) fn land_pending(&mut self) {
        if let Some((spine, fragment)) = self.pending_anchor.take() {
            if spine == self.spine {
                match self.layout_unit(spine) {
                    Some(layout) => {
                        // A fragment that isn't in the unit lands at its start.
                        self.page = layout.anchors.get(&fragment).copied().unwrap_or(0);
                    }
                    None => self.pending_anchor = Some((spine, fragment)),
                }
            }
        }
        if let Some((spine, offset)) = self.pending_offset {
            if spine != self.spine {
                self.pending_offset = None;
            } else if let Some(layout) = self.layout_unit(spine) {
                self.page = layout.page_of(offset);
                self.pending_offset = None;
            }
        }
    }

    /// A counter that moves every time the session drops its cached
    /// layouts — a font-size, theme, line-height or metrics change,
    /// or [`Session::release_caches`]. Page counts and extents read
    /// before it moved describe layouts that no longer exist; a scroll
    /// shell that finds it changed rebuilds its strip, anchoring on the
    /// locator offset at the top of the viewport, which survives.
    pub fn layout_generation(&self) -> u64 {
        self.layout_generation
    }

    /// Name the units a scroll shell has on screen, so eviction spares
    /// them along with the current unit. Without this the cache budget
    /// can drop a neighbouring chapter between the shell measuring it and
    /// drawing it, and the shell's next ask lays it out again — a stall
    /// on every frame near a chapter seam once the book is bigger than
    /// the budget. A half-open range of spine indices; `0..0` pins none.
    /// Pinning is a floor, not a ceiling: the budget still applies to
    /// everything else.
    pub fn pin_units(&mut self, units: std::ops::Range<usize>) {
        self.pinned_units = units;
    }

    /// Where a jump would land, without making it: the page in `target`'s
    /// unit that holds its offset, once that unit is laid out. What a
    /// scroll shell asks so it can scroll *to* the target instead of
    /// letting `goto` re-page the session behind its back. `None` when
    /// the unit cannot lay out.
    pub fn page_of(&mut self, target: Locator) -> Option<usize> {
        let layout = self.layout_unit(target.spine_index)?;
        Some(layout.page_of(target.char_offset))
    }

    /// Where a TOC fragment lands in its unit, as a page. `None` when the
    /// unit cannot lay out; a fragment the unit does not have lands on
    /// page 0, as [`Session::goto_anchor`] does.
    pub fn page_of_anchor(&mut self, spine: usize, fragment: &str) -> Option<usize> {
        let layout = self.layout_unit(spine)?;
        Some(layout.anchors.get(fragment).copied().unwrap_or(0))
    }
}
