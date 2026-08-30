import CChapbook
import Foundation

/// An open book.
///
/// Deliberately **not** `Sendable`. The handle underneath is `Send` and
/// not `Sync` — movable between threads, never shareable across them —
/// and that is exactly what a non-`Sendable` class expresses under
/// strict concurrency: region isolation can *transfer* a session into a
/// task for good, and using it from two isolation domains at once is the
/// data race Swift refuses to build. Marking this `@unchecked Sendable`
/// would be claiming the `Sync` the engine does not have; don't.
///
/// One consequence worth designing around rather than against: a
/// top-level `let` is a global in Swift's eyes and a global can never be
/// proven sent, so scope the session to a view controller or an actor,
/// not a singleton.
public final class Session {
    let raw: OpaquePointer
    private var waker: Unmanaged<WakerBox>?

    /// Open a book. The configuration is spent either way; on failure the
    /// error's `message` says why — install [`EngineLog`] first and the
    /// engine narrates its own failures too.
    public init(source: BookSource, configuration: SessionConfiguration) throws {
        let config = try configuration.makeRaw()
        // Every open consumes `config`, success or not.
        let session: OpaquePointer? =
            switch source {
            case .path(let url):
                cb_session_open_path(url.path, config)
            case .bytes(let data, let format):
                data.withUnsafeBytes { buffer in
                    cb_session_open_bytes(
                        buffer.bindMemory(to: UInt8.self).baseAddress,
                        buffer.count, format.rawValue, config)
                }
            case .fileDescriptor(let fd, let format):
                cb_session_open_fd(fd, format.rawValue, config)
            case .catalog(let url):
                cb_session_open_url(url.absoluteString, config)
            }
        guard let session else { throw ChapbookError.openFailure() }
        raw = session
    }

    deinit {
        cb_session_close(raw)
        waker?.release()
    }

    // MARK: Shape

    /// Reconfigure the page box. Call before the first render and on any
    /// size, scale or rotation change; the reader's place survives it.
    public func setMetrics(_ metrics: PageMetrics) throws {
        try check(cb_session_set_metrics(raw, metrics.raw))
    }

    public func title() -> String? {
        readString { cb_session_title(self.raw, $0, $1, $2) }
    }

    /// Pages in the current unit, at the current metrics.
    public func pageCount() throws -> Int {
        var count = 0
        try check(cb_session_page_count(raw, &count))
        return count
    }

    /// Units (chapters) in the book.
    public func spineLength() throws -> Int {
        var length = 0
        try check(cb_session_spine_len(raw, &length))
        return length
    }

    /// Where the reader is. Always the pair — a page number alone is
    /// meaningless across a unit boundary.
    public struct Position: Hashable, Sendable {
        public let spine: Int
        public let page: Int
    }

    public func position() throws -> Position {
        var raw = cb_position(spine: 0, page: 0)
        try check(cb_session_position(self.raw, &raw))
        return Position(spine: Int(raw.spine), page: Int(raw.page))
    }

    public func readingDirection() throws -> ReadingDirection {
        var raw: UInt32 = 0
        try check(cb_session_reading_direction(self.raw, &raw))
        return ReadingDirection(rawValue: raw) ?? .leftToRight
    }

    // MARK: Navigation

    /// Returns whether the position moved — use it, never a position
    /// comparison, to decide on a repaint.
    @discardableResult
    public func nextPage() throws -> Bool {
        var moved = false
        try check(cb_session_next_page(raw, &moved))
        return moved
    }

    @discardableResult
    public func prevPage() throws -> Bool {
        var moved = false
        try check(cb_session_prev_page(raw, &moved))
        return moved
    }

    // MARK: Input

    /// What a tap means, or `nil` for nothing. `point` is in the same
    /// logical panel coordinates the metrics were given — a `UITouch`
    /// location is already right, pass it as is. A rotated panel is
    /// undone on the engine's side.
    public func tapAction(at point: CGPoint) throws -> Action? {
        var raw: UInt32 = 0
        try check(cb_session_tap_action(self.raw, Float(point.x), Float(point.y), &raw))
        return raw == C.actionNone ? nil : Action(rawValue: raw)
    }

    /// Reconfigure the tap bands, as fractions of the page width, with
    /// `middle: nil` for a middle band that does nothing. The reading
    /// direction is never a parameter: it is the book's, so a tap cannot
    /// be resolved against the wrong edge. The default is thirds, middle
    /// bound to [`Action.toggleMenu`].
    public func setTapZones(
        prevFraction: CGFloat, nextFraction: CGFloat, middle: Action?
    ) throws {
        try check(
            cb_session_set_tap_zones(
                raw, Float(prevFraction), Float(nextFraction),
                middle?.rawValue ?? C.actionNone))
    }

    /// Apply a reader intent and learn both what happened and who owns
    /// the event — see [`ActionOutcome`].
    public func apply(_ action: Action) throws -> ActionOutcome {
        var raw: UInt32 = 0
        try check(cb_session_apply(self.raw, action.rawValue, &raw))
        return ActionOutcome(rawValue: raw) ?? .unhandled
    }

    // MARK: Settings

    public func settings() throws -> ReadingSettings {
        var raw = cb_settings(
            base_font_px: 0, line_height: 0, justify: false,
            publisher_styles: false, theme: 0)
        try check(cb_session_settings(self.raw, &raw))
        return ReadingSettings(raw: raw)
    }

    /// Apply settings, keeping the reader's place across the reflow.
    public func setSettings(_ settings: ReadingSettings, scope: SettingsScope) throws {
        try check(cb_session_set_settings(raw, settings.raw, scope.rawValue))
    }

    // MARK: Lifecycle and memory

    /// Save the position and let go of everything reconstructible,
    /// including the database and its POSIX locks — call it from the last
    /// callback the platform guarantees (`willResignActive` /
    /// `didEnterBackground`). The session stays usable afterwards.
    public func suspend() throws {
        try check(cb_session_suspend(raw))
    }

    /// Drop cached layouts and decoded pages, for a memory warning. The
    /// current page rebuilds on the next render.
    public func releaseCaches() throws {
        try check(cb_session_release_caches(raw))
    }

    public func cacheBytes() throws -> Int {
        var bytes = 0
        try check(cb_session_cache_bytes(raw, &bytes))
        return bytes
    }

    // MARK: Fonts, diagnosed

    /// How many faces the font source produced — worth logging once at
    /// startup, since font failures are the quiet kind.
    public func fontFaceCount() throws -> Int {
        var count = 0
        try check(cb_session_font_face_count(raw, &count))
        return count
    }

    /// Generic families that resolved to a name no loaded face carries,
    /// one `generic=family` per line; `nil` when everything resolved.
    public func unresolvedFontGenerics() -> String? {
        let report = readString { cb_session_font_unresolved(self.raw, $0, $1, $2) }
        return report.flatMap { $0.isEmpty ? nil : $0 }
    }

    // MARK: Background loads

    /// Comic and PDF pages decode on the engine's loader thread. The
    /// handler fires **on that thread** when something new is ready — do
    /// nothing there but hop to the main actor and call
    /// [`pollLoaded()`], repainting if it answers `true`.
    public func onWake(_ handler: @escaping @Sendable () -> Void) throws {
        let box = Unmanaged.passRetained(WakerBox(handler))
        // A Swift closure with captures is not a C function pointer,
        // which is exactly what `user` exists for.
        try check(
            cb_session_set_waker(
                raw,
                { user in
                    guard let user else { return }
                    Unmanaged<WakerBox>.fromOpaque(user).takeUnretainedValue().handler()
                },
                box.toOpaque()))
        waker?.release()
        waker = box
    }

    /// Take delivery of anything the loader finished; `true` means the
    /// visible page changed and a repaint is worth doing.
    public func pollLoaded() throws -> Bool {
        var changed = false
        try check(cb_session_poll_loaded(raw, &changed))
        return changed
    }

    public func hasPendingLoads() throws -> Bool {
        var pending = false
        try check(cb_session_has_pending_loads(raw, &pending))
        return pending
    }
}

final class WakerBox: @unchecked Sendable {
    let handler: @Sendable () -> Void
    init(_ handler: @escaping @Sendable () -> Void) { self.handler = handler }
}
