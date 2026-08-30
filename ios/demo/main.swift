// The demo reader: one screen, one book, the whole custody loop.
//
// First launch presents the document picker; the app stores a
// security-scoped bookmark and opens warm. Every later launch resolves
// the bookmark cold — no picker — and the book comes back on the page
// the reader left, because the engine adopts it by its bytes'
// fingerprint. Taps resolve through the engine's zones: outer thirds
// turn pages, the middle toggles a status overlay standing in for a
// menu.

import Chapbook
import UIKit
import UniformTypeIdentifiers

final class ReaderViewController: UIViewController, UIDocumentPickerDelegate {
    let pageView = UIImageView()
    let status = UILabel()
    var session: Session?
    // The page's text runs as VoiceOver's element list; render() owes it
    // one pageChanged() per draw, and it no-ops unless the text moved.
    var a11y: PageAccessibility?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        pageView.frame = view.bounds
        pageView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        pageView.contentMode = .scaleAspectFit
        view.addSubview(pageView)

        status.numberOfLines = 0
        status.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        status.frame = view.bounds.insetBy(dx: 16, dy: 60)
        status.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        status.isHidden = true
        view.addSubview(status)

        view.addGestureRecognizer(
            UITapGestureRecognizer(target: self, action: #selector(tapped)))

        // Suspend from the last callback the platform guarantees:
        // position saved, database closed, caches released.
        NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { try? self?.session?.suspend() }
        }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard session == nil else { return }
        if let bookmark = UserDefaults.standard.data(forKey: "book") {
            open(bookmark: bookmark, phase: "cold")
        } else {
            let epub = UTType(filenameExtension: "epub") ?? .data
            let picker = UIDocumentPickerViewController(
                forOpeningContentTypes: [epub, .pdf, .archive], asCopy: false)
            picker.delegate = self
            present(picker, animated: false)
        }
    }

    func documentPicker(
        _ picker: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]
    ) {
        guard let url = urls.first else { return }
        do {
            let bookmark = try SecurityScopedBook.bookmark(for: url)
            UserDefaults.standard.set(bookmark, forKey: "book")
            open(bookmark: bookmark, phase: "warm")
        } catch {
            show("bookmark failed: \(error)")
        }
    }

    func open(bookmark: Data, phase: String) {
        do {
            let resolved = try SecurityScopedBook.open(bookmark)
            let library = URL(
                fileURLWithPath: NSSearchPathForDirectoriesInDomains(
                    .applicationSupportDirectory, .userDomainMask, true)[0]
            ).appendingPathComponent("chapbook")
            try? FileManager.default.createDirectory(
                at: library, withIntermediateDirectories: true)

            let session = try Session(
                source: .fileDescriptor(resolved.fileDescriptor),
                configuration: SessionConfiguration(
                    fonts: .embedded(
                        directory: URL(
                            fileURLWithPath: Bundle.main.resourcePath! + "/fonts"),
                        family: "Crimson Text"),
                    libraryDirectory: library))

            let bounds = view.bounds
            try session.setMetrics(
                PageMetrics(
                    width: bounds.width, height: bounds.height,
                    marginTop: 60, marginRight: 24, marginBottom: 40, marginLeft: 24,
                    dpiScale: view.window?.screen.scale ?? UIScreen.main.scale))

            // Comic and PDF pages decode in the background; wake, poll,
            // repaint — from the main actor, never the loader thread.
            try session.onWake { [weak self] in
                Task { @MainActor in
                    guard let self, let session = self.session else { return }
                    if (try? session.pollLoaded()) == true { self.render() }
                }
            }

            self.session = session
            self.a11y = PageAccessibility(host: pageView, session: session)
            print("DEMO \(phase): \"\(session.title() ?? "?")\" — \(resolved.url.lastPathComponent)")
            render()
        } catch {
            show("\(phase) open failed: \(error)")
        }
    }

    func render() {
        guard let session else { return }
        do {
            let image = try session.renderImage()
            pageView.image = UIImage(
                cgImage: image,
                scale: view.window?.screen.scale ?? UIScreen.main.scale,
                orientation: .up)
            a11y?.pageChanged()
        } catch {
            show("render failed: \(error)")
        }
    }

    @objc func tapped(_ gesture: UITapGestureRecognizer) {
        guard let session else { return }
        let point = gesture.location(in: view)
        guard let action = (try? session.tapAction(at: point)) ?? nil else { return }
        switch try? session.apply(action) {
        case .changed:
            render()
        case .unhandled where action == .toggleMenu:
            toggleStatus()
        default:
            break
        }
    }

    func toggleStatus() {
        guard let session else { return }
        if status.isHidden {
            let position = (try? session.position()).map { "unit \($0.spine + 1), page \($0.page + 1)" }
            status.text = """
                \(session.title() ?? "?")
                \(position ?? "?")
                \((try? session.fontFaceCount()).map { "\($0) faces" } ?? "")
                capabilities: \(Capabilities.current().rawValue)
                """
            status.backgroundColor = .systemBackground.withAlphaComponent(0.85)
        }
        status.isHidden.toggle()
    }

    func show(_ line: String) {
        print("DEMO \(line)")
        status.text = line
        status.isHidden = false
    }

    func documentPickerWasCancelled(_ picker: UIDocumentPickerViewController) {
        show("picker cancelled — nothing to read without a book")
    }
}

@main
final class App: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        EngineLog.install { level, target, message in
            print("DEMO [engine] \(target): \(message)")
        }
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = ReaderViewController()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}
