//! What the session keeps and lets go of: the cache budget and LRU
//! eviction, wholesale release under memory pressure, and the suspend
//! lifecycle that closes the library.

use chapbook_layout::ChapterLayout;

use crate::Session;

impl Session {
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
        self.layouts.retain(|spine, _| *spine == pinned);
        self.layout_bytes.retain(|spine, _| *spine == pinned);
        self.images.retain(|spine, _| *spine == pinned);
        self.loaded_units.retain(|spine, _| *spine == pinned);
        self.placeholders.retain(|spine| *spine == pinned);
        self.links.retain(|spine, _| *spine == pinned);
        #[cfg(feature = "library")]
        self.resolved_highlights.retain(|spine, _| *spine == pinned);
        self.used_at.retain(|spine, _| *spine == pinned);
        self.unit_text_cache.replace(None);
        // The renderer's glyph-mask cache is the one cache with no other
        // release path; a fresh renderer starts it empty.
        self.renderer = chapbook_render_tinyskia::Renderer::new();
    }

    /// You are about to be stopped: persist, and let go of the database.
    ///
    /// Android's `onStop` is the only guaranteed callback there is and it
    /// has a time budget, so this saves the one piece of state that is
    /// written lazily — the reading position. Settings and annotations are
    /// already written when they change.
    ///
    /// iOS wants the same call for a second reason Android never raises.
    /// Bundled SQLite takes POSIX advisory locks, and an app still holding
    /// one on a file in a *shared* container when it suspends is killed by
    /// the watchdog with `0xdead10cc`. So this closes the connection rather
    /// than merely flushing it. A share extension or a widget means an app
    /// group, and an app group means a shared container, which is when that
    /// stops being hypothetical.
    ///
    /// Caches go too: a stopped app should not be holding decoded pages.
    ///
    /// The session stays usable. `onStop` is often followed by `onStart`
    /// with the process still alive, so the next thing that needs the
    /// library reopens it. What this does *not* do is stop the loader
    /// thread; a suspended session with a page still arriving will finish
    /// decoding it.
    pub fn suspend(&mut self) {
        self.save_position();
        self.release_caches();
        // Without a library there is no connection to let go of, and
        // nothing was lazily written that needs flushing first.
        #[cfg(feature = "library")]
        {
            self.library = None;
            self.suspended = true;
        }
    }

    /// The library, reopened if [`suspend`](Self::suspend) closed it.
    ///
    /// Every access goes through here, so "the database is shut because we
    /// were told to let go of it" and "this platform has no library" stay
    /// distinguishable — both are `None` in the field and only one should
    /// be retried.
    #[cfg(feature = "library")]
    pub(crate) fn library_mut(&mut self) -> Option<&mut chapbook_library::Library> {
        if self.suspended {
            self.suspended = false;
            if let Some(dir) = &self.library_dir {
                self.library = chapbook_library::Library::open(dir)
                    .map_err(|e| log::warn!("library unavailable after resume: {e}"))
                    .ok();
            }
        }
        self.library.as_mut()
    }

    /// Bytes the layout and image caches currently hold.
    ///
    /// Exact for decoded images, approximate for laid-out chapters — see
    /// `ChapterLayout::approx_bytes`. A host reporting memory, or deciding
    /// whether to lower the budget, wants this.
    pub fn cache_bytes(&self) -> usize {
        self.layout_bytes.values().sum::<usize>()
            + self.images.values().map(|i| i.bytes()).sum::<usize>()
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
        self.used_at.insert(spine, self.use_clock);
    }

    /// Cache a unit's layout, recording its byte estimate once —
    /// `cache_bytes()` runs inside the eviction loop and must not deep-walk
    /// every cached chapter per probe.
    pub(crate) fn cache_layout(&mut self, spine: usize, layout: ChapterLayout) {
        self.layout_bytes.insert(spine, layout.approx_bytes());
        self.layouts.insert(spine, layout);
    }

    pub(crate) fn drop_layout(&mut self, spine: usize) {
        self.layouts.remove(&spine);
        self.layout_bytes.remove(&spine);
    }

    pub(crate) fn clear_layouts(&mut self) {
        self.layouts.clear();
        self.layout_bytes.clear();
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
        while self.cache_bytes() > self.cache_budget {
            // Oldest use first; a unit with no recorded use is older still.
            let victim = self
                .layouts
                .keys()
                .chain(self.images.keys())
                .copied()
                .filter(|spine| *spine != pinned && Some(*spine) != keep)
                .min_by_key(|spine| self.used_at.get(spine).copied().unwrap_or(0));
            let Some(victim) = victim else {
                // Only the pinned unit is left. One page over budget beats
                // a session with nothing to show.
                return;
            };
            self.drop_layout(victim);
            self.images.remove(&victim);
            self.loaded_units.remove(&victim);
            self.placeholders.remove(&victim);
            self.used_at.remove(&victim);
            // Rebuildable side tables that ride along with a unit's layout;
            // without this they escape the budget the host set.
            self.links.remove(&victim);
            #[cfg(feature = "library")]
            self.resolved_highlights.remove(&victim);
        }
    }
}
