// Rung 2 of the iOS ladder: it binds.
//
// Every call below crosses the C ABI the way a Swift host would drive it —
// through the checked-in header, with no generated binding and no Rust in
// sight. The output is findings for docs/FFI.md, not code to keep.
//
// Arguments: <book.epub> <fonts-dir> <library-dir>

import CChapbook
import Foundation

// Finding, not styling: the cpp_compat header's C branch declares every
// enum twice — a plain `enum` for the constants and a fixed-width typedef
// for the signatures — and Swift imports those as two *different* types
// with the same name. The constants reach signature-land through
// `.rawValue`, converted once, here.
let OK = Int32(CB_OK.rawValue)
let ERR_BUFFER_TOO_SMALL = Int32(CB_ERR_BUFFER_TOO_SMALL.rawValue)
let LOG_INFO = Int32(CB_LOG_INFO.rawValue)
let ROTATION_NONE = UInt32(CB_ROTATION_NONE.rawValue)
let DIRECTION_LTR = UInt32(CB_DIRECTION_LTR.rawValue)
let DIRECTION_RTL = UInt32(CB_DIRECTION_RTL.rawValue)
let ACTION_NONE = UInt32(CB_ACTION_NONE.rawValue)
let ACTION_PREV_PAGE = UInt32(CB_ACTION_PREV_PAGE.rawValue)
let ACTION_NEXT_PAGE = UInt32(CB_ACTION_NEXT_PAGE.rawValue)
let ACTION_NEXT_UNIT = UInt32(CB_ACTION_NEXT_UNIT.rawValue)
let ACTION_TOGGLE_MENU = UInt32(CB_ACTION_TOGGLE_MENU.rawValue)
let OUTCOME_CHANGED = UInt32(CB_OUTCOME_CHANGED.rawValue)
let OUTCOME_UNCHANGED = UInt32(CB_OUTCOME_UNCHANGED.rawValue)
let OUTCOME_UNHANDLED = UInt32(CB_OUTCOME_UNHANDLED.rawValue)
let KEY_PAGE_DOWN = UInt32(CB_KEY_PAGE_DOWN.rawValue)

let args = CommandLine.arguments
guard args.count == 4 else {
    print("usage: spike <book.epub> <fonts-dir> <library-dir>")
    exit(64)
}
let bookPath = args[1], fontsDir = args[2], libraryDir = args[3]

var failures = 0
@MainActor func expect(_ ok: Bool, _ what: String) {
    print("\(ok ? "ok " : "FAIL") \(what)")
    if !ok { failures += 1 }
}

// The two-call string idiom, as a host has to write it.
func lastError() -> String {
    var needed = 0
    let probe = cb_last_error_message(nil, 0, &needed)
    guard probe == ERR_BUFFER_TOO_SMALL, needed > 0 else { return "" }
    var buf = [CChar](repeating: 0, count: needed)
    guard cb_last_error_message(&buf, buf.count, &needed) == OK else {
        return "<unreadable>"
    }
    return String(decoding: buf[..<(needed - 1)].map { UInt8(bitPattern: $0) }, as: UTF8.self)
}

func readString(_ call: (UnsafeMutablePointer<CChar>?, Int, UnsafeMutablePointer<Int>?) -> Int32) -> String? {
    var needed = 0
    guard call(nil, 0, &needed) == ERR_BUFFER_TOO_SMALL, needed > 0 else { return nil }
    var buf = [CChar](repeating: 0, count: needed)
    guard call(&buf, buf.count, &needed) == OK else { return nil }
    return String(decoding: buf[..<(needed - 1)].map { UInt8(bitPattern: $0) }, as: UTF8.self)
}

// ---- The ABI has a voice before it has a session ----

// A Swift closure with no captures is a C function pointer; one *with*
// captures is not, which is why `user` exists — unused here, load-bearing
// for any real shell (Unmanaged.fromOpaque, then hop to the main actor).
_ = cb_set_log_callback({ _, target, message, _ in
    let t = target.map { String(cString: $0) } ?? "?"
    let m = message.map { String(cString: $0) } ?? "?"
    print("   [engine] \(t): \(m)")
}, nil, LOG_INFO)

print("abi version \(cb_abi_version()), capabilities 0x\(String(cb_capabilities(), radix: 16))")
expect(cb_abi_version() > 0, "cb_abi_version answers")

// ---- Failure first: codes are the contract, and Swift can read both halves ----

let missing = cb_session_open_path("/nonexistent/book.epub", cb_config_new(cb_font_source_host()))
expect(missing == nil, "a bad path refuses to open")
let message = lastError()
expect(!message.isEmpty, "cb_last_error_message explains: \"\(message)\"")

// ---- Send-not-Sync, spelled in Swift ----

/// Deliberately **not** `Sendable`: strict concurrency will not let two
/// isolation domains share one of these, but region isolation can still
/// *transfer* it — which is exactly the handle's contract, movable and
/// not shareable. Adding `@unchecked Sendable` here would be claiming
/// `Sync`, which the Rust side does not have.
final class Session {
    let raw: OpaquePointer

    init?(path: String, fontsDir: String, libraryDir: String) {
        guard let fonts = cb_font_source_embedded(fontsDir, "Crimson Text"),
              let config = cb_config_new(fonts)
        else { return nil }
        guard cb_config_set_library_dir(config, libraryDir) == OK,
              let session = cb_session_open_path(path, config)
        else { return nil } // config is consumed either way; nothing to free
        raw = session
    }

    deinit { cb_session_close(raw) }

    func title() -> String? {
        readString { cb_session_title(raw, $0, $1, $2) }
    }
}

guard let session = Session(path: bookPath, fontsDir: fontsDir, libraryDir: libraryDir) else {
    print("FAIL open \(bookPath): \(lastError())")
    exit(1)
}

// ---- Metrics, metadata ----

let metrics = cb_metrics(
    width: 600, height: 800,
    margin_top: 40, margin_right: 40, margin_bottom: 40, margin_left: 40,
    dpi_scale: 2, rotation: ROTATION_NONE
)
expect(cb_session_set_metrics(session.raw, metrics) == OK, "metrics cross by value")

let title = session.title()
expect(title != nil && !title!.isEmpty, "title crosses out: \"\(title ?? "")\"")

var count = 0
expect(cb_session_page_count(session.raw, &count) == OK && count > 0, "\(count) pages at 600x800@2")

// ---- Input: the model Android proved, driven from Swift ----

var direction = DIRECTION_RTL
expect(cb_session_reading_direction(session.raw, &direction) == OK && direction == DIRECTION_LTR,
       "the book says which edge it reads from")

var action = ACTION_NONE
expect(cb_session_tap_action(session.raw, 100, 400, &action) == OK && action == ACTION_PREV_PAGE,
       "a tap in the left third means previous")
expect(cb_session_tap_action(session.raw, 500, 400, &action) == OK && action == ACTION_NEXT_PAGE,
       "a tap in the right third means next")

// The volume-slider case from the Android device run, replayed: at page
// zero prev does not move, and the event is still the reader's.
var outcome = OUTCOME_CHANGED
expect(cb_session_apply(session.raw, ACTION_PREV_PAGE, &outcome) == OK
       && outcome == OUTCOME_UNCHANGED, "prev at page 0 is consumed, no repaint")
expect(cb_session_apply(session.raw, ACTION_NEXT_PAGE, &outcome) == OK
       && outcome == OUTCOME_CHANGED, "next turns a page")
expect(cb_session_apply(session.raw, ACTION_TOGGLE_MENU, &outcome) == OK
       && outcome == OUTCOME_UNHANDLED, "the menu is the shell's")
expect(cb_key_default_action(KEY_PAGE_DOWN) == ACTION_NEXT_PAGE,
       "the default key table answers with no session")
expect(cb_char_default_action(UInt32(UnicodeScalar("n").value)) == ACTION_NEXT_UNIT,
       "and so does the char half")

// ---- Pixels cross; what they look like is rung 3's question ----

var w: UInt32 = 0, h: UInt32 = 0
expect(cb_session_render_size(session.raw, &w, &h) == OK && w == 1200 && h == 1600,
       "render size is device pixels: \(w)x\(h)")
var pixels = [UInt8](repeating: 0, count: Int(w) * Int(h) * 4)
let rc = pixels.withUnsafeMutableBufferPointer {
    cb_session_render_into(session.raw, $0.baseAddress, $0.count, w, h, Int(w) * 4)
}
expect(rc == OK && pixels.contains { $0 != 0 }, "render_into fills the caller's buffer")

// ---- Lifecycle: suspend releases, and the session stays usable ----

expect(cb_session_suspend(session.raw) == OK, "suspend")
expect(cb_session_render_size(session.raw, &w, &h) == OK, "and the session survives it")

// ---- The move. `roaming` is local so region isolation can see its
// whole life: it is created here, sent into the detached task, and never
// touched again from this side. That compiling under -swift-version 6 is
// Send-not-Sync proven by the compiler; adding a use of `roaming` after
// the Task is the data race Swift now refuses to build. ----

func titleFromAnotherThread(path: String, fontsDir: String, libraryDir: String) async -> String? {
    let dir = libraryDir + "/roam"
    try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    guard let roaming = Session(path: path, fontsDir: fontsDir, libraryDir: dir) else { return nil }
    return await Task.detached { roaming.title() }.value
}
let elsewhere = await titleFromAnotherThread(path: bookPath, fontsDir: fontsDir, libraryDir: libraryDir)
expect(elsewhere == title, "the handle moved to another thread and answered there")

print(failures == 0 ? "rung 2: it binds" : "rung 2: \(failures) failure(s)")
exit(failures == 0 ? 0 : 1)
