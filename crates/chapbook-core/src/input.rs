//! What a shell's native events *mean* to a reader.
//!
//! Deliberately not a gesture recognizer. Android, iOS and GTK each ship
//! one, each is better than anything written here, and each carries
//! conventions an app is judged on — fling velocity, edge slop, the
//! long-press timeout a user set in accessibility settings. A shell that
//! fights its platform's recognizer produces something that feels foreign,
//! which is the exact failure a portability layer exists to prevent.
//!
//! What is left once the recognizer is somebody else's is the part that is
//! genuinely engine knowledge, and it is all pure: an [`Action`] vocabulary
//! for shells to translate into, a [`TapZones`] policy that knows which
//! edge a page turn comes from, and a [`KeyMap`] that already knows a Kobo
//! has physical page-turn buttons. No event loop, no I/O, no timers.
//!
//! See `docs/FFI.md`, *Input, lifecycle and power*.

use crate::page::PageMetrics;

/// A reader intent, produced by a shell and applied by the engine.
///
/// This is the seam: shells translate native events into actions rather
/// than calling session verbs by hand, which is what lets tap zones and
/// key bindings be shared without taking a platform's gesture engine away.
///
/// `ToggleMenu` is the one variant the engine cannot apply for itself — a
/// reader's chrome belongs to the shell — and it is here because deciding
/// *that the middle of the page opens the menu* is the same policy
/// decision as deciding the other two thirds turn pages.
///
/// `#[non_exhaustive]`, because this set closes only if every reader
/// intent is guessed up front — bookmarks, search, a jump to the table
/// of contents are all plainly coming. A host matches with a fallback
/// arm, which is what [`Action::from_name`] returning `Option` already
/// asks of the string path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Action {
    /// Forward one page in reading order.
    NextPage,
    /// Back one page in reading order.
    PrevPage,
    /// Forward one spine unit (chapter), landing on its first page.
    NextUnit,
    /// Back one spine unit.
    PrevUnit,
    /// Return to where a followed link was taken from.
    Back,
    /// Raise the base font size one step.
    FontUp,
    /// Lower the base font size one step.
    FontDown,
    /// Advance the color theme one place in its cycle.
    CycleTheme,
    /// Show or hide the shell's own chrome. The engine has no menu; this
    /// is handed back to the caller.
    ToggleMenu,
}

impl Action {
    /// Stable lowercase name, for logs and for a persisted key map.
    pub fn name(self) -> &'static str {
        match self {
            Action::NextPage => "next-page",
            Action::PrevPage => "prev-page",
            Action::NextUnit => "next-unit",
            Action::PrevUnit => "prev-unit",
            Action::Back => "back",
            Action::FontUp => "font-up",
            Action::FontDown => "font-down",
            Action::CycleTheme => "cycle-theme",
            Action::ToggleMenu => "toggle-menu",
        }
    }

    /// Inverse of [`Action::name`].
    pub fn from_name(name: &str) -> Option<Action> {
        match name {
            "next-page" => Some(Action::NextPage),
            "prev-page" => Some(Action::PrevPage),
            "next-unit" => Some(Action::NextUnit),
            "prev-unit" => Some(Action::PrevUnit),
            "back" => Some(Action::Back),
            "font-up" => Some(Action::FontUp),
            "font-down" => Some(Action::FontDown),
            "cycle-theme" => Some(Action::CycleTheme),
            "toggle-menu" => Some(Action::ToggleMenu),
            _ => None,
        }
    }
}

/// What the engine did with an [`Action`], and what the shell owes the
/// platform in return.
///
/// Two questions, not one. "Should I repaint" and "did I consume this
/// event" are different, and a shell that has only the first gets the
/// second wrong in a way that is hard to attribute later: Android's
/// `onKeyDown` must return `true` to keep an event, and [`KeyMap`] binds
/// the volume keys by default, so a reader that answers "nothing
/// changed" on the last page hands the keypress back and the system
/// draws its volume slider over the book. iOS's responder chain has the
/// same shape with quieter symptoms.
///
/// The distinction is the engine's to make because the engine is what
/// knows. Whether there is anywhere to go [`Back`](Action::Back) to is a
/// fact about the back stack, and the answer at the bottom of it —
/// *let the platform's own Back have this* — is the convention on both
/// mobile targets.
///
/// Unlike [`Action`] and [`Key`] this is a closed set: consumed and
/// repainting are two bits, and the fourth corner (declining an event
/// but demanding a repaint) describes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionOutcome {
    /// Applied, and the position, size or theme moved. Consume the event
    /// and repaint.
    Changed,
    /// Applied, and nothing moved — the last page, or the font already at
    /// its stop. Consume the event anyway: the reader *does* take this
    /// key, it just had nothing to do this time.
    Unchanged,
    /// Not the engine's. [`Action::ToggleMenu`] always, because a
    /// reader's chrome belongs to the shell, and [`Action::Back`] with an
    /// empty history. Let the event through to the platform.
    Unhandled,
}

impl ActionOutcome {
    /// Whether the shell should repaint. Only [`Changed`](Self::Changed).
    pub fn needs_redraw(self) -> bool {
        self == ActionOutcome::Changed
    }

    /// Whether the shell should tell the platform it took the event —
    /// `true` for anything the engine applied, moved or not.
    pub fn consumed(self) -> bool {
        self != ActionOutcome::Unhandled
    }
}
/// Which physical edge reading starts from.
///
/// Tap zones are the only thing here that needs it: "the left third goes
/// back" is true of an English book and backwards for an Arabic or Hebrew
/// one, and getting it wrong makes a book unreadable rather than merely
/// unfamiliar.
///
/// The book owns this, not the shell. `Publication::reading_direction`
/// produces it — from EPUB's `page-progression-direction`, where a
/// package declares one — and `Session::reading_direction` hands it to
/// whoever is building [`TapZones`]. A shell that decides for itself
/// decides `Ltr` on every platform it ships to, because that is the
/// default and nothing else would tell it otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReadingDirection {
    /// Left to right: the previous page is off the left edge.
    #[default]
    Ltr,
    /// Right to left: the previous page is off the right edge.
    Rtl,
}

/// Where on the page a tap means what.
///
/// Three vertical bands: one at the edge a page turn comes *from*, one at
/// the edge it goes *to*, and whatever is left in the middle. That is the
/// policy nearly every ereader implements and each one gets slightly
/// differently, which is why the fractions are fields rather than
/// constants.
///
/// Bands are fractions of the full page width, margins included — a tap in
/// the margin is still a tap on that side of the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TapZones {
    /// Width of the previous-page band, as a fraction of page width.
    /// Zero disables it and gives the space to the middle.
    pub prev_fraction: f32,
    /// Width of the next-page band, as a fraction of page width.
    pub next_fraction: f32,
    /// What a tap between the two bands does. `None` ignores it, which is
    /// what a shell running its own menu gesture wants.
    pub middle: Option<Action>,
    /// Which edge `prev_fraction` is measured from.
    pub direction: ReadingDirection,
}

impl Default for TapZones {
    /// Thirds, with the middle opening the menu, reading left to right.
    ///
    /// The direction is the one field this cannot get right on its own.
    /// Prefer [`TapZones::new`] with `Session::reading_direction`, and
    /// reach for `default` only where there is no book open to ask.
    fn default() -> Self {
        TapZones {
            prev_fraction: 1.0 / 3.0,
            next_fraction: 1.0 / 3.0,
            middle: Some(Action::ToggleMenu),
            direction: ReadingDirection::Ltr,
        }
    }
}

impl TapZones {
    /// The default policy in a book's reading direction.
    ///
    /// The direction to pass is `Session::reading_direction`; see
    /// [`ReadingDirection`] for why it is not the shell's to choose.
    pub fn new(direction: ReadingDirection) -> Self {
        TapZones {
            direction,
            ..TapZones::default()
        }
    }

    /// Turn a tap into an action, or `None` if it falls outside the page
    /// or in a middle band with nothing bound to it.
    ///
    /// `x` and `y` are in *panel* coordinates — the numbers a touch event
    /// carries — and are mapped back through the rotation, so a shell
    /// drawing to a turned panel does not have to think about it.
    ///
    /// Bands are resolved previous-first, so overlapping fractions
    /// resolve to the previous-page side rather than to nothing.
    pub fn action_at(&self, x: f32, y: f32, metrics: &PageMetrics) -> Option<Action> {
        let (px, py) = metrics.panel_to_page(x, y);
        let (w, h) = (metrics.size.w, metrics.size.h);
        if w <= 0.0 || h <= 0.0 || px < 0.0 || py < 0.0 || px > w || py > h {
            return None;
        }
        // Measured from the reading-start edge, so the arithmetic below is
        // written once and RTL is a mirror of the coordinate rather than a
        // second set of comparisons.
        let from_start = match self.direction {
            ReadingDirection::Ltr => px / w,
            ReadingDirection::Rtl => 1.0 - px / w,
        };
        let (prev, next) = (self.prev_fraction.max(0.0), self.next_fraction.max(0.0));
        // The near edge of a band is exclusive and the far edge inclusive,
        // so the page is covered exactly once. That is also why the next
        // band is guarded on being non-empty: `>= 1.0 - 0.0` is true at
        // x == w, so a disabled band would still take the last column.
        if from_start < prev {
            Some(Action::PrevPage)
        } else if next > 0.0 && from_start >= 1.0 - next {
            Some(Action::NextPage)
        } else {
            self.middle
        }
    }
}

/// A key, in the vocabulary the engine binds — not a platform keycode.
///
/// A shell translates its own events into this and asks the [`KeyMap`]
/// what they mean. The list is short on purpose: every variant is bound by
/// [`KeyMap::default`], because a name here that means nothing anywhere is
/// surface at the Contract tier bought for nothing.
///
/// Modifiers are absent, also on purpose. Chorded shortcuts belong to the
/// shell — they are where an app's own commands live, and an engine that
/// claimed `Ctrl` would collide with every one of them. Consult the map
/// for unmodified presses only.
///
/// `#[non_exhaustive]`: a device with a button nobody here has held is
/// always possible, so a host matches with a fallback arm and ignores
/// what it does not recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    PageUp,
    PageDown,
    Space,
    Backspace,
    /// A dedicated previous-page button. Kobo and PocketBook put two on
    /// the bezel; they arrive as `KEY_PAGEUP`, `KEY_PREV` or a function
    /// key depending on the model and the kernel, which is exactly the
    /// reverse-engineering a shell should not have to repeat.
    TurnPrev,
    /// A dedicated next-page button.
    TurnNext,
    /// Volume up, which Android readers conventionally borrow for page
    /// turns. Only bound because a shell that forwards it here has
    /// already decided to take it from the system.
    VolumeUp,
    /// Volume down.
    VolumeDown,
    /// A printable character, already lowercased by the shell.
    Char(char),
}

/// The default binding table, plus whatever a shell changes about it.
///
/// Arrow keys are bound *logically* — `ArrowRight` is the next page in
/// both reading directions — rather than physically. A reader that
/// mirrored them in RTL would be pressing a key that points away from the
/// page it is going to, and the two conventions cannot both be the
/// default; this one at least matches what the tap zones do to the
/// underlying action.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyMap {
    bindings: Vec<(Key, Action)>,
}

impl Default for KeyMap {
    fn default() -> Self {
        use Action::*;
        use Key::*;
        KeyMap {
            bindings: vec![
                (ArrowRight, NextPage),
                (ArrowDown, NextPage),
                (PageDown, NextPage),
                (Space, NextPage),
                (TurnNext, NextPage),
                (VolumeDown, NextPage),
                (ArrowLeft, PrevPage),
                (ArrowUp, PrevPage),
                (PageUp, PrevPage),
                (TurnPrev, PrevPage),
                (VolumeUp, PrevPage),
                (Char('n'), NextUnit),
                (Char('p'), PrevUnit),
                (Char('b'), Back),
                (Backspace, Back),
                (Char('+'), FontUp),
                (Char('='), FontUp),
                (Char('-'), FontDown),
                (Char('t'), CycleTheme),
                (Char('m'), ToggleMenu),
            ],
        }
    }
}

impl KeyMap {
    /// A map with nothing bound, for a shell building its own table.
    pub fn empty() -> Self {
        KeyMap {
            bindings: Vec::new(),
        }
    }

    /// What this key does, or `None` if nothing — in which case the shell
    /// should handle the press itself rather than swallow it.
    pub fn action(&self, key: Key) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, a)| *a)
    }

    /// Bind a key, replacing any existing binding for it. One action may
    /// have several keys; one key has at most one action.
    pub fn bind(&mut self, key: Key, action: Action) {
        match self.bindings.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = action,
            None => self.bindings.push((key, action)),
        }
    }

    /// Remove a binding. Silent if there wasn't one.
    pub fn unbind(&mut self, key: Key) {
        self.bindings.retain(|(k, _)| *k != key);
    }

    /// Every binding, in table order — what a keyboard-shortcuts screen
    /// wants, and what makes the defaults auditable from outside.
    pub fn bindings(&self) -> &[(Key, Action)] {
        &self.bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EdgeSizes, Size};
    use crate::page::Rotation;

    const ALL_ACTIONS: [Action; 9] = [
        Action::NextPage,
        Action::PrevPage,
        Action::NextUnit,
        Action::PrevUnit,
        Action::Back,
        Action::FontUp,
        Action::FontDown,
        Action::CycleTheme,
        Action::ToggleMenu,
    ];

    fn page() -> PageMetrics {
        PageMetrics::new(Size::new(600.0, 800.0), EdgeSizes::uniform(40.0), 1.0)
    }

    #[test]
    fn action_names_round_trip() {
        for a in ALL_ACTIONS {
            assert_eq!(Action::from_name(a.name()), Some(a), "{}", a.name());
        }
        assert_eq!(Action::from_name("next page"), None);
        assert_eq!(Action::from_name(""), None);
    }

    #[test]
    fn thirds_turn_pages_and_the_middle_opens_the_menu() {
        let z = TapZones::default();
        let m = page();
        assert_eq!(z.action_at(10.0, 400.0, &m), Some(Action::PrevPage));
        assert_eq!(z.action_at(300.0, 400.0, &m), Some(Action::ToggleMenu));
        assert_eq!(z.action_at(590.0, 400.0, &m), Some(Action::NextPage));
        // A tap in the top or bottom margin is still a tap on that side.
        assert_eq!(z.action_at(10.0, 2.0, &m), Some(Action::PrevPage));
        assert_eq!(z.action_at(590.0, 798.0, &m), Some(Action::NextPage));
    }

    #[test]
    fn rtl_mirrors_the_bands_and_nothing_else() {
        let ltr = TapZones::default();
        let rtl = TapZones::new(ReadingDirection::Rtl);
        let m = page();
        for x in [10.0f32, 300.0, 590.0] {
            let mirrored = rtl.action_at(m.size.w - x, 400.0, &m);
            assert_eq!(ltr.action_at(x, 400.0, &m), mirrored, "x = {x}");
        }
        // Concretely: in an RTL book the previous page is off the right.
        assert_eq!(rtl.action_at(590.0, 400.0, &m), Some(Action::PrevPage));
        assert_eq!(rtl.action_at(10.0, 400.0, &m), Some(Action::NextPage));
    }

    #[test]
    fn a_rotated_panel_is_mapped_back_before_the_bands_are_read() {
        let m = page().with_rotation(Rotation::Quarter);
        let z = TapZones::default();
        // Panel is 800x600 now. `panel_to_page` sends (x, y) to (y, h - x),
        // so a tap near the panel's far edge lands in the page's first
        // third and has to read as a page back.
        assert_eq!(m.panel_size(), Size::new(800.0, 600.0));
        assert_eq!(z.action_at(700.0, 100.0, &m), Some(Action::PrevPage));
        assert_eq!(z.action_at(700.0, 500.0, &m), Some(Action::NextPage));
        // Unrotated, that same tap is the other answer entirely, which is
        // the bug a shell would otherwise ship.
        assert_eq!(
            z.action_at(700.0, 100.0, &page()),
            None,
            "700 is off a 600-wide page"
        );
    }

    #[test]
    fn taps_outside_the_page_are_nobodys() {
        let z = TapZones::default();
        let m = page();
        assert_eq!(z.action_at(-1.0, 400.0, &m), None);
        assert_eq!(z.action_at(601.0, 400.0, &m), None);
        assert_eq!(z.action_at(300.0, -1.0, &m), None);
        assert_eq!(z.action_at(300.0, 801.0, &m), None);
        // And a page with no area cannot be divided into thirds.
        let empty = PageMetrics::new(Size::ZERO, EdgeSizes::default(), 1.0);
        assert_eq!(z.action_at(0.0, 0.0, &empty), None);
    }

    #[test]
    fn the_middle_can_be_the_whole_page_or_nothing_at_all() {
        let m = page();
        let all_middle = TapZones {
            prev_fraction: 0.0,
            next_fraction: 0.0,
            ..TapZones::default()
        };
        assert_eq!(
            all_middle.action_at(0.0, 400.0, &m),
            Some(Action::ToggleMenu)
        );
        assert_eq!(
            all_middle.action_at(600.0, 400.0, &m),
            Some(Action::ToggleMenu)
        );

        // A shell running its own menu gesture wants the middle inert.
        let quiet = TapZones {
            middle: None,
            ..TapZones::default()
        };
        assert_eq!(quiet.action_at(300.0, 400.0, &m), None);
        assert_eq!(quiet.action_at(10.0, 400.0, &m), Some(Action::PrevPage));
    }

    #[test]
    fn overlapping_bands_resolve_to_prev_rather_than_to_nothing() {
        let m = page();
        let greedy = TapZones {
            prev_fraction: 0.8,
            next_fraction: 0.8,
            ..TapZones::default()
        };
        assert_eq!(greedy.action_at(300.0, 400.0, &m), Some(Action::PrevPage));
        assert_eq!(greedy.action_at(590.0, 400.0, &m), Some(Action::NextPage));

        // A negative fraction is a caller's arithmetic, not a band that
        // eats the other side.
        let negative = TapZones {
            prev_fraction: -0.5,
            ..TapZones::default()
        };
        assert_eq!(
            negative.action_at(10.0, 400.0, &m),
            Some(Action::ToggleMenu)
        );
    }

    #[test]
    fn the_default_map_already_knows_the_hardware() {
        let k = KeyMap::default();
        // The forcing case: bezel buttons nobody's desktop shell has.
        assert_eq!(k.action(Key::TurnNext), Some(Action::NextPage));
        assert_eq!(k.action(Key::TurnPrev), Some(Action::PrevPage));
        // Android's borrowed volume keys: down goes down the book.
        assert_eq!(k.action(Key::VolumeDown), Some(Action::NextPage));
        assert_eq!(k.action(Key::VolumeUp), Some(Action::PrevPage));
        // And what the desktop shells already do by hand.
        assert_eq!(k.action(Key::Space), Some(Action::NextPage));
        assert_eq!(k.action(Key::Char('n')), Some(Action::NextUnit));
        assert_eq!(k.action(Key::Char('t')), Some(Action::CycleTheme));
        assert_eq!(k.action(Key::Backspace), Some(Action::Back));
        // Unbound keys come back so the shell can have them.
        assert_eq!(k.action(Key::Char('q')), None);
        assert_eq!(k.action(Key::Char('N')), None, "shells lowercase first");
    }

    #[test]
    fn every_named_key_is_bound_by_default() {
        let bound: Vec<Key> = KeyMap::default()
            .bindings()
            .iter()
            .map(|(k, _)| *k)
            .collect();
        for key in [
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::PageUp,
            Key::PageDown,
            Key::Space,
            Key::Backspace,
            Key::TurnPrev,
            Key::TurnNext,
            Key::VolumeUp,
            Key::VolumeDown,
        ] {
            assert!(
                bound.contains(&key),
                "{key:?} is surface bought for nothing"
            );
        }
    }

    #[test]
    fn binding_replaces_and_unbinding_removes() {
        let mut k = KeyMap::default();
        let before = k.bindings().len();
        k.bind(Key::Space, Action::ToggleMenu);
        assert_eq!(k.action(Key::Space), Some(Action::ToggleMenu));
        assert_eq!(k.bindings().len(), before, "a rebind is not a new row");

        k.bind(Key::Char('g'), Action::Back);
        assert_eq!(k.bindings().len(), before + 1);

        k.unbind(Key::Char('g'));
        assert_eq!(k.action(Key::Char('g')), None);
        k.unbind(Key::Char('g'));
        assert_eq!(
            k.bindings().len(),
            before,
            "unbinding twice is not an error"
        );

        assert!(KeyMap::empty().bindings().is_empty());
    }
}
