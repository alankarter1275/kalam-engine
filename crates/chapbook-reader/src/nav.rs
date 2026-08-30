//! Moving through the book: page and unit turns, jumps and the back
//! stack, links, applied reader settings, and the input-action seam.

use chapbook_core::{
    Action, ActionOutcome, Locator, Point, ReadingDirection, ReadingSettings, TocEntry,
};
use chapbook_paint::FrameIntent;

use crate::{Session, SettingsScope, FONT_STEP_PX};

impl Session {
    // ---- Navigation ----

    /// Turn forward one page, crossing into the next unit at the end of
    /// this one. Returns whether the position moved, which is the answer a
    /// shell's loop wants and the one it cannot reliably derive from
    /// `page()` — see [`Position`].
    pub fn next_page(&mut self) -> bool {
        let before = self.position();
        let count = self.page_count();
        if self.page + 1 < count {
            self.page += 1;
            self.mark(FrameIntent::PageTurn);
        } else if self.spine + 1 < self.spine_len() {
            self.spine += 1;
            self.page = 0;
            self.mark(FrameIntent::UnitChange);
        }
        self.selection = None;
        self.position() != before
    }

    /// Turn back one page. Returns whether the position moved.
    pub fn prev_page(&mut self) -> bool {
        let before = self.position();
        if self.page > 0 {
            self.page -= 1;
            self.mark(FrameIntent::PageTurn);
        } else if self.spine > 0 {
            self.spine -= 1;
            let spine = self.spine;
            self.page = self
                .layout_unit(spine)
                .map_or(0, |l| l.pages.len())
                .saturating_sub(1);
            self.mark(FrameIntent::UnitChange);
        }
        self.selection = None;
        self.position() != before
    }

    /// Skip to the start of the next unit. Returns whether it moved.
    pub fn next_unit(&mut self) -> bool {
        let before = self.position();
        if self.spine + 1 < self.spine_len() {
            self.spine += 1;
            self.page = 0;
            self.selection = None;
            self.mark(FrameIntent::UnitChange);
        }
        self.position() != before
    }

    /// Skip to the start of the previous unit. Returns whether it moved.
    pub fn prev_unit(&mut self) -> bool {
        let before = self.position();
        if self.spine > 0 {
            self.spine -= 1;
            self.page = 0;
            self.selection = None;
            self.mark(FrameIntent::UnitChange);
        }
        self.position() != before
    }

    // ---- Settings ----

    /// Apply settings and remember them. Covers every field, including the
    /// three no shell could reach before: line height, justification, and
    /// whether publisher styles apply.
    ///
    /// Settings persist to the library, so font size survives a restart.
    /// `Global` is the reader's default from here on; `ThisBook` is an
    /// override that outlives later changes to the default.
    pub fn set_settings(&mut self, settings: ReadingSettings, scope: SettingsScope) {
        let changed = self.settings != settings;
        self.settings = settings;
        if changed {
            self.relayout_keeping_position();
        }
        self.persist_settings(scope);
    }

    /// Drop this book's override so it follows the reader's default again,
    /// applying that default now.
    #[cfg(feature = "library")]
    pub fn clear_book_settings(&mut self) {
        let book_id = self.book_id;
        let (Some(library), Some(id)) = (self.library_mut(), book_id) else {
            return;
        };
        if let Err(e) = library.clear_reading_settings(id) {
            log::error!("failed to clear book settings: {e}");
            return;
        }
        let settings = library.effective_settings(None);
        self.set_settings(settings, SettingsScope::Global);
    }

    /// Step the base font size, keeping the reader's place. A convenience
    /// over [`Session::set_settings`]; persists globally.
    pub fn adjust_font(&mut self, delta: f32) {
        let mut settings = self.settings.clone();
        settings.base_font_px = (settings.base_font_px + delta).clamp(10.0, 40.0);
        self.set_settings(settings, SettingsScope::Global);
    }

    /// Cycle light → sepia → dark. A convenience over
    /// [`Session::set_settings`]; persists globally.
    pub fn cycle_theme(&mut self) {
        let mut settings = self.settings.clone();
        settings.theme = settings.theme.cycle();
        self.set_settings(settings, SettingsScope::Global);
    }

    fn persist_settings(&mut self, scope: SettingsScope) {
        // Settings still apply; there is simply nowhere to write them
        // down, so they last as long as the session does.
        #[cfg(not(feature = "library"))]
        let _ = scope;
        #[cfg(feature = "library")]
        {
            let book_id = self.book_id;
            let settings = self.settings.clone();
            let Some(library) = self.library_mut() else {
                return;
            };
            let target = match scope {
                SettingsScope::Global => None,
                // No library record, nothing to hang an override on.
                SettingsScope::ThisBook => match book_id {
                    Some(id) => Some(id),
                    None => return,
                },
            };
            if let Err(e) = library.set_reading_settings(target, &settings) {
                log::error!("failed to save settings: {e}");
            }
        }
    }

    // ---- Input ----

    /// Which edge this book reads from, for [`TapZones`].
    ///
    /// Ask the session rather than reaching for [`TapZones::default`],
    /// which is `Ltr` and has no way to know better. The book declares
    /// this — EPUB's `page-progression-direction` — so a shell that
    /// picks for itself picks `Ltr` on every platform it ships to, and
    /// an RTL book paged the wrong way is unreadable rather than merely
    /// unfamiliar.
    ///
    /// ```no_run
    /// # use chapbook_core::TapZones;
    /// # fn f(session: &chapbook_reader::Session) {
    /// let zones = TapZones::new(session.reading_direction());
    /// # }
    /// ```
    ///
    /// [`TapZones`]: chapbook_core::TapZones
    /// [`TapZones::default`]: chapbook_core::TapZones::default
    pub fn reading_direction(&self) -> ReadingDirection {
        self.book.publication().reading_direction()
    }

    /// Apply a reader intent from [`chapbook_core::input`].
    ///
    /// This is the second half of the input seam: shells translate native
    /// events into an [`Action`] and the engine applies it, so a tap zone
    /// or a key binding is written once instead of once per platform.
    ///
    /// The [`ActionOutcome`] answers two questions rather than one,
    /// because a shell needs both and can derive neither from the other.
    /// See its own documentation for what guessing costs.
    pub fn apply(&mut self, action: Action) -> ActionOutcome {
        let moved = |did: bool| {
            if did {
                ActionOutcome::Changed
            } else {
                ActionOutcome::Unchanged
            }
        };
        match action {
            Action::NextPage => moved(self.next_page()),
            Action::PrevPage => moved(self.prev_page()),
            Action::NextUnit => moved(self.next_unit()),
            Action::PrevUnit => moved(self.prev_unit()),
            // The bottom of the back stack is where the platform's own
            // Back takes over, on both mobile targets. Declining it here
            // is what lets a shell forward the gesture unconditionally
            // instead of shadowing the history to know when not to.
            Action::Back => {
                if self.back() {
                    ActionOutcome::Changed
                } else {
                    ActionOutcome::Unhandled
                }
            }
            // `adjust_font` clamps, so at either stop this correctly says
            // nothing moved rather than asking for a redraw of the same
            // page at the same size. The key is consumed either way.
            Action::FontUp | Action::FontDown => {
                let before = self.settings.base_font_px;
                let step = if action == Action::FontUp {
                    FONT_STEP_PX
                } else {
                    -FONT_STEP_PX
                };
                self.adjust_font(step);
                moved(self.settings.base_font_px != before)
            }
            Action::CycleTheme => {
                self.cycle_theme();
                ActionOutcome::Changed
            }
            // A reader's chrome belongs to the shell: there is nothing
            // here to toggle, and the shell that bound this wants the
            // event back.
            Action::ToggleMenu => ActionOutcome::Unhandled,
            // `Action` is `#[non_exhaustive]`. An intent this engine has
            // no verb for is one the shell may still want to act on.
            _ => ActionOutcome::Unhandled,
        }
    }

    fn relayout_keeping_position(&mut self) {
        let locator = self.current_offset();
        self.drop_metrics_dependent();
        // Glyph masks are keyed by size; a relayout that changed the font
        // scale would otherwise leave the old sizes' masks resident.
        self.renderer = chapbook_render_tinyskia::Renderer::new();
        let spine = self.spine;
        if let Some(layout) = self.layout_unit(spine) {
            self.page = layout.page_of(locator);
        }
        self.mark(FrameIntent::Relayout);
    }

    // ---- Navigation ----

    /// The book's table of contents, for a shell that offers one.
    pub fn toc(&self) -> &[TocEntry] {
        self.book.publication().toc()
    }

    /// Where the reader is now, as a locator.
    pub fn locator(&self) -> Locator {
        Locator::new(self.spine, self.current_offset())
    }

    /// Jump to a locator, remembering where we came from. `false` if the
    /// spine index doesn't exist.
    pub fn goto(&mut self, target: Locator) -> bool {
        self.jump(target.spine_index, Some(target.char_offset), None)
    }

    /// Jump to an element `id` within a unit — a TOC fragment or a
    /// footnote. `false` if the spine index doesn't exist; a fragment that
    /// turns out not to be in the unit lands at its start.
    pub fn goto_anchor(&mut self, spine_index: usize, fragment: &str) -> bool {
        self.jump(spine_index, None, Some(fragment.to_string()))
    }

    /// Jump to a TOC entry, by spine index where the entry has one and by
    /// href otherwise.
    pub fn goto_toc(&mut self, entry: &TocEntry) -> bool {
        let Some(spine) = entry
            .spine_index
            .or_else(|| entry.href.as_deref().and_then(|h| self.spine_index_of(h)))
        else {
            return false;
        };
        match &entry.fragment {
            Some(fragment) => self.goto_anchor(spine, fragment),
            None => self.goto(Locator::chapter_start(spine)),
        }
    }

    /// The link under a point in panel coordinates, as written in the
    /// document. Only inside the link's own text: pressing the margin
    /// beside a link is not pressing the link.
    pub fn link_at(&mut self, x: f32, y: f32) -> Option<String> {
        let (px, py) = self.metrics.map_or((x, y), |m| m.panel_to_page(x, y));
        let (spine, page) = (self.spine, self.page);
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(px, py))?;
        self.unit(spine)?
            .links
            .as_ref()?
            .iter()
            .find(|link| offset >= link.start && offset < link.end)
            .map(|link| link.href.clone())
    }

    /// Follow a document-internal link. External links (anything with a
    /// scheme) are a shell decision, not a reading position, so they
    /// return `false` untouched.
    pub fn follow_link(&mut self, href: &str) -> bool {
        if href.contains("://") || href.starts_with("mailto:") {
            return false;
        }
        let (path, fragment) = match href.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment.to_string())),
            None => (href, None),
        };
        // An empty path is a jump within this unit.
        let spine = if path.is_empty() {
            self.spine
        } else {
            let Ok(item) = self.book.publication().spine_item(self.spine) else {
                return false;
            };
            let resolved = chapbook_epub::resolve_href(&item.href.clone(), path);
            match self.spine_index_of(&resolved) {
                Some(spine) => spine,
                None => return false,
            }
        };
        match fragment {
            Some(fragment) => self.goto_anchor(spine, &fragment),
            None => self.goto(Locator::chapter_start(spine)),
        }
    }

    /// Return to where the last jump started. `false` with nothing to go
    /// back to.
    pub fn back(&mut self) -> bool {
        let Some(target) = self.back_stack.pop() else {
            return false;
        };
        self.land(target.spine_index, Some(target.char_offset), None);
        true
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    /// Spine index of a container-root path, tolerating the leading slash
    /// EPUB manifests may or may not carry.
    fn spine_index_of(&self, href: &str) -> Option<usize> {
        let want = href
            .split('#')
            .next()
            .unwrap_or(href)
            .trim_start_matches('/');
        self.book
            .publication()
            .spine()
            .iter()
            .position(|item| item.href.trim_start_matches('/') == want)
    }

    /// A jump: remember where we were, then land.
    fn jump(&mut self, spine: usize, offset: Option<u32>, anchor: Option<String>) -> bool {
        if spine >= self.book.publication().spine().len() {
            return false;
        }
        let from = self.locator();
        self.land(spine, offset, anchor);
        // Cap the trail: a reader chasing footnotes shouldn't grow it
        // without bound.
        self.back_stack.push(from);
        if self.back_stack.len() > 64 {
            self.back_stack.remove(0);
        }
        true
    }

    fn land(&mut self, spine: usize, offset: Option<u32>, anchor: Option<String>) {
        let same_unit = spine == self.spine;
        self.spine = spine;
        self.page = 0;
        self.pending_offset = offset.map(|offset| (spine, offset));
        self.pending_anchor = anchor.map(|anchor| (spine, anchor));
        self.selection = None;
        self.mark(if same_unit {
            FrameIntent::PageTurn
        } else {
            FrameIntent::UnitChange
        });
    }
}
