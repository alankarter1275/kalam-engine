import CChapbook
import Foundation

/// Where a session's fonts come from. Required, not defaulted, because
/// "no fonts" fails silently: a fontless session lays out, renders and
/// conforms — it just paginates every book to one blank page. On iOS
/// there is no host-font branch upstream, so the ordinary choice is
/// [`embedded`](FontSource.embedded(directory:family:)) over faces the
/// app bundles.
public struct FontSource: Sendable {
    enum Kind: Sendable {
        case host
        case embedded(directory: String, family: String)
    }

    let kind: Kind

    /// The platform's own font collection. Correct on macOS; on iOS it
    /// resolves to an empty database today (fontdb has no iOS branch) and
    /// the open fails with a message saying exactly that.
    public static let host = FontSource(kind: .host)

    /// A directory of faces the caller owns — bundled with the app is the
    /// expected shape — with `family` naming the family all five CSS
    /// generics resolve to.
    public static func embedded(directory: URL, family: String) -> FontSource {
        FontSource(kind: .embedded(directory: directory.path, family: family))
    }

    /// Build the C-side source. The caller owns the result until it is
    /// consumed by `cb_config_new`.
    func makeRaw() throws -> OpaquePointer {
        let raw: OpaquePointer? =
            switch kind {
            case .host:
                cb_font_source_host()
            case .embedded(let directory, let family):
                cb_font_source_embedded(directory, family)
            }
        guard let raw else { throw ChapbookError.openFailure() }
        return raw
    }
}

/// Everything a session needs from its host, named explicitly — the
/// engine reaches for nothing behind the caller's back, because every
/// implicit reach (installed fonts, `$HOME`, environment variables) is a
/// desktop assumption.
public struct SessionConfiguration: Sendable {
    public var fonts: FontSource

    /// Where positions, annotations and settings persist. On iOS pass a
    /// directory under `Library/Application Support` — there is no
    /// platform default on purpose, and leaving it unset opens the book
    /// with no library: it reads fine and remembers nothing.
    ///
    /// If the app ever gains a share extension or widget, the directory
    /// moves into the app-group container — and must, because suspend
    /// releases the database's POSIX locks precisely so the watchdog
    /// (`0xdead10cc`) has nothing to kill over.
    public var libraryDirectory: URL?

    /// Cap on the decoded-page and layout caches, in bytes. The engine's
    /// default is 192 MiB; a memory warning can lower it live through
    /// [`Session.releaseCaches()`].
    public var cacheBudgetBytes: Int?

    /// How catalogs and streamed pages are fetched. The default is
    /// per-platform — `URLSession.shared` on iOS, the engine's bundled
    /// transport on macOS; see [`HTTPTransport`].
    public var transport: HTTPTransport

    public init(
        fonts: FontSource,
        libraryDirectory: URL? = nil,
        cacheBudgetBytes: Int? = nil,
        transport: HTTPTransport = .platformDefault
    ) {
        self.fonts = fonts
        self.libraryDirectory = libraryDirectory
        self.cacheBudgetBytes = cacheBudgetBytes
        self.transport = transport
    }

    /// Build the C-side config, consuming a fresh font source. The result
    /// is owned by the caller until a `cb_session_open_*` consumes it.
    func makeRaw() throws -> OpaquePointer {
        let fonts = try fonts.makeRaw()
        // `cb_config_new` consumes `fonts` whether it succeeds or not.
        guard let config = cb_config_new(fonts) else {
            throw ChapbookError.openFailure()
        }
        do {
            if let dir = libraryDirectory {
                try check(cb_config_set_library_dir(config, dir.path))
            }
            if let budget = cacheBudgetBytes {
                try check(cb_config_set_cache_budget(config, budget))
            }
            try transport.install(into: config)
        } catch {
            cb_config_free(config)
            throw error
        }
        return config
    }
}
