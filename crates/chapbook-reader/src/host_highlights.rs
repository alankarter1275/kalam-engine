//! kalam: highlights a host keeps in its own database.
//!
//! The library path (`annotations.rs`) stores marks itself and hands out
//! its own ids. A host with its own records (Kalam's `annotations` table)
//! wants the reverse: it stores the mark, it owns the id, and the session
//! only needs to *paint* the span and re-find it after the text moved.
//! That is all this module is — a list of host-owned highlights held in
//! memory for the life of the session, resolved into each unit's locator
//! space on demand, exactly the way stored annotations are resolved.
//!
//! Nothing here touches a database. A host that opens a book calls
//! [`Session::set_host_highlights`] with what it has on file; when the
//! reader marks a passage, the host takes the two [`LayeredLocator`]s from
//! [`Session::layered_locator_at`], writes its row, and calls
//! [`Session::show_host_highlight`] with its own row id.

use chapbook_core::{resolve_in_text, LayeredLocator, Locator};
use chapbook_paint::FrameIntent;

use crate::{Highlight, Session};

/// A highlight as the host stores it: two layered locators and the
/// host's own id. The text is optional and only carried back out in
/// [`Highlight::text`], for a list.
#[derive(Debug, Clone, PartialEq)]
pub struct HostHighlight {
    /// The host's id — its database row, not anything of the session's.
    pub id: i64,
    pub start: LayeredLocator,
    pub end: LayeredLocator,
    /// `#rrggbb` or `#rrggbbaa`; `None` paints in the theme's colour.
    pub color: Option<String>,
    pub text: Option<String>,
}

impl Session {
    /// Replace the host's highlights for this book — what the host has on
    /// file, at open or after a reload. Every unit's resolved cache is
    /// dropped and rebuilt lazily as units are painted.
    pub fn set_host_highlights(&mut self, highlights: Vec<HostHighlight>) {
        self.host_highlights = highlights;
        self.forget_resolved_host_highlights();
        self.mark(FrameIntent::Annotation);
    }

    /// Add one highlight the host just stored. Its span is resolved and
    /// painted at once if it lands in a unit already laid out.
    pub fn show_host_highlight(&mut self, highlight: HostHighlight) {
        self.hide_host_highlight(highlight.id);
        let spine = self.unit_for(&highlight.start);
        self.host_highlights.push(highlight);
        if let Some(spine) = spine {
            if let Some(unit) = self.units.get_mut(&spine) {
                unit.resolved_host_highlights = None;
            }
        }
        self.mark(FrameIntent::Annotation);
    }

    /// Change a shown highlight's colour. `None` returns it to the theme's.
    pub fn recolor_host_highlight(&mut self, id: i64, color: Option<&str>) {
        let color = color.map(str::to_string);
        for highlight in self.host_highlights.iter_mut().filter(|h| h.id == id) {
            highlight.color = color.clone();
        }
        for resolved in self
            .units
            .values_mut()
            .filter_map(|unit| unit.resolved_host_highlights.as_mut())
        {
            for highlight in resolved.iter_mut().filter(|h| h.id == id) {
                highlight.color = color.clone();
            }
        }
        self.mark(FrameIntent::Annotation);
    }

    /// Stop painting a highlight. The host has already deleted its row.
    pub fn hide_host_highlight(&mut self, id: i64) {
        self.host_highlights.retain(|h| h.id != id);
        for resolved in self
            .units
            .values_mut()
            .filter_map(|unit| unit.resolved_host_highlights.as_mut())
        {
            resolved.retain(|h| h.id != id);
        }
        self.mark(FrameIntent::Annotation);
    }

    /// The host's highlights that paint in `spine`, resolved into its
    /// locator space and cached (that space does not move under relayout,
    /// so a font-size change costs nothing here). Empty while the unit's
    /// text is unavailable.
    pub fn host_highlights(&mut self, spine: usize) -> &[Highlight] {
        let attempted = self
            .unit(spine)
            .is_some_and(|unit| unit.resolved_host_highlights.is_some());
        if !attempted {
            let any_here = self
                .host_highlights
                .iter()
                .any(|h| self.unit_for(&h.start) == Some(spine));
            let resolved = if any_here {
                let Some(text) = self.cached_unit_text(spine) else {
                    return &[];
                };
                self.resolve_host_highlights(spine, &text)
            } else {
                Vec::new()
            };
            self.unit_mut(spine).resolved_host_highlights = Some(resolved);
        }
        self.unit(spine)
            .and_then(|unit| unit.resolved_host_highlights.as_deref())
            .unwrap_or(&[])
    }

    /// The host's highlight under a point in panel coordinates — for a tap
    /// on a marked passage to mean "this one". Only inside the marked
    /// text, like a link.
    pub fn host_highlight_at(&mut self, x: f32, y: f32) -> Option<i64> {
        let (px, py) = self.content_point(x, y);
        let (spine, page) = (self.spine, self.page);
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(chapbook_core::Point::new(px, py))?;
        self.host_highlights(spine)
            .iter()
            .find(|h| offset >= h.start && offset < h.end)
            .map(|h| h.id)
    }

    /// Jump to the start of one of the host's highlights. `false` if it is
    /// not shown or its unit cannot be read.
    pub fn goto_host_highlight(&mut self, id: i64) -> bool {
        let Some(start) = self
            .host_highlights
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.start.clone())
        else {
            return false;
        };
        let Some(spine) = self.unit_for(&start) else {
            return false;
        };
        let Some(text) = self.cached_unit_text(spine) else {
            return false;
        };
        let trusted = self.href_matches(spine, &start);
        let offset = resolve_in_text(&text, &start, trusted).offset();
        self.goto(Locator::new(spine, offset))
    }

    /// Which unit a stored locator lands in: by href where the book still
    /// has that item (a re-ordered edition still finds its chapter), by
    /// index otherwise, `None` if neither exists.
    pub(crate) fn unit_for(&self, locator: &LayeredLocator) -> Option<usize> {
        let items = self.book.publication().spine();
        items
            .iter()
            .position(|item| item.href == locator.spine_href)
            .or_else(|| (locator.spine_index < items.len()).then_some(locator.spine_index))
    }

    /// Whether `spine` is the very item the locator was captured in, which
    /// is when its exact offset can be trusted. The host has no edition
    /// fingerprint to offer, so this is the whole test: the same href in a
    /// re-imported copy of the same file resolves exactly, and if the file
    /// changed underneath, the offset is bounds-checked and the quote
    /// context is the fallback the way it always was.
    fn href_matches(&self, spine: usize, locator: &LayeredLocator) -> bool {
        self.book
            .publication()
            .spine_item(spine)
            .map(|item| item.href == locator.spine_href)
            .unwrap_or(false)
    }

    fn resolve_host_highlights(&self, spine: usize, text: &str) -> Vec<Highlight> {
        self.host_highlights
            .iter()
            .filter(|h| self.unit_for(&h.start) == Some(spine))
            .filter_map(|h| {
                let trusted = self.href_matches(spine, &h.start);
                let start = resolve_in_text(text, &h.start, trusted).offset();
                let end = resolve_in_text(text, &h.end, trusted).offset();
                // A range that collapsed under re-anchoring has nothing
                // left to paint.
                (end > start).then(|| Highlight {
                    id: h.id,
                    spine,
                    start,
                    end,
                    text: h.text.clone(),
                    color: h.color.clone(),
                })
            })
            .collect()
    }

    fn forget_resolved_host_highlights(&mut self) {
        for unit in self.units.values_mut() {
            unit.resolved_host_highlights = None;
        }
    }
}
