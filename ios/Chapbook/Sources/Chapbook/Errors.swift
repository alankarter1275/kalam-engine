import CChapbook
import Foundation

/// A failure reported across the C ABI.
///
/// `status` is the contract — the codes in `chapbook.h` are permanent and
/// safe to match on. `message` is the human-readable half, fetched from
/// the boundary's thread-local at the moment of failure, and explicitly
/// not stable: show it, log it, never parse it.
public struct ChapbookError: Error, CustomStringConvertible, Sendable {
    /// `nil` for the constructors, which report failure as a null handle
    /// plus a message rather than a code.
    public let status: Int32?
    public let message: String

    public var description: String {
        status.map { "chapbook error \($0): \(message)" } ?? "chapbook error: \(message)"
    }

    /// The failure a status-returning call just reported.
    static func last(_ status: Int32) -> ChapbookError {
        ChapbookError(status: status, message: lastErrorMessage())
    }

    /// The failure behind a constructor's null return.
    static func openFailure() -> ChapbookError {
        ChapbookError(status: nil, message: lastErrorMessage())
    }
}

// The cpp_compat header declares every enum twice for C: a plain `enum`
// for the constants and a fixed-width typedef for the signatures, and
// Swift imports those as two different types with the same name. Every
// constant reaches signature-land through `.rawValue`, converted once,
// here — the "constants shim" the spike said the real package would want.
enum C {
    static let ok = Int32(CB_OK.rawValue)
    static let bufferTooSmall = Int32(CB_ERR_BUFFER_TOO_SMALL.rawValue)

    static let actionNone = UInt32(CB_ACTION_NONE.rawValue)
    static let formatGuess = UInt32(CB_FORMAT_GUESS.rawValue)
}

/// Throw unless the boundary said OK.
@inline(__always)
func check(_ status: Int32) throws {
    guard status == C.ok else { throw ChapbookError.last(status) }
}

/// The two-call string idiom, as every string-returning entry point wants
/// it driven: probe with no buffer, learn the size, call again. `nil` when
/// the call failed for a reason other than the expected probe result.
func readString(
    _ call: (UnsafeMutablePointer<CChar>?, Int, UnsafeMutablePointer<Int>?) -> Int32
) -> String? {
    var needed = 0
    guard call(nil, 0, &needed) == C.bufferTooSmall, needed > 0 else { return nil }
    var buf = [CChar](repeating: 0, count: needed)
    guard call(&buf, buf.count, &needed) == C.ok, needed >= 1 else { return nil }
    return String(
        decoding: buf[..<(needed - 1)].map { UInt8(bitPattern: $0) },
        as: UTF8.self
    )
}

func lastErrorMessage() -> String {
    readString { cb_last_error_message($0, $1, $2) } ?? ""
}
