import CChapbook
import Foundation

/// A reader intent. Produced by a tap-zone or key lookup — or the app's
/// own UI — and handed to [`Session.apply(_:)`]; an app never needs to
/// interpret one.
///
/// An open set: the header appends values and never renumbers, so raw
/// values are safe to persist and unknown ones safe to pass through.
public struct Action: RawRepresentable, Hashable, Sendable {
    public let rawValue: UInt32
    public init(rawValue: UInt32) { self.rawValue = rawValue }

    public static let nextPage = Action(rawValue: UInt32(CB_ACTION_NEXT_PAGE.rawValue))
    public static let prevPage = Action(rawValue: UInt32(CB_ACTION_PREV_PAGE.rawValue))
    public static let nextUnit = Action(rawValue: UInt32(CB_ACTION_NEXT_UNIT.rawValue))
    public static let prevUnit = Action(rawValue: UInt32(CB_ACTION_PREV_UNIT.rawValue))
    public static let back = Action(rawValue: UInt32(CB_ACTION_BACK.rawValue))
    public static let fontUp = Action(rawValue: UInt32(CB_ACTION_FONT_UP.rawValue))
    public static let fontDown = Action(rawValue: UInt32(CB_ACTION_FONT_DOWN.rawValue))
    public static let cycleTheme = Action(rawValue: UInt32(CB_ACTION_CYCLE_THEME.rawValue))
    public static let toggleMenu = Action(rawValue: UInt32(CB_ACTION_TOGGLE_MENU.rawValue))

    /// What `key` does in the engine's default table — which already
    /// knows a Kobo's bezel buttons — or `nil` for a key this reader
    /// does not bind, in which case handle the press yourself rather
    /// than swallowing it. Unmodified presses only: chords are the
    /// app's own command space.
    public static func `default`(for key: Key) -> Action? {
        let raw = cb_key_default_action(key.rawValue)
        return raw == C.actionNone ? nil : Action(rawValue: raw)
    }

    /// The printable-character half of the default table.
    public static func `default`(for scalar: Unicode.Scalar) -> Action? {
        let raw = cb_char_default_action(scalar.value)
        return raw == C.actionNone ? nil : Action(rawValue: raw)
    }
}

/// A key in the vocabulary the engine binds — not a platform keycode.
/// Translate the HID usages a hardware keyboard reports into these; the
/// translation has no opinions in it, the opinions are in the table
/// behind [`Action.default(for:)-swift.type.method`].
public struct Key: RawRepresentable, Hashable, Sendable {
    public let rawValue: UInt32
    public init(rawValue: UInt32) { self.rawValue = rawValue }

    public static let arrowLeft = Key(rawValue: UInt32(CB_KEY_ARROW_LEFT.rawValue))
    public static let arrowRight = Key(rawValue: UInt32(CB_KEY_ARROW_RIGHT.rawValue))
    public static let arrowUp = Key(rawValue: UInt32(CB_KEY_ARROW_UP.rawValue))
    public static let arrowDown = Key(rawValue: UInt32(CB_KEY_ARROW_DOWN.rawValue))
    public static let pageUp = Key(rawValue: UInt32(CB_KEY_PAGE_UP.rawValue))
    public static let pageDown = Key(rawValue: UInt32(CB_KEY_PAGE_DOWN.rawValue))
    public static let space = Key(rawValue: UInt32(CB_KEY_SPACE.rawValue))
    public static let backspace = Key(rawValue: UInt32(CB_KEY_BACKSPACE.rawValue))
    public static let turnPrev = Key(rawValue: UInt32(CB_KEY_TURN_PREV.rawValue))
    public static let turnNext = Key(rawValue: UInt32(CB_KEY_TURN_NEXT.rawValue))
}

/// What the engine did with an action — both of the answers an app
/// needs, because neither derives from the other: *should I repaint*,
/// and *did the reader consume this event*.
///
/// The second is what the responder chain is owed: call `super` (or
/// otherwise let the event through) only on `.unhandled`. That is also
/// what `.back` answers at the bottom of the trail, deliberately —
/// exactly where the platform's own Back should take over.
public enum ActionOutcome: UInt32, Sendable {
    /// Applied and something moved. Consume the event and repaint.
    case changed = 0
    /// Applied and nothing moved — the last page, the font at its stop.
    /// Consume the event anyway; do not repaint.
    case unchanged = 1
    /// Not the engine's. Let the event through to the platform.
    case unhandled = 2
}

/// Which physical edge reading starts from. The book declares it; it is
/// exposed so an app can *show* it — the only way a reader can tell a
/// correctly-flipped RTL book from a bug.
public enum ReadingDirection: UInt32, Sendable {
    case leftToRight = 0
    case rightToLeft = 1
}
