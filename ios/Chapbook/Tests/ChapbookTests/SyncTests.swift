import Foundation
import Testing

@testable import Chapbook

// The sync reach, driven the way an app drives it: record where a book
// syncs, open a worker over an injected URLSession, ask, and drain what
// happened. A URLProtocol stub is the service, so nothing here touches a
// network — what is being proven is the Swift half of the crossing: the
// worker's PUT goes out through the app's own session, and the reports
// come back typed. The reconcile logic itself is chapbook-sync's suite;
// the C marshaling is chapbook-ffi's tests/abi.rs.

private let fixtures = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()  // ChapbookTests
    .deletingLastPathComponent()  // Tests
    .deletingLastPathComponent()  // Chapbook
    .deletingLastPathComponent()  // ios
    .deletingLastPathComponent()  // repo root
    .appendingPathComponent("fixtures")

private func fonts() -> FontSource {
    .embedded(directory: fixtures.appendingPathComponent("fonts"), family: "Crimson Text")
}

private func scratch(_ name: String) throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "chapbook-swift-sync-\(ProcessInfo.processInfo.processIdentifier)-\(name)")
    try? FileManager.default.removeItem(at: dir)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}

/// Open a book, move, suspend: the library now holds a position that has
/// never reached a service, which is exactly what makes the first
/// reconcile a push. Returns the library row.
private func stockedDirtyBook(_ dir: URL) throws -> Int64 {
    let session = try Session(
        source: .path(fixtures.appendingPathComponent("cbz/minimal.cbz")),
        configuration: SessionConfiguration(fonts: fonts(), libraryDirectory: dir))
    try session.setMetrics(
        PageMetrics(
            width: 600, height: 800,
            marginTop: 40, marginRight: 40, marginBottom: 40, marginLeft: 40,
            dpiScale: 1))
    try session.nextPage()
    try session.suspend()
    return try #require(try session.bookID())
}

private final class MethodLog: @unchecked Sendable {
    private let lock = NSLock()
    private var seen: [String] = []
    func record(_ method: String) { lock.withLock { seen.append(method) } }
    var methods: [String] { lock.withLock { seen } }
    func reset() { lock.withLock { seen = [] } }
}

/// A progression service that accepts everything: GET answers 200 with an
/// empty body ("nothing recorded yet", the draft's own shape), and a
/// write answers 204 ("stored"). `canInit` says yes to everything, so
/// nothing in these tests can reach a real network.
private final class AcceptingService: URLProtocol {
    static let log = MethodLog()

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        let method = request.httpMethod ?? "GET"
        Self.log.record(method)
        guard let url = request.url else { return }
        let response = HTTPURLResponse(
            url: url, statusCode: method == "GET" ? 200 : 204,
            httpVersion: "HTTP/1.1", headerFields: [:])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocolDidFinishLoading(self)
    }
}

/// A service that is down, URLProtocol-shaped.
private final class DeadService: URLProtocol {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        client?.urlProtocol(
            self,
            didFailWithError: NSError(
                domain: NSURLErrorDomain, code: NSURLErrorCannotConnectToHost))
    }
}

private func stubbedSession(_ stub: URLProtocol.Type) -> URLSession {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [stub]
    return URLSession(configuration: configuration)
}

/// Drain until the batch finishes, on the worker's own pace.
private func reports(from worker: SyncWorker, within seconds: TimeInterval = 10) throws
    -> [SyncReport]
{
    var drained: [SyncReport] = []
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        drained += try worker.drainReports()
        if drained.contains(where: { if case .finished = $0 { true } else { false } }) {
            return drained
        }
        Thread.sleep(forTimeInterval: 0.02)
    }
    Issue.record("no finished report within \(seconds)s: \(drained)")
    return drained
}

@Test func syncTargetsRoundTripThroughTheLibrary() throws {
    #expect(Capabilities.current().contains(.sync))

    let dir = try scratch("targets")
    defer { try? FileManager.default.removeItem(at: dir) }
    let book = try stockedDirtyBook(dir)
    let library = try Library(directory: dir)

    // A sideloaded book has no services, and that is an ordinary nil.
    #expect(library.syncProgressionURL(book: book) == nil)
    #expect(library.syncAnnotationContainer(book: book) == nil)

    let progression = URL(string: "https://sync.example.com/progression/9")!
    let container = URL(string: "https://sync.example.com/marks/")!
    try library.setSyncTargets(
        book: book, progressionURL: progression, annotationContainer: container)
    #expect(library.syncProgressionURL(book: book) == progression)
    #expect(library.syncAnnotationContainer(book: book) == container)

    // Two nils make it local again.
    try library.setSyncTargets(book: book, progressionURL: nil, annotationContainer: nil)
    #expect(library.syncProgressionURL(book: book) == nil)
    #expect(library.syncAnnotationContainer(book: book) == nil)
}

@Test func aDirtyPositionPushesThroughAnInjectedURLSession() throws {
    let dir = try scratch("push")
    defer { try? FileManager.default.removeItem(at: dir) }
    let book = try stockedDirtyBook(dir)

    do {
        let library = try Library(directory: dir)
        try library.setSyncTargets(
            book: book,
            progressionURL: URL(string: "https://sync.example.com/progression/9")!,
            annotationContainer: nil)
    }

    AcceptingService.log.reset()
    let woke = Counter()
    let worker = try SyncWorker(
        libraryDirectory: dir,
        deviceID: "swift-test-device", deviceName: "swift test",
        transport: .urlSession(stubbedSession(AcceptingService.self)),
        onWake: { woke.increment() })
    try worker.requestAll()

    let drained = try reports(from: worker)
    guard case .book(let report)? = drained.first else {
        Issue.record("expected a book report first: \(drained)")
        return
    }
    #expect(report.book == book)
    // The never-synced position owed the service a write, and the write
    // went out as a PUT through the stubbed session — the send half of
    // the transport, proven end to end.
    #expect(report.position == .pushed)
    #expect(report.detail == nil)
    #expect(report.marksError == nil, "no container was recorded")
    #expect(AcceptingService.log.methods.contains("PUT"))

    #expect(drained.last == .finished(books: 1))
    #expect(woke.value >= 1, "the waker nudged per report")
}

@Test func aDeadServiceCrossesAsAReportNotADeadBatch() throws {
    let dir = try scratch("dead")
    defer { try? FileManager.default.removeItem(at: dir) }
    let book = try stockedDirtyBook(dir)

    do {
        let library = try Library(directory: dir)
        try library.setSyncTargets(
            book: book,
            progressionURL: URL(string: "https://sync.example.com/progression/9")!,
            annotationContainer: nil)
    }

    let worker = try SyncWorker(
        libraryDirectory: dir,
        deviceID: "swift-test-device", deviceName: "swift test",
        transport: .urlSession(stubbedSession(DeadService.self)))
    try worker.requestBook(book)

    let drained = try reports(from: worker)
    guard case .book(let report)? = drained.first else {
        Issue.record("expected a book report first: \(drained)")
        return
    }
    #expect(report.book == book)
    // The transfer produced nothing, and that is the position's failure,
    // not the batch's — the platform error's own words cross as detail.
    #expect(report.position == .failed)
    #expect(report.detail?.isEmpty == false)
    #expect(drained.last == .finished(books: 1))

    // A book the caller names that has nothing to sync answers by name.
    try worker.requestBook(book + 999)
    let failed = try reports(from: worker)
    guard case .bookFailed(let id, let reason)? = failed.first else {
        Issue.record("expected a bookFailed report: \(failed)")
        return
    }
    #expect(id == book + 999)
    #expect(!reason.isEmpty)
}

private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    func increment() { lock.withLock { count += 1 } }
    var value: Int { lock.withLock { count } }
}
