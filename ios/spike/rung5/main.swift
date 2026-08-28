// Rung 5 of the iOS ladder: it takes a security-scoped bookmark.
//
// The iOS twin of Android's content URI, and the other half of the
// argument for typed sources. The flow under test is the one that fails
// in real apps: not the picker's happy path, but the *cold* one — a
// bookmark persisted on a previous launch, resolved before any session
// exists, access started, and only then a file descriptor handed to
// `cb_session_open_fd`, which takes ownership.
//
// First launch: pick the book in the picker; the app stores the bookmark
// and opens warm. Every later launch: no picker, resolve from cold.

import CChapbook
import Darwin
import UIKit
import UniformTypeIdentifiers

let OK = Int32(CB_OK.rawValue)
let FORMAT_GUESS = UInt32(CB_FORMAT_GUESS.rawValue)

final class Flow: UIViewController, UIDocumentPickerDelegate {
    let label = UILabel()
    var report: [String] = []

    func say(_ line: String) {
        print("RUNG5 \(line)")
        report.append(line)
        label.text = report.joined(separator: "\n")
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        label.numberOfLines = 0
        label.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        label.frame = view.bounds.insetBy(dx: 16, dy: 60)
        label.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(label)
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if let bookmark = UserDefaults.standard.data(forKey: "book") {
            say("cold: bookmark found (\(bookmark.count) bytes), no picker")
            resolveAndOpen(bookmark, from: "cold")
        } else {
            say("first launch: presenting the picker")
            let epub = UTType(filenameExtension: "epub") ?? .data
            let picker = UIDocumentPickerViewController(forOpeningContentTypes: [epub], asCopy: false)
            picker.delegate = self
            present(picker, animated: false)
        }
    }

    func documentPicker(_ picker: UIDocumentPickerViewController,
                        didPickDocumentsAt urls: [URL]) {
        guard let url = urls.first else { return }
        say("picked: \(url.path)")
        // Access must be live while the bookmark is *made*, and on iOS the
        // bookmark options are empty — .withSecurityScope is macOS.
        let scoped = url.startAccessingSecurityScopedResource()
        say("picker URL needed scope: \(scoped)")
        do {
            let bookmark = try url.bookmarkData()
            UserDefaults.standard.set(bookmark, forKey: "book")
            say("bookmark stored (\(bookmark.count) bytes)")
            resolveAndOpen(bookmark, from: "warm")
        } catch {
            say("FAIL bookmarkData: \(error)")
        }
        if scoped { url.stopAccessingSecurityScopedResource() }
    }

    func resolveAndOpen(_ bookmark: Data, from phase: String) {
        var stale = false
        let url: URL
        do {
            url = try URL(resolvingBookmarkData: bookmark, options: [],
                          relativeTo: nil, bookmarkDataIsStale: &stale)
        } catch {
            say("FAIL resolve (\(phase)): \(error)")
            return
        }
        say("\(phase): resolved to \(url.lastPathComponent), stale: \(stale)")

        // Revocable, and does not survive relaunch by itself — this call
        // is the whole reason the bookmark exists.
        let scoped = url.startAccessingSecurityScopedResource()
        say("\(phase): access granted, scope needed: \(scoped)")
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }

        // The descriptor door: what a bookmark ultimately becomes. The
        // session takes ownership, so no close on this side.
        let fd = open(url.path, O_RDONLY)
        guard fd >= 0 else {
            say("FAIL open(2): errno \(errno)")
            return
        }

        let fontsDir = Bundle.main.resourcePath! + "/fonts"
        let library = NSSearchPathForDirectoriesInDomains(
            .applicationSupportDirectory, .userDomainMask, true)[0] + "/chapbook"
        try? FileManager.default.createDirectory(
            atPath: library, withIntermediateDirectories: true)

        guard let fonts = cb_font_source_embedded(fontsDir, "Crimson Text"),
              let config = cb_config_new(fonts),
              cb_config_set_library_dir(config, library) == OK,
              let session = cb_session_open_fd(fd, FORMAT_GUESS, config)
        else {
            var needed = 0
            var buf = [CChar](repeating: 0, count: 512)
            _ = cb_last_error_message(&buf, buf.count, &needed)
            let msg = String(decoding: buf[..<max(needed - 1, 0)].map { UInt8(bitPattern: $0) }, as: UTF8.self)
            say("FAIL open_fd: \(msg)")
            return
        }

        let metrics = cb_metrics(
            width: 390, height: 844,
            margin_top: 60, margin_right: 24, margin_bottom: 40, margin_left: 24,
            dpi_scale: 3, rotation: UInt32(CB_ROTATION_NONE.rawValue))
        _ = cb_session_set_metrics(session, metrics)

        var needed = 0
        var buf = [CChar](repeating: 0, count: 512)
        var title = "?"
        if cb_session_title(session, &buf, buf.count, &needed) == OK {
            title = String(decoding: buf[..<(needed - 1)].map { UInt8(bitPattern: $0) }, as: UTF8.self)
        }
        var pages = 0
        _ = cb_session_page_count(session, &pages)
        say("\(phase): open through the fd — \"\(title)\", \(pages) pages in unit 0")
        cb_session_close(session)
        say(phase == "cold" ? "rung 5: it takes a bookmark" : "now terminate and relaunch cold")
    }

    func documentPickerWasCancelled(_ picker: UIDocumentPickerViewController) {
        say("picker cancelled — nothing to test without a book")
    }
}

@main
final class App: UIResponder, UIApplicationDelegate {
    var window: UIWindow?
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = Flow()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}
