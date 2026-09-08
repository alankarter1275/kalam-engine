import Foundation
import Testing

@testable import Chapbook

// The injected-URLSession path, run with no socket: a URLProtocol stub is
// the canned catalog, so what is being proven is the whole crossing — the
// engine's request becomes a URLRequest the session actually performs,
// and the stub's response becomes the book. The Rust half of the seam has
// its own suite (chapbook-ffi's tests/http.rs); this is the Swift half.

private let host = "https://shelf.example.com"

private func lazyFeed() -> Data {
    Data(
        """
        <?xml version="1.0"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <id>urn:cat:comics</id><title>Comics</title>
          <entry><id>urn:c1</id><title>Test Comic</title>
            <link rel="alternate" href="\(host)/entry" type="application/atom+xml;type=entry;profile=opds-catalog"/>
          </entry>
        </feed>
        """.utf8)
}

private func completeEntry() -> Data {
    Data(
        """
        <?xml version="1.0"?>
        <entry xmlns="http://www.w3.org/2005/Atom" xmlns:pse="http://vaemendis.net/opds-pse/ns">
          <id>urn:c1</id><title>Test Comic</title>
          <link rel="http://vaemendis.net/opds-pse/stream"
                href="\(host)/pages?page={pageNumber}&amp;width={maxWidth}"
                type="image/jpeg" pse:count="3"/>
        </entry>
        """.utf8)
}

private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    func increment() { lock.withLock { count += 1 } }
    var value: Int { lock.withLock { count } }
}

/// The catalog, one URLProtocol deep. `canInit` says yes to everything,
/// so nothing in these tests can reach a real network.
private final class CatalogStub: URLProtocol {
    static let hits = Counter()

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        Self.hits.increment()
        guard let url = request.url else { return }
        let (status, contentType, body): (Int, String, Data) =
            switch url.path {
            case "/feed": (200, "application/atom+xml;profile=opds-catalog", lazyFeed())
            case "/entry": (200, "application/atom+xml;type=entry", completeEntry())
            case "/pages": (200, "image/jpeg", Data("JPEGDATA".utf8))
            default: (404, "text/plain", Data("not found".utf8))
            }
        let response = HTTPURLResponse(
            url: url, statusCode: status, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": contentType])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }
}

/// A network that is down, URLProtocol-shaped.
private final class UnpluggedStub: URLProtocol {
    static let hits = Counter()

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        Self.hits.increment()
        client?.urlProtocol(
            self,
            didFailWithError: NSError(
                domain: NSURLErrorDomain, code: NSURLErrorNotConnectedToInternet))
    }
}

private func stubbedSession(_ stub: URLProtocol.Type) -> URLSession {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [stub]
    return URLSession(configuration: configuration)
}

private func fixturesFonts() -> FontSource {
    let fixtures = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // ChapbookTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // Chapbook
        .deletingLastPathComponent()  // ios
        .deletingLastPathComponent()  // repo root
        .appendingPathComponent("fixtures/fonts")
    return .embedded(directory: fixtures, family: "Crimson Text")
}

private func scratch(_ name: String) throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "chapbook-transport-test-\(ProcessInfo.processInfo.processIdentifier)-\(name)")
    try? FileManager.default.removeItem(at: dir)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}

@Test func aCatalogOpensThroughAnInjectedURLSession() throws {
    let library = try scratch("opens")
    defer { try? FileManager.default.removeItem(at: library) }

    let session = try Session(
        source: .catalog(URL(string: "\(host)/feed")!),
        configuration: SessionConfiguration(
            fonts: fixturesFonts(), libraryDirectory: library,
            transport: .urlSession(stubbedSession(CatalogStub.self))))

    // The feed and its complete entry both went through the stub, which
    // is the proof the injected session carried the engine's requests.
    #expect(CatalogStub.hits.value >= 2)
    #expect(session.title() == "Test Comic")
    #expect(try session.spineLength() == 3)
}

@Test func aDeadNetworkFailsTheOpenAndProvesItWasAsked() throws {
    let library = try scratch("unplugged")
    defer { try? FileManager.default.removeItem(at: library) }

    #expect(throws: ChapbookError.self) {
        try Session(
            source: .catalog(URL(string: "\(host)/feed")!),
            configuration: SessionConfiguration(
                fonts: fixturesFonts(), libraryDirectory: library,
                transport: .urlSession(stubbedSession(UnpluggedStub.self))))
    }
    #expect(UnpluggedStub.hits.value >= 1)
}

@Test func theDefaultTransportMatchesThePlatform() {
    // ureq stays the macOS answer; iOS defaults to URLSession. This test
    // runs on macOS, so what it can pin is its own half — and that the
    // bundled transport really is in this build for the default to mean
    // anything.
    #if os(macOS)
        guard case .bundled = HTTPTransport.platformDefault.kind else {
            Issue.record("macOS should default to the bundled transport")
            return
        }
        #expect(Capabilities.current().contains(.bundledHTTP))
    #else
        guard case .urlSession = HTTPTransport.platformDefault.kind else {
            Issue.record("iOS should default to URLSession")
            return
        }
    #endif
}
