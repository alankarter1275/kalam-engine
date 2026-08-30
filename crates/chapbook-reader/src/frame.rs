//! The frame: what changed since the last one (intent and damage), and
//! the display list a shell rasterizes.

use chapbook_core::Rect;
#[cfg(feature = "library")]
use chapbook_core::Rgba;
use chapbook_paint::{Frame, FrameIntent, Selection};

use crate::Session;

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
    #[cfg(feature = "library")]
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
    #[cfg(any(feature = "library", feature = "_image-book"))]
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
    #[cfg(feature = "library")]
    fn range_damage(&self, start: u32, end: u32) -> Option<Rect> {
        let page = self.layout(self.spine)?.pages.get(self.page)?;
        page.rects_for_range(start, end)
            .into_iter()
            .reduce(|damage, rect| damage.union(&rect))
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
        // has laid out.
        // Both of these are dropped rather than deferred once the reader
        // has left the unit they were captured in. A pending landing is a
        // statement about one unit, and the reader having navigated away
        // supersedes it — carrying it along would resolve an offset from
        // one chapter against the pages of another.
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
        let (spine, page_idx) = (self.spine, self.page);
        // Stored highlights first, the live selection on top of them.
        // Without a library nothing is stored, so the live selection is
        // the only thing that paints.
        #[cfg(not(feature = "library"))]
        let mut selections: Vec<Selection> = Vec::new();
        #[cfg(feature = "library")]
        let highlight_color = self.settings.theme.highlight();
        #[cfg(feature = "library")]
        let mut selections: Vec<Selection> = self
            .highlights(spine)
            .iter()
            .map(|h| Selection {
                start: h.start,
                end: h.end,
                // A stored color keeps the theme's transparency unless it
                // states its own; an unparseable one falls back rather
                // than vanishing.
                color: h
                    .color
                    .as_deref()
                    .and_then(|hex| Rgba::from_hex(hex, highlight_color.a))
                    .unwrap_or(highlight_color),
            })
            .collect();
        if let Some((start, end)) = self.selected_range() {
            selections.push(Selection {
                start,
                end,
                color: self.settings.theme.selection(),
            });
        }
        let background = self.settings.theme.background();
        let page_count = self.layout_unit(spine).map_or(0, |l| l.pages.len());
        if page_count == 0 {
            return None;
        }
        self.page = self.page.min(page_count - 1);
        let page_idx = page_idx.min(page_count - 1);
        let layout = self.layout(spine)?;
        let page = layout.pages.get(page_idx)?;
        Some(chapbook_paint::build_display_list(
            page,
            background,
            &selections,
        ))
    }
}
