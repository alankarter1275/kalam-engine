//! What the session keeps and lets go of: the cache budget and LRU
//! eviction, wholesale release under memory pressure, and the suspend
//! lifecycle.

use chapbook_layout::ChapterLayout;
use chapbook_paint::ImageStore;

use crate::open::OpenBook;
use crate::{Session, UnitState};

impl Session {
    /// A unit's cached state, if any facet of it exists.
    pub(crate) fn unit(&self, spine: usize) -> Option<&UnitState> {
        self.units.get(&spine)
    }

    /// A unit's cached state, created empty if absent. Prefer the direct
    /// `self.units.entry(..)` form where another field of `self` is
    /// borrowed across the call — a method borrows all of `self`.
    pub(crate) fn unit_mut(&mut self, spine: usize) -> &mut UnitState {
        self.units.entry(spine).or_default()
    }

    /// A unit's cached layout, if it is laid out.
    pub(crate) fn layout(&self, spine: usize) -> Option<&ChapterLayout> {
        self.units.get(&spine)?.layout.as_ref()
    }

    /// A unit's image store, or the shared empty one — so callers can
    /// hold a reference either way.
    pub(crate) fn unit_images(&self, spine: usize) -> &ImageStore {
        self.units
            .get(&spine)
            .and_then(|unit| unit.images.as_ref())
            .unwrap_or(&self.empty_images)
    }

    /// Give back everything that can be rebuilt, keeping only the unit on
    /// screen.
    ///
    /// The lever `onTrimMemory` and `didReceiveMemoryWarning` had nothing
    /// to call. Distinct from lowering the budget: a budget is a
    /// steady-state cap, this is "right now, as much as you can". Nothing
    /// dropped here is authoritative — pages are re-read and re-decoded,
    /// chapters laid out again — so the cost is a slower next page turn,
    /// not a lost anything.
    pub fn release_caches(&mut self) {
        let pinned = self.spine;
        self.units.retain(|spine, _| *spine == pinned);
        self.unit_text_cache.replace(None);
        // kalam: layouts are gone, so a scrolling shell's strip is stale
        // — though only its measured slots, and re-asking rebuilds them.
        self.layout_generation += 1;
        // The renderer's glyph-mask cache is the one cache with no other
        // release path; a fresh renderer starts it empty.
        self.renderer = chapbook_render_tinyskia::Renderer::new();
    }

    /// The host is about to be stopped. Caches go: a stopped app should
    /// not be holding decoded pages.
    ///
    /// kalam: upstream also wrote the position to its library and closed
    /// the database here. Neither exists now; a host saves the position
    /// it got from [`Session::layered_locator`] itself.
    ///
    /// The session stays usable. `onStop` is often followed by `onStart`
    /// with the process still alive. What this does *not* do is stop the
    /// loader thread; a suspended session with a page still arriving will
    /// finish decoding it.
    pub fn suspend(&mut self) {
        self.release_caches();
    }

    /// Bytes the layout and image caches currently hold.
    ///
    /// Exact for decoded images, approximate for laid-out chapters — see
    /// `ChapterLayout::approx_bytes`. A host reporting memory, or deciding
    /// whether to lower the budget, wants this.
    pub fn cache_bytes(&self) -> usize {
        self.units
            .values()
            .map(|unit| unit.layout_bytes + unit.images.as_ref().map_or(0, ImageStore::bytes))
            .sum()
    }

    /// What the caches are allowed to hold.
    pub fn cache_budget(&self) -> usize {
        self.cache_budget
    }

    /// Change the cap, evicting immediately if the caches are now over it.
    ///
    /// Runtime rather than construction-only because memory pressure is a
    /// runtime event: a host told to trim lowers this and the session
    /// obeys at once.
    pub fn set_cache_budget(&mut self, bytes: usize) {
        self.cache_budget = bytes;
        self.evict_to_budget();
    }

    /// Note that a unit was just used, for eviction order.
    pub(crate) fn touch(&mut self, spine: usize) {
        self.use_clock += 1;
        let clock = self.use_clock;
        self.unit_mut(spine).used_at = clock;
    }

    /// Cache a unit's layout, recording its byte estimate once —
    /// `cache_bytes()` runs inside the eviction loop and must not deep-walk
    /// every cached chapter per probe.
    pub(crate) fn cache_layout(&mut self, spine: usize, layout: ChapterLayout) {
        let unit = self.unit_mut(spine);
        unit.layout_bytes = layout.approx_bytes();
        unit.layout = Some(layout);
    }

    /// Drop what a metrics or settings change invalidates, unit by unit:
    /// layouts and placeholder pages always, and text books' image stores,
    /// which are rebuilt per layout. Everything else is a fact about the
    /// *file*, not the geometry — links are locator-space, image-book
    /// pixels are metrics-independent — and survives.
    pub(crate) fn drop_metrics_dependent(&mut self) {
        self.layout_generation += 1;
        let epub = matches!(self.book, OpenBook::Epub(_));
        for unit in self.units.values_mut() {
            unit.layout = None;
            unit.layout_bytes = 0;
            unit.placeholder = false;
            if epub {
                unit.images = None;
            }
        }
    }

    /// Drop least-recently-used units until the caches fit the budget.
    ///
    /// **The current unit is never evicted.** Everything else is fair game,
    /// which is safe because none of it is authoritative: a comic page is
    /// re-read from the archive and re-decoded, a chapter is laid out
    /// again. The caches have always been reconstructible; they had simply
    /// never been treated as caches, and were cleared only wholesale — and
    /// for image books, not even then.
    ///
    /// A miss costs a re-decode, measured at 77-120 ms for a 1600x2400
    /// comic page, so a budget below a few pages trades a memory problem
    /// for a paging-back problem. The default is sized well clear of that.
    fn evict_to_budget(&mut self) {
        self.evict_keeping(None);
    }

    /// Evict, additionally sparing `keep` — the unit a caller is in the
    /// middle of building, which would otherwise be a candidate the moment
    /// it is not the current one (a prefetch, a search).
    pub(crate) fn evict_keeping(&mut self, keep: Option<usize>) {
        let pinned = self.spine;
        // kalam: a scrolling shell's visible units are pinned too.
        let visible = self.pinned_units.clone();
        while self.cache_bytes() > self.cache_budget {
            // Oldest use first, among units actually holding memory; a
            // unit with no recorded use is older still (`used_at` of 0).
            let victim = self
                .units
                .iter()
                .filter(|(spine, unit)| {
                    **spine != pinned
                        && Some(**spine) != keep
                        && !visible.contains(spine)
                        && (unit.layout.is_some() || unit.images.is_some())
                })
                .min_by_key(|(_, unit)| unit.used_at)
                .map(|(spine, _)| *spine);
            let Some(victim) = victim else {
                // Only the pinned unit is left. One page over budget beats
                // a session with nothing to show.
                return;
            };
            // Every facet goes together — the point of `UnitState`. The
            // side tables that ride along with a layout used to be listed
            // here one by one, and forgetting one let it escape the budget
            // the host set.
            self.units.remove(&victim);
        }
    }
}
