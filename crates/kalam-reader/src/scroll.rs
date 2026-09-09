//! The book as one strip: the model behind `ReaderView`'s scrolled mode.
//!
//! The engine lays a chapter out as pages — that is what pagination-first
//! means — and says how tall each page's *content* is once the empty tail
//! a page break left is cut off (`Session::page_extents`). This module
//! glues those trimmed pages end to end into a single strip: chapter
//! after chapter, band after band, with the margin a break discarded put
//! back between bands and a seam between chapters. A chapter that has not
//! been laid out yet is a guess — its character count times a
//! pixels-per-character density learned from the chapters that have —
//! and the guess is replaced by the measurement the first time the
//! viewport reaches it.
//!
//! The one thing this has to get right is that correcting a guess never
//! moves the text under the reader's eyes. Every scroll the reader makes
//! re-derives an *anchor*: the band under the reading line (the point
//! `MARGIN_TOP` below the viewport's top edge, where a page's first line
//! sits in paged mode) and how far into that band the line is — or, in a
//! chapter still guessed, how far through the guess. Every change of
//! heights (a chapter measured, a font change that drops every
//! measurement) puts the scroll position back where the anchor says.
//! Chapters above the viewport may grow or shrink by thousands of pixels;
//! the scrollbar's thumb moves, the words do not.
//!
//! The anchor is also the reading position the widget reports: the page
//! under the reading line. A jump puts its target page's top exactly
//! there, so a position saved from here and restored lands where it was
//! saved, with no drift from one session to the next. A relayout is the
//! one change the anchor cannot survive by itself — the pages are new —
//! so the widget notes which *line of text* is on the reading line
//! before, asks the engine where that line went, and anchors to it.
//!
//! No GTK in here. Strip coordinates are CSS px from the top of the book;
//! the widget maps them to its own (`widget y = strip y − scroll`). That
//! keeps the arithmetic testable on a machine with no display, which is
//! where it is tested.

use chapbook_core::Rect;
use chapbook_reader::PageExtent;

use crate::view::{MARGIN_BOTTOM, MARGIN_TOP};

/// Space between the last line of one chapter and the first of the next:
/// one page's bottom margin plus the next page's top margin, so a chapter
/// seam in the strip looks like the page turn it is in paged mode.
pub(crate) const CHAPTER_GAP: f32 = MARGIN_TOP + MARGIN_BOTTOM;

/// How much of the viewport a "page" of scrolling moves: a little less
/// than all of it, so the last line read is still on screen as the first.
pub(crate) const PAGE_SCROLL_FRACTION: f32 = 0.9;

/// A guessed chapter is never shorter than this much of the viewport, so
/// an empty or image-only chapter still has a slot to scroll through.
const MIN_ESTIMATE_VIEWPORTS: f32 = 0.5;

/// One page's band in the strip: where the page's content, trimmed to
/// what is on it, sits in strip space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Band {
    pub spine: usize,
    pub page: usize,
    /// Strip y the band starts at.
    pub top: f32,
    /// The band's height — the page's `used_height`.
    pub height: f32,
    /// Page-space y that lands at `top`: the page's content-box top.
    pub page_y: f32,
}

impl Band {
    pub(crate) fn bottom(&self) -> f32 {
        self.top + self.height
    }
}

/// What is under the reading line, in terms that survive a reflow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Anchor {
    /// A measured page band, with the reading line `dy` below its top
    /// (negative when the line is in the gap above the band).
    Page { spine: usize, page: usize, dy: f32 },
    /// A chapter not yet measured, with the reading line this far through
    /// its guessed height.
    Estimate { spine: usize, fraction: f32 },
}

#[derive(Debug, Clone)]
struct Slot {
    /// Locator chars in the chapter, for the guess.
    chars: u64,
    /// The chapter's pages, once measured.
    pages: Option<Vec<PageExtent>>,
    /// Measured or guessed height of the chapter's content.
    height: f32,
    /// Strip y the chapter starts at.
    top: f32,
}

pub(crate) struct Strip {
    slots: Vec<Slot>,
    viewport: f32,
    scroll_y: f32,
    anchor: Anchor,
    /// The engine's layout generation the measurements belong to.
    generation: u64,
    px_per_char: f32,
}

impl Strip {
    /// A strip with every chapter guessed: `chars` per chapter, at
    /// `px_per_char` until a measurement teaches a better density.
    pub(crate) fn new(chars: Vec<u64>, viewport: f32, px_per_char: f32, generation: u64) -> Strip {
        let mut strip = Strip {
            slots: chars
                .into_iter()
                .map(|chars| Slot {
                    chars,
                    pages: None,
                    height: 0.0,
                    top: 0.0,
                })
                .collect(),
            viewport: viewport.max(1.0),
            scroll_y: 0.0,
            anchor: Anchor::Page {
                spine: 0,
                page: 0,
                dy: 0.0,
            },
            generation,
            px_per_char: px_per_char.max(0.01),
        };
        strip.reflow();
        strip
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn viewport(&self) -> f32 {
        self.viewport
    }

    pub(crate) fn scroll_y(&self) -> f32 {
        self.scroll_y
    }

    pub(crate) fn is_measured(&self, spine: usize) -> bool {
        self.slots
            .get(spine)
            .is_some_and(|slot| slot.pages.is_some())
    }

    /// The whole strip, margins included.
    pub(crate) fn total_height(&self) -> f32 {
        match self.slots.last() {
            Some(slot) => slot.top + slot.height + MARGIN_BOTTOM,
            None => MARGIN_TOP + MARGIN_BOTTOM,
        }
    }

    pub(crate) fn max_scroll(&self) -> f32 {
        (self.total_height() - self.viewport).max(0.0)
    }

    /// The page under the reading line, once the chapter there is
    /// measured. What the widget reports as the reading position.
    pub(crate) fn reading_page(&self) -> Option<(usize, usize)> {
        match self.anchor {
            Anchor::Page { spine, page, .. } => Some((spine, page)),
            Anchor::Estimate { .. } => None,
        }
    }

    // ---- Changes ----

    /// The engine dropped its layouts (a font, theme, column or viewport
    /// change): every measurement is stale. The chapters' sizes in
    /// characters and the density learned so far are not, and stay.
    pub(crate) fn rebuild(&mut self, generation: u64, viewport: f32) {
        self.generation = generation;
        self.viewport = viewport.max(1.0);
        for slot in &mut self.slots {
            slot.pages = None;
        }
        self.reflow();
    }

    /// Replace a chapter's guess with its pages. The reading line stays
    /// on the same text; the scroll position moves to keep it there.
    pub(crate) fn measure(&mut self, spine: usize, pages: Vec<PageExtent>) {
        let Some(slot) = self.slots.get_mut(spine) else {
            return;
        };
        slot.pages = Some(pages);
        self.learn_density();
        self.reflow();
    }

    /// Scroll to a position the reader chose (wheel, keys, the scrollbar).
    /// Clamped to the strip; the anchor follows. Whether it moved.
    pub(crate) fn set_scroll(&mut self, y: f32) -> bool {
        let before = self.scroll_y;
        self.scroll_y = y.clamp(0.0, self.max_scroll());
        self.anchor = self.anchor_at(self.scroll_y + MARGIN_TOP);
        (self.scroll_y - before).abs() > 0.0
    }

    pub(crate) fn scroll_by(&mut self, dy: f32) -> bool {
        self.set_scroll(self.scroll_y + dy)
    }

    /// Put a page's top on the reading line — a jump, a mode switch.
    /// Exact once the chapter is measured; until then the chapter's top
    /// stands in, and the measurement moves the scroll to the page.
    pub(crate) fn jump_to(&mut self, spine: usize, page: usize) {
        self.anchor_to(spine, page, 0.0);
    }

    /// Put the point `dy` below a page's content top on the reading line
    /// — how a relayout puts the line the reader was on back where it
    /// was, once the engine has said which page and how far down that
    /// line now is.
    pub(crate) fn anchor_to(&mut self, spine: usize, page: usize, dy: f32) {
        self.anchor = Anchor::Page { spine, page, dy };
        self.follow_anchor();
    }

    /// The reading line as a point on a page: (spine, page, page-space
    /// y). `None` while the chapter under it is guessed.
    pub(crate) fn reading_line(&self) -> Option<(usize, usize, f32)> {
        let Anchor::Page { spine, page, dy } = self.anchor else {
            return None;
        };
        let band = self.bands(spine).nth(page)?;
        Some((spine, page, band.page_y + dy))
    }

    /// Re-derive the anchor from where the scroll is now — the end of a
    /// frame, once everything visible is measured, so that
    /// [`Strip::reading_page`] names a real page.
    pub(crate) fn settle_anchor(&mut self) {
        self.anchor = self.anchor_at(self.scroll_y + MARGIN_TOP);
    }

    // ---- Queries ----

    /// The chapters the viewport shows any part of, as a half-open range
    /// of spine indices.
    pub(crate) fn visible_units(&self) -> std::ops::Range<usize> {
        if self.slots.is_empty() {
            return 0..0;
        }
        let mut first = self.slot_at(self.scroll_y);
        // A viewport whose top edge is in the seam after a chapter shows
        // none of that chapter.
        let past_first = self
            .slots
            .get(first)
            .is_some_and(|slot| self.scroll_y >= slot.top + slot.height);
        if past_first && first + 1 < self.slots.len() {
            first += 1;
        }
        let last = self.slot_at(self.scroll_y + self.viewport).max(first);
        first..last + 1
    }

    /// Chapters to measure before painting: the visible ones still
    /// guessed, and the anchor's if it is — the reader is looking at it,
    /// and until it is measured the scroll only approximates where.
    pub(crate) fn unmeasured_visible(&self) -> Vec<usize> {
        let mut pending: Vec<usize> = self
            .visible_units()
            .filter(|&spine| !self.is_measured(spine))
            .collect();
        if let Anchor::Page { spine, .. } = self.anchor {
            if spine < self.slots.len() && !self.is_measured(spine) && !pending.contains(&spine) {
                pending.insert(0, spine);
            }
        }
        pending
    }

    /// The bands the viewport shows any part of, top to bottom.
    pub(crate) fn visible_bands(&self) -> Vec<Band> {
        let (lo, hi) = (self.scroll_y, self.scroll_y + self.viewport);
        self.visible_units()
            .flat_map(|spine| self.bands(spine))
            .filter(|band| band.top < hi && band.bottom() > lo)
            .collect()
    }

    /// The band under a widget y, with the page-space y the point has on
    /// that page. `None` in a gap, a seam, a margin or a guessed chapter.
    pub(crate) fn widget_to_page(&self, y: f32) -> Option<(Band, f32)> {
        let strip_y = self.scroll_y + y;
        let spine = self.slot_at(strip_y);
        let band = self
            .bands(spine)
            .find(|band| strip_y >= band.top && strip_y < band.bottom())?;
        Some((band, band.page_y + strip_y - band.top))
    }

    /// A page-space rect on `band`'s page, in widget coordinates.
    pub(crate) fn to_widget(&self, band: &Band, rect: Rect) -> Rect {
        Rect::new(
            rect.origin.x,
            rect.origin.y - band.page_y + band.top - self.scroll_y,
            rect.size.w,
            rect.size.h,
        )
    }

    // ---- Internals ----

    /// The bands of one chapter, in order; none while it is guessed.
    fn bands(&self, spine: usize) -> impl Iterator<Item = Band> + '_ {
        let slot = self.slots.get(spine);
        let mut y = slot.map_or(0.0, |slot| slot.top);
        let pages = slot.and_then(|slot| slot.pages.as_deref()).unwrap_or(&[]);
        pages.iter().enumerate().map(move |(page, extent)| {
            y += extent.gap_before;
            let band = Band {
                spine,
                page,
                top: y,
                height: extent.used_height,
                page_y: extent.content.origin.y,
            };
            y += extent.used_height;
            band
        })
    }

    /// The chapter a strip y falls in. The seam after a chapter belongs
    /// to it; the margin above the first chapter belongs to the first.
    fn slot_at(&self, y: f32) -> usize {
        self.slots
            .partition_point(|slot| slot.top <= y)
            .saturating_sub(1)
    }

    /// The anchor for a reading line at strip `y`.
    fn anchor_at(&self, y: f32) -> Anchor {
        let spine = self.slot_at(y);
        let Some(slot) = self.slots.get(spine) else {
            return Anchor::Page {
                spine: 0,
                page: 0,
                dy: 0.0,
            };
        };
        // The band containing y, else the first below it (y is in a gap),
        // else the last (y is in the seam after the chapter).
        let band = self
            .bands(spine)
            .find(|band| y < band.bottom())
            .or_else(|| self.bands(spine).last());
        match band {
            Some(band) => Anchor::Page {
                spine,
                page: band.page,
                dy: y - band.top,
            },
            None => Anchor::Estimate {
                spine,
                fraction: if slot.height > 0.0 {
                    ((y - slot.top) / slot.height).clamp(0.0, 1.0)
                } else {
                    0.0
                },
            },
        }
    }

    /// Where the anchor is now, as a strip y.
    fn anchor_y(&self) -> f32 {
        match self.anchor {
            Anchor::Page { spine, page, dy } => {
                let Some(slot) = self.slots.get(spine) else {
                    return MARGIN_TOP;
                };
                let band = self
                    .bands(spine)
                    .nth(page)
                    .or_else(|| self.bands(spine).last());
                match band {
                    Some(band) => band.top + dy,
                    None => slot.top + dy,
                }
            }
            Anchor::Estimate { spine, fraction } => self
                .slots
                .get(spine)
                .map_or(MARGIN_TOP, |slot| slot.top + fraction * slot.height),
        }
    }

    fn follow_anchor(&mut self) {
        self.scroll_y = (self.anchor_y() - MARGIN_TOP).clamp(0.0, self.max_scroll());
    }

    /// Pixels per character, from every measured chapter with text.
    fn learn_density(&mut self) {
        let (mut px, mut chars) = (0.0f64, 0.0f64);
        for slot in &self.slots {
            if let Some(pages) = &slot.pages {
                if slot.chars > 0 {
                    px += f64::from(measured_height(pages));
                    chars += slot.chars as f64;
                }
            }
        }
        if chars > 0.0 && px > 0.0 {
            self.px_per_char = (px / chars) as f32;
        }
    }

    /// Recompute every chapter's height and top, then put the scroll back
    /// where the anchor says.
    fn reflow(&mut self) {
        let floor = self.viewport * MIN_ESTIMATE_VIEWPORTS;
        let density = self.px_per_char;
        let mut y = MARGIN_TOP;
        for slot in &mut self.slots {
            slot.height = match &slot.pages {
                Some(pages) => measured_height(pages),
                None => (slot.chars as f32 * density).max(floor),
            };
            slot.top = y;
            y += slot.height + CHAPTER_GAP;
        }
        self.follow_anchor();
    }
}

fn measured_height(pages: &[PageExtent]) -> f32 {
    pages
        .iter()
        .map(|page| page.gap_before + page.used_height)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: f32 = 800.0;

    /// A page at the widget's metrics: content box `MARGIN_TOP` down.
    fn extent(gap_before: f32, used_height: f32, start_offset: u32) -> PageExtent {
        PageExtent {
            gap_before,
            used_height,
            content: Rect::new(40.0, MARGIN_TOP, 520.0, 712.0),
            start_offset,
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_measurement_never_moves_the_text_under_the_reading_line() {
        // Three chapters of a thousand chars, guessed at half a pixel
        // each: 500 px slots at 48, 636 and 1224.
        let mut strip = Strip::new(vec![1000, 1000, 1000], VIEWPORT, 0.5, 1);
        assert!(near(strip.total_height(), 1224.0 + 500.0 + MARGIN_BOTTOM));

        // The reader scrolls into the second chapter's guess.
        strip.set_scroll(700.0);
        let line = 700.0 + MARGIN_TOP;
        assert_eq!(
            strip.anchor,
            Anchor::Estimate {
                spine: 1,
                fraction: (line - 636.0) / 500.0
            }
        );
        assert_eq!(strip.reading_page(), None, "nothing measured yet");

        // Measuring it teaches a density of 1.02 px/char, which regrows
        // the other two guesses; the reading line keeps its fraction.
        strip.measure(1, vec![extent(0.0, 700.0, 0), extent(20.0, 300.0, 5000)]);
        let top1 = MARGIN_TOP + 1020.0 + CHAPTER_GAP;
        assert!(near(strip.scroll_y(), top1 + 0.224 * 1020.0 - MARGIN_TOP));
        strip.settle_anchor();
        assert_eq!(strip.reading_page(), Some((1, 0)));
        let before = strip.widget_to_page(MARGIN_TOP).expect("on the first band");

        // The chapter above turns out 2000 px, not the 1020 guessed:
        // everything below it moves 980 px down, the scroll with it, and
        // the same point of the same page is still under the reading
        // line.
        strip.measure(0, vec![extent(0.0, 2000.0, 0)]);
        let after = strip.widget_to_page(MARGIN_TOP).expect("still on it");
        assert_eq!(
            (after.0.spine, after.0.page),
            (before.0.spine, before.0.page)
        );
        assert!(near(after.1, before.1), "{} vs {}", after.1, before.1);
        assert!(near(after.0.top - before.0.top, 980.0));
        assert!(near(
            strip.scroll_y(),
            before.0.top + 980.0 + 228.48 - MARGIN_TOP
        ));
    }

    #[test]
    fn a_jump_puts_the_page_top_on_the_reading_line() {
        let mut strip = Strip::new(vec![1000; 4], VIEWPORT, 0.5, 1);
        // Before the chapter is measured the chapter's top stands in.
        strip.jump_to(1, 1);
        let guessed_top1 = MARGIN_TOP + 500.0 + CHAPTER_GAP;
        assert!(near(strip.scroll_y(), guessed_top1 - MARGIN_TOP));

        // Measured (1130 px for 1000 chars, so the chapter above regrows
        // to 1130 too), the scroll moves on to the page itself.
        strip.measure(1, vec![extent(0.0, 700.0, 0), extent(30.0, 400.0, 900)]);
        strip.settle_anchor();
        assert_eq!(strip.reading_page(), Some((1, 1)));
        let (band, page_y) = strip.widget_to_page(MARGIN_TOP).expect("the page's band");
        assert_eq!((band.spine, band.page), (1, 1));
        let top1 = MARGIN_TOP + 1130.0 + CHAPTER_GAP;
        assert!(near(band.top, top1 + 700.0 + 30.0));
        assert!(
            near(page_y, MARGIN_TOP),
            "the band's first line, at its content top"
        );
        assert!(near(strip.scroll_y(), band.top - MARGIN_TOP));
    }

    #[test]
    fn points_map_between_the_widget_and_the_page() {
        let mut strip = Strip::new(vec![4000, 4000], VIEWPORT, 0.5, 1);
        strip.measure(0, vec![extent(0.0, 700.0, 0), extent(24.0, 300.0, 4000)]);
        assert!(near(strip.scroll_y(), 0.0));

        // Unscrolled, the first band is the page itself: widget y is
        // page y.
        let (band, page_y) = strip.widget_to_page(148.0).unwrap();
        assert_eq!((band.spine, band.page), (0, 0));
        assert!(near(page_y, 148.0));

        // The second band starts 24 px below the first ends; a point 28 px
        // into it is 28 px into that page's content.
        let second_top = MARGIN_TOP + 700.0 + 24.0;
        let (band, page_y) = strip.widget_to_page(second_top + 28.0).unwrap();
        assert_eq!((band.spine, band.page), (0, 1));
        assert!(near(page_y, MARGIN_TOP + 28.0));
        let rect = strip.to_widget(&band, Rect::new(40.0, MARGIN_TOP + 28.0, 100.0, 20.0));
        assert!(near(rect.origin.y, second_top + 28.0), "and back: {rect:?}");

        // The gap between bands, the seam after the chapter and the
        // guessed chapter are nobody's text.
        assert!(strip.widget_to_page(MARGIN_TOP + 700.0 + 10.0).is_none());
        assert!(strip.widget_to_page(second_top + 300.0 + 10.0).is_none());
        let seam_end = second_top + 300.0 + CHAPTER_GAP;
        assert!(strip.widget_to_page(seam_end + 10.0).is_none());

        // Scrolled, the same page point is higher in the widget.
        strip.set_scroll(500.0);
        let (band, page_y) = strip.widget_to_page(second_top + 28.0 - 500.0).unwrap();
        assert_eq!((band.spine, band.page), (0, 1));
        assert!(near(page_y, MARGIN_TOP + 28.0));
    }

    #[test]
    fn the_scroll_stays_inside_the_strip() {
        let mut strip = Strip::new(vec![1000; 3], VIEWPORT, 0.5, 1);
        strip.set_scroll(-100.0);
        assert!(near(strip.scroll_y(), 0.0));
        strip.set_scroll(1e9);
        assert!(near(strip.scroll_y(), strip.max_scroll()));
        assert!(near(strip.max_scroll(), strip.total_height() - VIEWPORT));

        // A book shorter than the viewport does not scroll at all.
        let mut short = Strip::new(vec![100], VIEWPORT, 0.5, 1);
        short.measure(0, vec![extent(0.0, 100.0, 0)]);
        assert!(near(
            short.total_height(),
            MARGIN_TOP + 100.0 + MARGIN_BOTTOM
        ));
        short.set_scroll(50.0);
        assert!(near(short.scroll_y(), 0.0));
        assert_eq!(short.visible_units(), 0..1);
    }

    #[test]
    fn a_rebuild_forgets_the_pages_and_keeps_what_it_learned() {
        let mut strip = Strip::new(vec![1000; 3], VIEWPORT, 0.5, 1);
        strip.measure(0, vec![extent(0.0, 1500.0, 0)]);
        assert!(strip.is_measured(0));
        strip.rebuild(2, VIEWPORT);
        assert_eq!(strip.generation(), 2);
        assert!(!strip.is_measured(0));
        assert_eq!(strip.unmeasured_visible(), vec![0]);
        // 1.5 px/char now, from the chapter that was measured.
        let expected = MARGIN_TOP + 3.0 * 1500.0 + 2.0 * CHAPTER_GAP + MARGIN_BOTTOM;
        let total = strip.total_height();
        assert!(near(total, expected), "{total} vs {expected}");
    }

    #[test]
    fn visible_units_and_bands_follow_the_viewport() {
        let mut strip = Strip::new(vec![1000; 3], VIEWPORT, 0.5, 1);
        for spine in 0..3 {
            strip.measure(spine, vec![extent(0.0, 700.0, 0)]);
        }
        // Slots at 48, 836 and 1624. The top of the book sees one.
        assert_eq!(strip.visible_units(), 0..1);
        assert_eq!(strip.visible_bands().len(), 1);
        // Halfway down the first chapter, the seam and the second are in.
        strip.set_scroll(500.0);
        assert_eq!(strip.visible_units(), 0..2);
        let bands: Vec<(usize, usize)> = strip
            .visible_bands()
            .iter()
            .map(|band| (band.spine, band.page))
            .collect();
        assert_eq!(bands, vec![(0, 0), (1, 0)]);
        assert!(strip.unmeasured_visible().is_empty());
        strip.settle_anchor();
        assert_eq!(strip.reading_page(), Some((0, 0)));
        // A guessed chapter in view is reported for measuring.
        strip.rebuild(2, VIEWPORT);
        assert_eq!(
            strip.unmeasured_visible(),
            strip.visible_units().collect::<Vec<_>>()
        );
    }
}
