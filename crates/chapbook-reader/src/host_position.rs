//! kalam: layered positions for a host that keeps its own records.
//!
//! Upstream persisted positions and marks through chapbook's own library
//! (removed here — Kalam has its database). A host needs the same durable
//! position — quote context, spine fraction, whole-book progression — as a
//! *value* it can store wherever it likes and hand back later, and a way
//! to land on one that tolerates the book having been re-imported or
//! re-parsed in between. These calls are that, and nothing else: the
//! resolve chain is `chapbook_core::locator`'s, exactly as the library
//! path used it. (Plus one hit-test, `word_at_exact`, that the host's
//! tap-to-look-up needs and the reference viewers never did.)

use chapbook_core::{resolve_in_text, LayeredLocator, Locator, Point};

use crate::text_surface::unit_locator_text;
use crate::Session;

/// Book-wide char counts around the current unit — what
/// `LayeredLocator::capture` needs beyond the offset itself.
pub(crate) struct UnitCharContext {
    /// The current unit's locator text.
    pub(crate) text: String,
    /// Chars in the spine items before it.
    pub(crate) prior: u64,
    /// Chars across the whole book.
    pub(crate) total: u64,
}

impl Session {
    /// Char counts around the current unit. The per-unit counts are a
    /// property of the file — computed once per session (the one
    /// whole-book pass) and reused by every later capture.
    pub(crate) fn unit_char_context(&self) -> UnitCharContext {
        let counts = self.char_counts.get_or_init(|| {
            (0..self.book.publication().spine().len())
                .map(|i| {
                    unit_locator_text(self.book.publication(), i)
                        .map(|text| text.chars().count() as u64)
                        .unwrap_or(0)
                })
                .collect()
        });
        UnitCharContext {
            text: self.cached_unit_text(self.spine).unwrap_or_default(),
            prior: counts.iter().take(self.spine).sum(),
            total: counts.iter().sum(),
        }
    }

    /// The word under a point, only if the point is *on* its line — the
    /// tap-to-look-up test. [`Session::word_at`] snaps to the nearest line
    /// (right for a caret, wrong for a tap: a press in the margin beside a
    /// paragraph is a page turn, not a lookup). Same hit rule as
    /// [`Session::link_at`].
    pub fn word_at_exact(&mut self, x: f32, y: f32) -> Option<(u32, u32)> {
        let (px, py) = self.content_point(x, y);
        let (spine, page) = (self.spine, self.page);
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(px, py))?;
        let page = self.speakable_page()?;
        page.words
            .iter()
            .find(|word| word.locator_start <= offset && offset < word.locator_end)
            .map(|word| (word.locator_start, word.locator_end))
    }

    /// The current reading position as a full layered locator — a value
    /// the host can store and later hand to [`Session::goto_layered`].
    ///
    /// The first call in a session walks every chapter once to count its
    /// text (`book_progression` needs the whole-book total); a second or
    /// two on a long book on a slow disk. Every later call is cheap. A
    /// host should ask for this on close and at chapter changes, and use
    /// [`Session::unit_fraction`] for the every-page-turn save.
    ///
    /// `None` for a book with no text layer at the current unit (an
    /// image book, or a spine index the book no longer has).
    pub fn layered_locator(&self) -> Option<LayeredLocator> {
        self.layered_locator_at(self.current_offset())
    }

    /// A layered locator for one endpoint of a selection, or any other
    /// offset in the current unit's locator text. What a host stores for
    /// each end of a highlight it keeps itself.
    pub fn layered_locator_at(&self, char_offset: u32) -> Option<LayeredLocator> {
        let href = self
            .book
            .publication()
            .spine_item(self.spine)
            .ok()?
            .href
            .clone();
        let ctx = self.unit_char_context();
        if ctx.text.is_empty() {
            return None;
        }
        Some(LayeredLocator::capture(
            &href,
            self.spine,
            &ctx.text,
            char_offset,
            ctx.prior,
            ctx.total,
        ))
    }

    /// How far through the current unit the page on screen starts, in
    /// `0.0..=1.0` — the cheap position, from the unit's own text only
    /// (one parse per chapter, cached). What a host saves on every page
    /// turn; [`Session::layered_locator`] is the durable one.
    pub fn unit_fraction(&self) -> f64 {
        let Some(text) = self.cached_unit_text(self.spine) else {
            return 0.0;
        };
        let total = text.chars().count();
        if total == 0 {
            return 0.0;
        }
        (f64::from(self.current_offset()) / total as f64).clamp(0.0, 1.0)
    }

    /// How many locator-text chars a spine item has — the denominator a
    /// host needs to turn "40% of the way through chapter 3" (a stored
    /// fraction, a bookmark) into an offset for [`Session::goto`]. Parses
    /// that one unit's text (cached), not the whole book. `None` for a
    /// spine index the book does not have or a unit with no text layer.
    pub fn chapter_char_count(&self, spine: usize) -> Option<u64> {
        if spine >= self.book.publication().spine().len() {
            return None;
        }
        self.cached_unit_text(spine)
            .map(|text| text.chars().count() as u64)
    }

    /// Land on a stored layered locator.
    ///
    /// Resolves the way a restored library position does: the spine item
    /// is found by href first (so a re-ordered edition still lands in the
    /// right chapter) and by index only when no item has that href; then
    /// the exact offset is used if `same_edition` says the text is the one
    /// the locator was captured against, else the quote context is
    /// searched for and, failing that, the spine fraction is taken.
    /// `false` if no unit can be found at all.
    pub fn goto_layered(&mut self, target: &LayeredLocator, same_edition: bool) -> bool {
        let spine = {
            let publication = self.book.publication();
            let items = publication.spine();
            items
                .iter()
                .position(|item| item.href == target.spine_href)
                .or_else(|| (target.spine_index < items.len()).then_some(target.spine_index))
        };
        let Some(spine) = spine else {
            return false;
        };
        let href_matched = self
            .book
            .publication()
            .spine_item(spine)
            .map(|item| item.href == target.spine_href)
            .unwrap_or(false);
        let offset = match self.cached_unit_text(spine) {
            Some(text) => resolve_in_text(&text, target, same_edition && href_matched).offset(),
            None => 0,
        };
        self.goto(Locator::new(spine, offset))
    }
}
