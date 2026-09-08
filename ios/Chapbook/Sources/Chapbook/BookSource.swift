import CChapbook
import Darwin
import Foundation

/// Which reader opens the bytes. `.guess` decides from the bytes
/// themselves, and is the right answer even when a name is available —
/// the EPUB `mimetype` entry and the `%PDF` header do not lie, and a
/// filename does.
public struct BookFormat: RawRepresentable, Hashable, Sendable {
    public let rawValue: UInt32
    public init(rawValue: UInt32) { self.rawValue = rawValue }

    public static let guess = BookFormat(rawValue: UInt32(CB_FORMAT_GUESS.rawValue))
    public static let epub = BookFormat(rawValue: UInt32(CB_FORMAT_EPUB.rawValue))
    public static let cbz = BookFormat(rawValue: UInt32(CB_FORMAT_CBZ.rawValue))
    public static let pdf = BookFormat(rawValue: UInt32(CB_FORMAT_PDF.rawValue))
}

/// Where a book's bytes come from.
///
/// Every case reaches the library (when the configuration names one):
/// identity is a fingerprint of the bytes, so a book opened from a
/// descriptor keeps its position, annotations and per-book settings just
/// as a path-opened one does. What the engine cannot do is reopen the
/// *file* — holding the security-scoped bookmark and re-resolving it on
/// the next launch is the app's half of custody.
public enum BookSource {
    /// A file the app can reach by path — its own container, mostly.
    case path(URL)

    /// Bytes already in memory. They are copied across the boundary.
    case bytes(Data, format: BookFormat = .guess)

    /// An open file descriptor — what a security-scoped bookmark
    /// ultimately becomes. **The session takes ownership** and closes it
    /// with the session, so hand over a descriptor nothing else will use.
    case fileDescriptor(Int32, format: BookFormat = .guess)

    /// An OPDS catalog URL, resolved to a page stream: every page is
    /// fetched on demand through the configuration's [`HTTPTransport`]
    /// and cached under its library directory — which the configuration
    /// **must** name, or the open fails saying so. Opening one is
    /// synchronous network, so construct catalog sessions off the main
    /// actor.
    case catalog(URL)
}

/// The custody flow around the document picker, as free functions the
/// demo and any real app share. The rules encoded here were all learned
/// the hard way:
///
/// - Access must be live while the bookmark is *made*, and on iOS the
///   bookmark options are empty — `.withSecurityScope` is macOS.
/// - The scope dance is unconditional. A picker URL inside the app's own
///   container still answers `true` to `startAccessing`, so code that
///   special-cases "foreign" files works until the day the URL is
///   somebody else's.
/// - Scoped access does not survive relaunch. Resolve the stored
///   bookmark and start access *before* constructing the session, every
///   cold launch.
public enum SecurityScopedBook {
    /// A bookmark for a URL the picker just vended. Store the data;
    /// UserDefaults is fine, it is not a secret.
    public static func bookmark(for url: URL) throws -> Data {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        #if os(macOS)
            return try url.bookmarkData(options: [.withSecurityScope])
        #else
            return try url.bookmarkData()
        #endif
    }

    public struct Resolved {
        /// Ready for [`BookSource.fileDescriptor(_:format:)`], which takes
        /// ownership.
        public let fileDescriptor: Int32
        public let url: URL
        /// The platform suggests re-making the bookmark; the descriptor
        /// is still good.
        public let isStale: Bool
    }

    /// Resolve a stored bookmark to an open descriptor: resolve, start
    /// scoped access, `open(2)`, stop access. The descriptor stays
    /// readable after the scope closes — POSIX checks permissions at
    /// open — which is what lets the session own it for as long as the
    /// book is open.
    public static func open(_ bookmark: Data) throws -> Resolved {
        var stale = false
        #if os(macOS)
            let options: URL.BookmarkResolutionOptions = [.withSecurityScope]
        #else
            let options: URL.BookmarkResolutionOptions = []
        #endif
        let url = try URL(
            resolvingBookmarkData: bookmark, options: options,
            relativeTo: nil, bookmarkDataIsStale: &stale)

        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }

        let fd = Darwin.open(url.path, O_RDONLY)
        guard fd >= 0 else {
            throw ChapbookError(
                status: nil,
                message: "open(2) failed for \(url.lastPathComponent): errno \(errno)")
        }
        return Resolved(fileDescriptor: fd, url: url, isStale: stale)
    }
}
