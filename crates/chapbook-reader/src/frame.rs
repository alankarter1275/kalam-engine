//! The frame: what changed since the last one (intent and damage), and
//! the display list a shell rasterizes.

use chapbook_core::{Rect, Rgba};
use chapbook_paint::{Frame, FrameIntent, Selection};

use crate::{Highlight, Session};

/// Where changes have landed since the last frame was taken.
///
/// Kept apart from the pending [`FrameIntent`] on purpose. Intent is
/// ordered by how disturbing a change is and collapses to the strongest
/// one; damage is a union and collapses to "everywhere" the moment any
/// change cannot say where it went. Folding the two together made damage
/// hostage to that ordering, so a highlight discarded the region a live
/// selection had already named.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PendingDamage {
    /// Some change since the last frame could not name its region.
    unstated: bool,
    /// Union of the regions that could, in page coordinates.
    region: Option<Rect>,
}

impl Session {
    /// Record what changed. The strongest intent since the last frame is
    /// the one that describes it, so this never downgrades.
    ///
    /// A change recorded here does not say where it landed, so the frame
    /// falls back to a full repaint. Use [`Session::mark_range`] when the
    /// change is confined to a span of text.
    pub(crate) fn mark(&mut self, intent: FrameIntent) {
        self.pending = self.pending.max(intent);
        // A selection is the one exception: it states its region at frame
        // time instead, from the difference between what is painted and
        // what is selected now.
        if intent != FrameIntent::Selection {
            self.pending_damage.unstated = true;
        }
    }

    /// Record a change confined to a locator range on the current page.
    ///
    /// Damage accumulates independently of the intent ordering. A
    /// highlight landing while a selection is live must not lose its
    /// region just because `Annotation` outranks `Selection` — both name
    /// where they changed, so the frame reports the union of the two.
    pub(crate) fn mark_range(&mut self, intent: FrameIntent, start: u32, end: u32) {
        match self.range_damage(start, end) {
            Some(region) => self.mark_rect(intent, region),
            // The range is on another page, so it disturbs nothing here —
            // which is a stated region of nothing, not an unstated one.
            None => self.pending = self.pending.max(intent),
        }
    }

    /// Record a change confined to a region the engine already knows in
    /// page coordinates, rather than one it has to derive from locators.
    // Highlights state a region, and so does a page image arriving.
    pub(crate) fn mark_rect(&mut self, intent: FrameIntent, region: Rect) {
        self.pending = self.pending.max(intent);
        if self.pending_damage.unstated {
            return;
        }
        self.pending_damage.region = Some(match self.pending_damage.region {
            Some(existing) => existing.union(&region),
            None => region,
        });
    }

    /// The area a locator range covers on the current page, or `None` when
    /// it lies on another page and so disturbs nothing here.
    fn range_damage(&self, start: u32, end: u32) -> Option<Rect> {
        let page = self.layout(self.spine)?.pages.get(self.page)?;
        let region = page
            .rects_for_range(start, end)
            .into_iter()
            .reduce(|damage, rect| damage.union(&rect))?;
        // Into the space the display list is actually in. `rects_for_range`
        // answers in fit-page coordinates — the space every consumer of the
        // text surface agrees on — but `apply_view` has scaled the ops a
        // backend will paint, so on a zoomed page the two disagree by
        // exactly the view transform.
        //
        // It matters for one combination and it is a real one: a PDF is an
        // image book, so it zooms, and it has a text layer, so it selects.
        // A selection is also the one intent that does *not* fall back to
        // "the whole page", by design — which means an unmapped rect here
        // is not merely imprecise, it names a region the change did not
        // touch. A windowed shell repainting everything never notices; a
        // panel doing partial updates leaves stale pixels, which is the one
        // failure mode damage must not have.
        Some(self.view_rect(region))
    }

    /// The current page as paint-neutral display ops, plus what changed
    /// since the last frame — the backend contract from
    /// `docs/ARCHITECTURE.md`, for shells that rasterize themselves: a GPU
    /// backend, a platform canvas, an e-ink panel, an exporter.
    /// [`Session::render`] is this plus the bundled CPU rasterizer.
    ///
    /// `Image` ops carry keys into the unit's [`Session::image_store`],
    /// not pixels, and glyph runs name faces in its font database, so a
    /// shell needs [`Session::paint_resources`] too.
    ///
    /// Taking a frame consumes the change record: the next one reports
    /// [`FrameIntent::Repaint`] until something else moves.
    ///
    /// `None` until metrics are set, and while the unit has no page — an
    /// image book still loading, or a unit that failed to load.
    pub fn frame(&mut self) -> Option<Frame> {
        let list = self.page_display_list()?;
        let intent = std::mem::take(&mut self.pending);
        let damage = self.damage_for();
        self.pending_damage = PendingDamage::default();
        self.painted_selection = self.selected_range();
        Some(Frame {
            list,
            intent,
            damage,
        })
    }

    /// The region a frame disturbs, when it is cheaper to state than to
    /// repaint — `None` meaning "assume the whole page", which is always
    /// correct and sometimes wasteful.
    ///
    /// This deliberately does not consult the intent. Intent answers "how
    /// disturbing is this change", damage answers "where is it", and
    /// tying the second to the ordering of the first meant a highlight
    /// repainted the entire page because `Annotation` outranked the
    /// `Selection` that had already named its lines.
    fn damage_for(&self) -> Option<Rect> {
        if self.pending_damage.unstated {
            return None;
        }
        let mut damage = self.pending_damage.region;
        // A moved selection disturbs both where it was and where it is.
        let current = self.selected_range();
        if current != self.painted_selection {
            let page = self.layout(self.spine)?.pages.get(self.page)?;
            let moved = [self.painted_selection, current]
                .into_iter()
                .flatten()
                .flat_map(|(start, end)| page.rects_for_range(start, end))
                .reduce(|damage, rect| damage.union(&rect));
            damage = match (damage, moved) {
                (Some(a), Some(b)) => Some(a.union(&b)),
                (a, b) => a.or(b),
            };
        }
        damage
    }

    fn page_display_list(&mut self) -> Option<chapbook_paint::DisplayList> {
        self.metrics?;
        // Resolve a restored offset, or a jump's anchor, once the unit
        // has laid out. kalam: shared with `settle()`, the scroll shell's
        // frameless way of landing the same jumps.
        self.land_pending();
        let (spine, page_idx) = (self.spine, self.page);
        // Stored highlights first, the live selection on top of them.
        // kalam: colours come from the effective palette (a shell's exact
        // colours if it set them, else the theme's presets). The
        // highlights are the host's (`host_highlights.rs`).
        let highlight_color = self.settings.palette().highlight;
        // A stored color keeps the theme's transparency unless it states
        // its own; an unparseable one falls back rather than vanishing.
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
        if let Some((start, end)) = self.selected_range() {
            selections.push(Selection {
                start,
                end,
                color: self.settings.palette().selection,
            });
        }
        let background = self.settings.palette().background;
        let page_count = self.layout_unit(spine).map_or(0, |l| l.pages.len());
        if page_count == 0 {
            return None;
        }
        self.page = self.page.min(page_count - 1);
        let page_idx = page_idx.min(page_count - 1);
        let layout = self.layout(spine)?;
        let page = layout.pages.get(page_idx)?;
        let mut list = chapbook_paint::build_display_list(page, background, &selections);
        // The image-book zoom view, applied here so every backend and
        // both of frame()'s doors see the same pixels-to-be.
        self.apply_view(&mut list);
        Some(list)
    }
}
