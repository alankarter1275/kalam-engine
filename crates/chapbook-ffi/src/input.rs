//! The input model: what a host's native events *mean* to a reader.
//!
//! Deliberately not a gesture recognizer — the platform's own is better
//! and is what an app is judged on. What crosses here is the part that is
//! genuinely engine knowledge: an action vocabulary, a tap-zone policy
//! that knows which edge a page turn comes from, and the default key
//! table. The model lives in `chapbook_core::input`; this file is its C
//! shape, held back until a touchscreen had actually exercised it — see
//! `docs/FFI.md`, *What the touchscreen settled*.

use chapbook_reader::chapbook_core::{Action, ActionOutcome, Key, KeyMap, ReadingDirection};

use crate::error::{cb_status, fail, guard};
use crate::session::cb_session;

/// A reader intent. Hosts produce these — from a tap zone, a key lookup,
/// or their own UI — and hand them to [`cb_session_apply`]; a host never
/// needs to interpret one.
///
/// `CB_ACTION_NONE` is not an action: it is the "nothing" value
/// [`cb_session_tap_action`] and the key lookups answer with, and
/// [`cb_session_set_tap_zones`] accepts for a band that does nothing.
/// Applying it is an error, not a no-op.
///
/// The set is open — bookmarks, search, a jump to the table of contents
/// are plainly coming — and new values are only ever appended. Values in
/// this header are permanent, so a host may persist them.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_action {
    CB_ACTION_NONE = 0,
    /// Forward one page in reading order.
    CB_ACTION_NEXT_PAGE = 1,
    /// Back one page in reading order.
    CB_ACTION_PREV_PAGE = 2,
    /// Forward one spine unit (chapter), landing on its first page.
    CB_ACTION_NEXT_UNIT = 3,
    /// Back one spine unit.
    CB_ACTION_PREV_UNIT = 4,
    /// Return to where a followed link was taken from.
    CB_ACTION_BACK = 5,
    /// Raise the base font size one step. The step is the engine's, so a
    /// reader who changes device finds the same ladder.
    CB_ACTION_FONT_UP = 6,
    /// Lower the base font size one step.
    CB_ACTION_FONT_DOWN = 7,
    /// Advance the color theme one place in its cycle.
    CB_ACTION_CYCLE_THEME = 8,
    /// Show or hide the host's own chrome. The engine has no menu, so
    /// applying this always comes back `CB_OUTCOME_UNHANDLED`: it exists
    /// because *the middle of the page opens the menu* is the same policy
    /// decision as *the outer thirds turn pages*.
    CB_ACTION_TOGGLE_MENU = 9,
}

/// What the engine did with an action — two answers, because a host needs
/// both and can derive neither from the other: *should I repaint*, and
/// *did the reader consume this event*.
///
/// The second answer is what a host owes its platform. Android's
/// `onKeyDown` must return `true` on `CHANGED` **and** `UNCHANGED`, or the
/// last page of every book gets the system volume slider drawn over it;
/// iOS's responder chain is the same shape — call `super` only on
/// `UNHANDLED`. `UNHANDLED` is also what `CB_ACTION_BACK` returns at the
/// bottom of the back trail, deliberately: that is exactly where the
/// platform's own Back should take over, so a host forwards the gesture
/// unconditionally instead of shadowing the history to know when to stop.
///
/// Unlike [`cb_action`] this set is closed: consumed and repainting are
/// two bits, and the fourth corner — declining an event while demanding a
/// repaint — describes nothing.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_action_outcome {
    /// Applied, and the position, size or theme moved. Consume the event
    /// and repaint.
    CB_OUTCOME_CHANGED = 0,
    /// Applied, and nothing moved — the last page, or the font at its
    /// stop. Consume the event anyway; do not repaint.
    CB_OUTCOME_UNCHANGED = 1,
    /// Not the engine's. Let the event through to the platform.
    CB_OUTCOME_UNHANDLED = 2,
}

/// Which physical edge reading starts from. The book declares it — EPUB's
/// `page-progression-direction` — and the engine consults it; it crosses
/// the boundary so a host can *show* it, which is the only way a reader
/// can tell a correctly-flipped RTL book from a bug, and so a host's own
/// gestures (a page-turn swipe, say) can agree with the tap zones.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_reading_direction {
    /// Left to right: the previous page is off the left edge.
    CB_DIRECTION_LTR = 0,
    /// Right to left: the previous page is off the right edge.
    CB_DIRECTION_RTL = 1,
}

/// A key, in the vocabulary the engine binds — not a platform keycode.
///
/// A host translates its own stable codes into this: Android's
/// `KEYCODE_*`, the HID usages iOS and macOS report on hardware
/// keyboards, GDK keyvals. The translation has no opinions in it; the
/// opinions are in the table behind [`cb_key_default_action`]. Printable
/// characters are not here — they go through [`cb_char_default_action`].
///
/// Modifiers are deliberately absent: chorded shortcuts are where a
/// host's own commands live, and an engine that claimed `Ctrl` would
/// collide with every one of them. Consult the table for unmodified
/// presses only.
///
/// Zero is deliberately unassigned, so a zeroed variable never names a
/// key. The set is open, like [`cb_action`], and grows the same way.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_key {
    CB_KEY_ARROW_LEFT = 1,
    CB_KEY_ARROW_RIGHT = 2,
    CB_KEY_ARROW_UP = 3,
    CB_KEY_ARROW_DOWN = 4,
    CB_KEY_PAGE_UP = 5,
    CB_KEY_PAGE_DOWN = 6,
    CB_KEY_SPACE = 7,
    CB_KEY_BACKSPACE = 8,
    /// A dedicated previous-page button. Kobo and PocketBook put two on
    /// the bezel; they arrive as `KEY_PAGEUP`, `KEY_PREV` or a function
    /// key depending on the model and the kernel, which is exactly the
    /// reverse-engineering a host should not have to repeat.
    CB_KEY_TURN_PREV = 9,
    /// A dedicated next-page button.
    CB_KEY_TURN_NEXT = 10,
    /// Volume up, which Android readers conventionally borrow for page
    /// turns. Only forward it here if the host has already decided to
    /// take it from the system — and note that iOS offers no way to.
    CB_KEY_VOLUME_UP = 11,
    /// Volume down.
    CB_KEY_VOLUME_DOWN = 12,
}

/// [`cb_action`] from the engine's `Action`, for answers crossing out.
///
/// `Action` is `#[non_exhaustive]`: an intent this header has no number
/// for yet answers `CB_ACTION_NONE` rather than leaking a value the host
/// cannot have been compiled to expect.
fn action_out(action: Option<Action>) -> cb_action {
    match action {
        Some(Action::NextPage) => cb_action::CB_ACTION_NEXT_PAGE,
        Some(Action::PrevPage) => cb_action::CB_ACTION_PREV_PAGE,
        Some(Action::NextUnit) => cb_action::CB_ACTION_NEXT_UNIT,
        Some(Action::PrevUnit) => cb_action::CB_ACTION_PREV_UNIT,
        Some(Action::Back) => cb_action::CB_ACTION_BACK,
        Some(Action::FontUp) => cb_action::CB_ACTION_FONT_UP,
        Some(Action::FontDown) => cb_action::CB_ACTION_FONT_DOWN,
        Some(Action::CycleTheme) => cb_action::CB_ACTION_CYCLE_THEME,
        Some(Action::ToggleMenu) => cb_action::CB_ACTION_TOGGLE_MENU,
        Some(_) | None => cb_action::CB_ACTION_NONE,
    }
}

/// The engine's `Action` from a [`cb_action`], for values crossing in.
fn action_in(action: cb_action) -> Option<Action> {
    match action {
        cb_action::CB_ACTION_NONE => None,
        cb_action::CB_ACTION_NEXT_PAGE => Some(Action::NextPage),
        cb_action::CB_ACTION_PREV_PAGE => Some(Action::PrevPage),
        cb_action::CB_ACTION_NEXT_UNIT => Some(Action::NextUnit),
        cb_action::CB_ACTION_PREV_UNIT => Some(Action::PrevUnit),
        cb_action::CB_ACTION_BACK => Some(Action::Back),
        cb_action::CB_ACTION_FONT_UP => Some(Action::FontUp),
        cb_action::CB_ACTION_FONT_DOWN => Some(Action::FontDown),
        cb_action::CB_ACTION_CYCLE_THEME => Some(Action::CycleTheme),
        cb_action::CB_ACTION_TOGGLE_MENU => Some(Action::ToggleMenu),
    }
}

/// A fraction of the page width a caller may legally give us: finite and
/// within the page. Spelled out for the same reason as the metrics check —
/// NaN arrives from C as easily as any other bit pattern.
fn band(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

/// Which edge the open book reads from.
#[no_mangle]
pub unsafe extern "C" fn cb_session_reading_direction(
    session: *const cb_session,
    direction: *mut cb_reading_direction,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        // SAFETY: a handle from an open call, not yet closed.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
        };
        if direction.is_null() {
            return fail(
                cb_status::CB_ERR_NULL_ARGUMENT,
                "direction out-pointer is null",
            );
        }
        let value = match session.inner.reading_direction() {
            ReadingDirection::Ltr => cb_reading_direction::CB_DIRECTION_LTR,
            ReadingDirection::Rtl => cb_reading_direction::CB_DIRECTION_RTL,
        };
        // SAFETY: checked non-null just above.
        unsafe { *direction = value };
        cb_status::CB_OK
    })
}

/// Reconfigure the tap bands: how wide the previous- and next-page bands
/// are, as fractions of the page width, and what a tap between them does —
/// `CB_ACTION_NONE` for a middle band that does nothing, which is what a
/// host running its own menu gesture wants. Until this is called, a
/// session has the default policy: thirds, with the middle
/// `CB_ACTION_TOGGLE_MENU`.
///
/// The reading direction is deliberately not a parameter. It is the
/// book's, and is re-read here, so a host cannot flip a book by
/// configuring it. Bands overlapping resolve to the previous-page side;
/// a zero-width band is disabled, and its space goes to the middle.
#[no_mangle]
pub unsafe extern "C" fn cb_session_set_tap_zones(
    session: *mut cb_session,
    prev_fraction: f32,
    next_fraction: f32,
    middle: cb_action,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        use chapbook_reader::chapbook_core::TapZones;
        // SAFETY: a handle from an open call, not yet closed.
        let Some(session) = (unsafe { session.as_mut() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
        };
        if !(band(prev_fraction) && band(next_fraction)) {
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                "prev_fraction and next_fraction must be finite fractions in 0..=1",
            );
        }
        session.zones = TapZones {
            prev_fraction,
            next_fraction,
            middle: action_in(middle),
            direction: session.inner.reading_direction(),
        };
        cb_status::CB_OK
    })
}

/// What a tap at a point means, or `CB_ACTION_NONE` for nothing.
///
/// `x` and `y` are **logical units** in **panel** coordinates — the
/// numbers a touch event actually carries, in the same space
/// `cb_session_set_metrics` was given. On iOS a `UITouch` location is
/// already logical: pass it as is. On Android a `MotionEvent` is in view
/// pixels: divide by density first, or every tap on a dense screen lands
/// in the last band and nothing errors. A rotated panel is undone on this
/// side, so a host never applies the inverse itself.
///
/// The bands were resolved against the book's reading direction when they
/// were configured, which is why the answer is the session's to give:
/// the same tap on the same box means previous-page in an English book
/// and next-page in a Hebrew one.
///
/// `CB_ERR_UNAVAILABLE` until metrics are set — a session with no page
/// box cannot say where its thirds are.
#[no_mangle]
pub unsafe extern "C" fn cb_session_tap_action(
    session: *const cb_session,
    x: f32,
    y: f32,
    action: *mut cb_action,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        // SAFETY: a handle from an open call, not yet closed.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
        };
        if action.is_null() {
            return fail(
                cb_status::CB_ERR_NULL_ARGUMENT,
                "action out-pointer is null",
            );
        }
        if !(x.is_finite() && y.is_finite()) {
            return fail(cb_status::CB_ERR_INVALID_ARGUMENT, "x and y must be finite");
        }
        let Some(metrics) = session.inner.metrics() else {
            return fail(
                cb_status::CB_ERR_UNAVAILABLE,
                "no page box yet: set metrics first",
            );
        };
        // SAFETY: checked non-null just above.
        unsafe { *action = action_out(session.zones.action_at(x, y, &metrics)) };
        cb_status::CB_OK
    })
}

/// What a key does in the default table, or `CB_ACTION_NONE` for one this
/// reader does not bind — in which case the host should handle the press
/// itself rather than swallow it.
///
/// Free of any session, because the value is the default table itself: it
/// already knows the bezel buttons on a Kobo and the volume keys an
/// Android reader borrows, which is the difference between porting a
/// shell and reverse-engineering one. A host that lets readers rebind
/// keys keeps its overrides on its own side and falls back to this.
///
/// Arrow keys are bound *logically* — `CB_KEY_ARROW_RIGHT` is the next
/// page in both reading directions — which at least agrees with what the
/// tap zones resolve to.
#[no_mangle]
pub extern "C" fn cb_key_default_action(key: cb_key) -> cb_action {
    guard(cb_action::CB_ACTION_NONE, || {
        let key = match key {
            cb_key::CB_KEY_ARROW_LEFT => Key::ArrowLeft,
            cb_key::CB_KEY_ARROW_RIGHT => Key::ArrowRight,
            cb_key::CB_KEY_ARROW_UP => Key::ArrowUp,
            cb_key::CB_KEY_ARROW_DOWN => Key::ArrowDown,
            cb_key::CB_KEY_PAGE_UP => Key::PageUp,
            cb_key::CB_KEY_PAGE_DOWN => Key::PageDown,
            cb_key::CB_KEY_SPACE => Key::Space,
            cb_key::CB_KEY_BACKSPACE => Key::Backspace,
            cb_key::CB_KEY_TURN_PREV => Key::TurnPrev,
            cb_key::CB_KEY_TURN_NEXT => Key::TurnNext,
            cb_key::CB_KEY_VOLUME_UP => Key::VolumeUp,
            cb_key::CB_KEY_VOLUME_DOWN => Key::VolumeDown,
        };
        action_out(KeyMap::default().action(key))
    })
}

/// What a printable character does in the default table, or
/// `CB_ACTION_NONE`. The other half of [`cb_key_default_action`], split
/// out because a character is a Unicode scalar and not a member of a
/// closed set.
///
/// `codepoint` is a Unicode scalar value. The table binds lowercase
/// letters; ASCII uppercase is folded here so a host need not care, and
/// anything that is not a scalar value answers `CB_ACTION_NONE`.
/// Unmodified presses only, as above.
#[no_mangle]
pub extern "C" fn cb_char_default_action(codepoint: u32) -> cb_action {
    guard(cb_action::CB_ACTION_NONE, || {
        let Some(ch) = char::from_u32(codepoint) else {
            return cb_action::CB_ACTION_NONE;
        };
        action_out(KeyMap::default().action(Key::Char(ch.to_ascii_lowercase())))
    })
}

/// Apply a reader intent, and learn both of the things a host needs to
/// know about what happened — see [`cb_action_outcome`].
///
/// `CB_ACTION_NONE` is refused as `CB_ERR_INVALID_ARGUMENT` rather than
/// treated as a quiet no-op: a host holding `NONE` has a tap or a key
/// that meant nothing, and applying it anyway is a bug worth hearing
/// about on the spot.
#[no_mangle]
pub unsafe extern "C" fn cb_session_apply(
    session: *mut cb_session,
    action: cb_action,
    outcome: *mut cb_action_outcome,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        // SAFETY: a handle from an open call, not yet closed.
        let Some(session) = (unsafe { session.as_mut() }) else {
            return fail(cb_status::CB_ERR_NULL_ARGUMENT, "session is null");
        };
        if outcome.is_null() {
            return fail(
                cb_status::CB_ERR_NULL_ARGUMENT,
                "outcome out-pointer is null",
            );
        }
        let Some(action) = action_in(action) else {
            return fail(
                cb_status::CB_ERR_INVALID_ARGUMENT,
                "CB_ACTION_NONE is not an action",
            );
        };
        let value = match session.inner.apply(action) {
            ActionOutcome::Changed => cb_action_outcome::CB_OUTCOME_CHANGED,
            ActionOutcome::Unchanged => cb_action_outcome::CB_OUTCOME_UNCHANGED,
            ActionOutcome::Unhandled => cb_action_outcome::CB_OUTCOME_UNHANDLED,
        };
        // SAFETY: checked non-null just above.
        unsafe { *outcome = value };
        cb_status::CB_OK
    })
}
