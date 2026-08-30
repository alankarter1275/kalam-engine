import CoreGraphics
import Darwin
import Foundation
import Testing

@testable import Chapbook

// The macOS slice exists so these run with no simulator in the loop; the
// boundary they drive is byte-for-byte the one the iOS slices carry.

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

private func scratchLibrary(_ name: String) throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("chapbook-swift-test-\(ProcessInfo.processInfo.processIdentifier)-\(name)")
    try? FileManager.default.removeItem(at: dir)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}

private func metrics() -> PageMetrics {
    PageMetrics(
        width: 600, height: 800,
        marginTop: 40, marginRight: 40, marginBottom: 40, marginLeft: 40,
        dpiScale: 1)
}

@Test func aBookOpensReadsAndRenders() throws {
    let library = try scratchLibrary("open")
    defer { try? FileManager.default.removeItem(at: library) }

    let session = try Session(
        source: .path(fixtures.appendingPathComponent("epub/minimal.epub")),
        configuration: SessionConfiguration(fonts: fonts(), libraryDirectory: library))
    try session.setMetrics(metrics())

    #expect(session.title()?.isEmpty == false)
    #expect(try session.pageCount() > 0)
    #expect(try session.fontFaceCount() > 0)
    #expect(try session.readingDirection() == .leftToRight)

    let (w, h) = try session.renderSize()
    let image = try session.renderImage()
    #expect(image.width == w && image.height == h)
}

@Test func aBadPathIsAnErrorWithASentence() {
    #expect(throws: ChapbookError.self) {
        try Session(
            source: .path(URL(fileURLWithPath: "/nonexistent/book.epub")),
            configuration: SessionConfiguration(fonts: fonts()))
    }
}

@Test func tapsResolveThroughTheBook() throws {
    let library = try scratchLibrary("taps")
    defer { try? FileManager.default.removeItem(at: library) }

    let session = try Session(
        source: .path(fixtures.appendingPathComponent("epub/minimal.epub")),
        configuration: SessionConfiguration(fonts: fonts(), libraryDirectory: library))
    try session.setMetrics(metrics())

    #expect(try session.tapAction(at: CGPoint(x: 100, y: 400)) == .prevPage)
    #expect(try session.tapAction(at: CGPoint(x: 500, y: 400)) == .nextPage)
    #expect(try session.tapAction(at: CGPoint(x: 300, y: 400)) == .toggleMenu)

    // Page zero: prev applies, nothing moves, the event is still ours.
    #expect(try session.apply(.prevPage) == .unchanged)
    // The engine has no chrome; the menu is the app's.
    #expect(try session.apply(.toggleMenu) == .unhandled)
}

@Test func theDefaultTablesAnswerWithNoSession() {
    #expect(Action.default(for: .pageDown) == .nextPage)
    #expect(Action.default(for: .turnNext) == .nextPage)
    #expect(Action.default(for: "n") == .nextUnit)
    #expect(Action.default(for: "q") == nil)
}

@Test func aDescriptorBookKeepsItsPlace() throws {
    // The custody loop as an app runs it, through the Swift layer: open
    // from a descriptor, read, suspend, reopen from a fresh descriptor,
    // come back to the same page. The engine adopts the book by its
    // bytes' fingerprint; no path ever crosses.
    let library = try scratchLibrary("fd-place")
    defer { try? FileManager.default.removeItem(at: library) }
    let comic = fixtures.appendingPathComponent("cbz/minimal.cbz")
    let configuration = SessionConfiguration(fonts: fonts(), libraryDirectory: library)

    let descriptor = { Darwin.open(comic.path, O_RDONLY) }

    do {
        let session = try Session(
            source: .fileDescriptor(descriptor()), configuration: configuration)
        try session.setMetrics(metrics())
        try session.nextPage()
        try session.nextPage()
        #expect(try session.position().spine == 2)
        try session.suspend()
    }

    let reopened = try Session(
        source: .fileDescriptor(descriptor()), configuration: configuration)
    try reopened.setMetrics(metrics())
    #expect(try reopened.position().spine == 2, "the place survived a cold reopen")
}

@Test func theBuildSaysWhatItCanDo() {
    #expect(Capabilities.current().contains(.library))
    #expect(engineABIVersion() > 0)
}

@Test func thePageSpeaksItsText() throws {
    let library = try scratchLibrary("text-surface")
    defer { try? FileManager.default.removeItem(at: library) }

    let session = try Session(
        source: .path(fixtures.appendingPathComponent("epub/minimal.epub")),
        configuration: SessionConfiguration(fonts: fonts(), libraryDirectory: library))

    // Nothing laid out yet: nil, not empty.
    #expect(try session.pageTextRuns() == nil)
    #expect(try session.speakablePage() == nil)

    try session.setMetrics(metrics())
    _ = try session.pageCount()  // forces the layout

    let runs = try #require(session.pageTextRuns())
    #expect(!runs.isEmpty)
    for run in runs {
        #expect(!run.text.isEmpty)
        #expect(run.rect.width > 0 && run.rect.height > 0)
    }

    let page = try #require(session.speakablePage())
    #expect(!page.text.isEmpty)
    #expect(!page.words.isEmpty)
    for word in page.words {
        #expect(word.textRange.upperBound <= UInt32(page.text.count))
    }

    // The first word has geometry a highlight can paint.
    let first = try #require(page.words.first)
    #expect(try !session.rects(for: first.locators).isEmpty)
}
