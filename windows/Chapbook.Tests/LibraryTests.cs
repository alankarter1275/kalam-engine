using Chapbook;
using Xunit;

namespace Chapbook.Tests;

public class LibraryTests
{
    /// <summary>
    /// Put two books on a shelf the only way there is: by opening them.
    /// There is no import call, which is the shelf's whole shape — a book
    /// arrives by being read.
    /// </summary>
    private static string Shelf(string dir)
    {
        foreach (string book in new[] { "long.epub", "minimal.epub" })
        {
            using var session = Session.OpenPath(Fixture.Book(book), Fixture.Config(dir));
            session.SetMetrics(Fixture.Metrics);
            _ = session.Render();
            session.Suspend();
        }
        return dir;
    }

    [Fact]
    public void ABookReachesTheShelfByBeingOpened()
    {
        string dir = Fixture.Scratch();
        long id;
        using (var session = Session.OpenPath(Fixture.Book("long.epub"), Fixture.Config(dir)))
        {
            session.SetMetrics(Fixture.Metrics);
            id = session.BookId ?? throw new Xunit.Sdk.XunitException("no row");
        }

        using var library = new Library(dir);
        ShelfBook book = Assert.Single(library.Books());

        // The row the session said it was.
        Assert.Equal(id, book.Id);
        Assert.Equal("The Long Book", book.Title);
        Assert.NotEmpty(book.Fingerprint);
        Assert.NotNull(book.FilePath);
        Assert.True(book.AddedAt > DateTimeOffset.UnixEpoch);
    }

    [Fact]
    public void AnEmptyShelfIsAnEmptyListAndNotAFailure()
    {
        using var library = new Library(Fixture.Scratch());
        Assert.Empty(library.Books());
        Assert.Empty(library.Collections());
    }

    [Fact]
    public void SearchFoldsCaseAndMatchesWholeWordsByPrefix()
    {
        using var library = new Library(Shelf(Fixture.Scratch()));

        Assert.Equal(2, library.Books().Count);
        Assert.Single(library.Books(new ShelfQuery { Search = "long" }));
        Assert.Single(library.Books(new ShelfQuery { Search = "LONG" }));

        // Punctuation alone matches no book rather than every book, which
        // is what a search field wants while someone is still typing.
        Assert.Empty(library.Books(new ShelfQuery { Search = "..." }));
        Assert.Empty(library.Books(new ShelfQuery { Search = "nothingmatchesthis" }));
    }

    [Fact]
    public void PagingAndSortingAreTheQuerysAndNotTheHosts()
    {
        using var library = new Library(Shelf(Fixture.Scratch()));

        IReadOnlyList<ShelfBook> byTitle = library.Books(new ShelfQuery { Sort = ShelfSort.Title });
        Assert.Equal(2, byTitle.Count);
        Assert.Equal(
            byTitle.Select(b => b.Title).OrderBy(t => t, StringComparer.Ordinal),
            byTitle.Select(b => b.Title));

        Assert.Single(library.Books(new ShelfQuery { Limit = 1 }));
        Assert.Single(library.Books(new ShelfQuery { Limit = 1, Offset = 1 }));
        Assert.Empty(library.Books(new ShelfQuery { Limit = 1, Offset = 2 }));
    }

    [Fact]
    public void ReadingStateFollowsMarkingABookFinished()
    {
        string dir = Shelf(Fixture.Scratch());
        using var library = new Library(dir);
        ShelfBook first = library.Books(new ShelfQuery { Sort = ShelfSort.Title })[0];

        library.SetFinished(first.Id, true);
        ShelfBook finished = Assert.Single(
            library.Books(new ShelfQuery { State = ReadingState.Finished }));
        Assert.Equal(first.Id, finished.Id);
        Assert.NotNull(finished.FinishedAt);

        library.SetFinished(first.Id, false);
        Assert.Empty(library.Books(new ShelfQuery { State = ReadingState.Finished }));
    }

    [Fact]
    public void CollectionsGroupBooksAndSurviveTheirMembers()
    {
        string dir = Shelf(Fixture.Scratch());
        using var library = new Library(dir);
        ShelfBook book = library.Books()[0];

        long shelf = library.CreateCollection("To read");
        library.AddToCollection(book.Id, shelf);

        Collection collection = Assert.Single(library.Collections());
        Assert.Equal("To read", collection.Name);
        Assert.Equal(1, collection.Books);

        ShelfBook inIt = Assert.Single(library.Books(new ShelfQuery { Collection = shelf }));
        Assert.Equal(book.Id, inIt.Id);
        Assert.Contains(inIt.Collections, c => c.Id == shelf && c.Name == "To read");

        library.RenameCollection(shelf, "Later");
        Assert.Equal("Later", library.Collections()[0].Name);

        library.RemoveFromCollection(book.Id, shelf);
        Assert.Empty(library.Books(new ShelfQuery { Collection = shelf }));

        // Deleting a collection is not deleting its books.
        library.DeleteCollection(shelf);
        Assert.Empty(library.Collections());
        Assert.Equal(2, library.Books().Count);
    }

    [Fact]
    public void ABookLearnsWhereItSyncsAndCanForgetAgain()
    {
        string dir = Shelf(Fixture.Scratch());
        using var library = new Library(dir);
        long book = library.Books()[0].Id;

        // A sideloaded book has no entry and so no service. That is the
        // ordinary state, not a missing configuration.
        Assert.Null(library.SyncProgressionUrl(book));
        Assert.Null(library.SyncAnnotationContainer(book));

        library.SetSyncTargets(
            book, "https://example.invalid/progression/1", "https://example.invalid/marks/1");
        Assert.Equal("https://example.invalid/progression/1", library.SyncProgressionUrl(book));
        Assert.Equal("https://example.invalid/marks/1", library.SyncAnnotationContainer(book));

        // Two nulls make it local again.
        library.SetSyncTargets(book, null, null);
        Assert.Null(library.SyncProgressionUrl(book));
        Assert.Null(library.SyncAnnotationContainer(book));
    }

    [Fact]
    public void AShelfIsReadableWhileABookIsOpen()
    {
        string dir = Shelf(Fixture.Scratch());

        // Two connections on one directory is the ordinary way to draw a
        // shelf while a book is being read; the database is WAL and this
        // is what makes a library different from a session.
        using var session = Session.OpenPath(Fixture.Book("long.epub"), Fixture.Config(dir));
        session.SetMetrics(Fixture.Metrics);
        using var library = new Library(dir);

        Assert.Equal(2, library.Books().Count);
        Assert.Contains(library.Books(), b => b.Id == session.BookId);
    }

    [Fact]
    public void DeletingABookTakesItOffTheShelf()
    {
        string dir = Shelf(Fixture.Scratch());
        using var library = new Library(dir);
        long book = library.Books()[0].Id;

        library.DeleteBook(book);
        Assert.Single(library.Books());
        Assert.DoesNotContain(library.Books(), b => b.Id == book);
    }
}
