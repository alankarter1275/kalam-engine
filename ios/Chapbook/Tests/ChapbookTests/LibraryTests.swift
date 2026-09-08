import Foundation
import Testing

@testable import Chapbook

// The shelf, driven the way an app draws one: open books to put them
// there, then browse. The macOS slice runs these with no simulator; the
// boundary is byte-for-byte the one the iOS slices carry.

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
            "chapbook-swift-shelf-\(ProcessInfo.processInfo.processIdentifier)-\(name)")
    try? FileManager.default.removeItem(at: dir)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}

/// A book reaches the library by being opened, so stocking the shelf is
/// three reads. Returns the directory they landed in.
private func stocked(_ name: String) throws -> URL {
    let dir = try scratch(name)
    for book in ["epub/minimal.epub", "epub/series.epub", "epub/series-legacy.epub"] {
        _ = try Session(
            source: .path(fixtures.appendingPathComponent(book)),
            configuration: SessionConfiguration(fonts: fonts(), libraryDirectory: dir))
    }
    return dir
}

@Test func theShelfListsWhatWasOpened() throws {
    let dir = try stocked("list")
    defer { try? FileManager.default.removeItem(at: dir) }

    let library = try Library(directory: dir)
    let books = try library.books()
    #expect(books.count == 3)
    // A default query is the whole shelf, newest first.
    #expect(books.allSatisfy { $0.state == .unread })
    #expect(books.allSatisfy { !$0.fingerprint.isEmpty })
    #expect(books.allSatisfy { $0.fileURL != nil })
}

@Test func aSearchFoldsCaseAndAccentsAndReachesTheSeries() throws {
    let dir = try stocked("search")
    defer { try? FileManager.default.removeItem(at: dir) }
    let library = try Library(directory: dir)

    let cycle = try library.books(Library.Query(search: "fixture cycle", sort: .series))
    #expect(cycle.count == 2)
    #expect(cycle.first?.series == "The Fixture Cycle")
    // Sorted by position within the series, and the position is
    // fractional because `group-position` is.
    #expect(cycle.first?.seriesIndex == 1)
    #expect(cycle.last?.seriesIndex == 2.5)

    // A book in no series says so with nil rather than an empty string.
    let minimal = try library.books(Library.Query(search: "minimal"))
    #expect(minimal.count == 1)
    #expect(minimal.first?.series == nil)
    #expect(minimal.first?.seriesIndex == nil)
    #expect(minimal.first?.language == "en")

    // A reader typing a real title is not composing a query language.
    #expect(try library.books(Library.Query(search: "Legacy-Fixture")).count == 1)
    #expect(try library.books(Library.Query(search: "!!!")).isEmpty)
}

@Test func collectionsGroupBooksAndShowUpOnTheRows() throws {
    let dir = try stocked("collections")
    defer { try? FileManager.default.removeItem(at: dir) }
    let library = try Library(directory: dir)

    #expect(try library.collections().isEmpty)
    let shelf = try library.createCollection(named: "To Reread")
    // Idempotent on the name.
    #expect(try library.createCollection(named: "To Reread") == shelf)

    let first = try #require(try library.books().first)
    try library.add(book: first.id, to: shelf)

    let listed = try library.collections()
    #expect(listed.count == 1)
    #expect(listed.first?.name == "To Reread")
    #expect(listed.first?.bookCount == 1)

    let members = try library.books(Library.Query(collection: shelf))
    #expect(members.count == 1)
    #expect(members.first?.collections.map(\.name) == ["To Reread"])

    // Deleting takes the grouping, not the books.
    try library.deleteCollection(shelf)
    #expect(try library.collections().isEmpty)
    #expect(try library.books().count == 3)
}

@Test func finishingIsRecordedAndIsNotProgress() throws {
    let dir = try scratch("finish")
    defer { try? FileManager.default.removeItem(at: dir) }

    let session = try Session(
        source: .path(fixtures.appendingPathComponent("epub/minimal.epub")),
        configuration: SessionConfiguration(fonts: fonts(), libraryDirectory: dir))
    let id = try #require(try session.bookID())

    let library = try Library(directory: dir)
    try library.setFinished(true, book: id)

    let finished = try library.books(Library.Query(state: .finished))
    #expect(finished.count == 1)
    #expect(finished.first?.id == id)
    #expect(finished.first?.finishedAt != nil)
    // Never opened far enough to have one, and finished all the same:
    // the two are different questions.
    #expect(finished.first?.progress == nil)

    try library.setFinished(false, book: id)
    #expect(try library.books(Library.Query(state: .finished)).isEmpty)

    // Soft removal keeps the row, so the shelf empties without the
    // book's history going with it.
    try library.delete(book: id)
    #expect(try library.books().isEmpty)
}
