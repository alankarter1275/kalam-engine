using System.Collections.Concurrent;
using System.Text;
using Chapbook;
using Xunit;

namespace Chapbook.Tests;

/// <summary>
/// A transport that answers from a table and remembers what it was asked.
/// </summary>
/// <remarks>
/// No socket anywhere, and that is the point rather than a shortcut: the
/// transport <i>is</i> the seam, so a fake one exercises the whole sync
/// path — worker thread, callbacks, response builders, reports — with
/// nothing to flake. A real <see cref="HttpClientTransport"/> is the same
/// two methods over <see cref="HttpClient"/>.
///
/// It is touched from the worker's thread and read from the test's, which
/// is exactly the contract's "may fire on any thread", so the log is
/// concurrent.
/// </remarks>
internal sealed class FakeTransport : HttpTransport
{
    internal readonly record struct Call(string Method, string Url, string Body);

    internal ConcurrentQueue<Call> Calls { get; } = new();

    /// <summary>Set from the ABI's finalizer when the engine lets go.</summary>
    internal int Released;

    // `protected internal` here only because this assembly is
    // internals-visible for the layout gate. A host, which is not, writes
    // `protected override` — outside the declaring assembly the internal
    // half is invisible and C# refuses to widen it.
    protected internal override void OnReleased() => Interlocked.Increment(ref Released);

    /// <summary>What to answer, by URL. Anything unlisted is a 404.</summary>
    internal Dictionary<string, (ushort Status, string ContentType, string Body)> Answers { get; }
        = new();

    /// <summary>Headers to report back on every answer.</summary>
    internal Dictionary<string, string> ResponseHeaders { get; } = new();

    public override void Get(in HttpRequest request, HttpResponseBuilder response)
    {
        Calls.Enqueue(new Call("GET", request.Url, string.Empty));
        Answer(request.Url, response);
    }

    public override void Send(
        string method, in HttpRequest request, ReadOnlySpan<byte> body,
        HttpResponseBuilder response)
    {
        Calls.Enqueue(new Call(method, request.Url, Encoding.UTF8.GetString(body)));
        Answer(request.Url, response);
    }

    private void Answer(string url, HttpResponseBuilder response)
    {
        if (Answers.TryGetValue(url, out var answer))
        {
            response.SetStatus(answer.Status);
            response.SetContentType(answer.ContentType);
            foreach ((string name, string value) in ResponseHeaders)
            {
                response.AddHeader(name, value);
            }
            response.AppendBody(Encoding.UTF8.GetBytes(answer.Body));
            return;
        }
        response.SetStatus(404);
        response.SetContentType("text/plain");
        response.AppendBody("no"u8);
    }
}

public class TransportTests
{
    [Fact]
    public void ASessionFetchesThroughTheHostsTransport()
    {
        var transport = new FakeTransport();
        using var config = new SessionConfiguration(FontSource.Host())
            .WithLibraryDirectory(Fixture.Scratch())
            .WithTransport(transport);

        // An OPDS URL is the one source that has to reach the network to
        // open at all, so this proves the `get` callback is wired without
        // needing a server: the engine asks, the transport answers 404,
        // and the open fails for that reason rather than for a missing
        // transport.
        var error = Assert.Throws<ChapbookException>(
            () => Session.OpenUrl("https://example.invalid/catalog.atom", config));
        Assert.NotEqual(Status.Ok, error.Status);

        FakeTransport.Call call = Assert.Single(transport.Calls);
        Assert.Equal("GET", call.Method);
        Assert.Equal("https://example.invalid/catalog.atom", call.Url);
    }

    [Fact]
    public void TheEngineFinalizesATransportItWasGivenAndNeverOpened()
    {
        var transport = new FakeTransport();
        var config = new SessionConfiguration(FontSource.Host())
            .WithLibraryDirectory(Fixture.Scratch())
            .WithTransport(transport);

        Assert.Equal(0, transport.Released);

        // Disposing a configuration that never became a session still
        // releases the transport. That is what the finalizer in the ABI is
        // for, and getting it wrong leaks a GC handle per attempt with
        // nothing to notice it.
        config.Dispose();
        Assert.Equal(1, transport.Released);
    }
}

public class SyncTests
{
    private const string Device = "11111111-2222-3333-4444-555555555555";

    /// <summary>Open two books so the shelf has rows to sync.</summary>
    private static string Shelf(string dir)
    {
        foreach (string book in new[] { "long.epub", "minimal.epub" })
        {
            using var session = Session.OpenPath(Fixture.Book(book), Fixture.Config(dir));
            session.SetMetrics(Fixture.Metrics);
            _ = session.Render();
            session.NextPage();
            session.Suspend();
        }
        return dir;
    }

    /// <summary>Drain until the batch says it finished, or give up.</summary>
    private static IReadOnlyList<SyncReport> Await(SyncWorker worker, int millis = 10_000)
    {
        var reports = new List<SyncReport>();
        DateTime deadline = DateTime.UtcNow.AddMilliseconds(millis);
        while (DateTime.UtcNow < deadline)
        {
            reports.AddRange(worker.DrainReports());
            if (reports.Any(r => r.Kind == SyncReportKind.Finished))
            {
                return reports;
            }
            Thread.Sleep(10);
        }
        Assert.Fail($"the batch never finished; saw {reports.Count} reports");
        return reports;
    }

    [Fact]
    public void AShelfWhereNothingSyncsFinishesImmediately()
    {
        string dir = Shelf(Fixture.Scratch());
        var transport = new FakeTransport();
        using var worker = new SyncWorker(dir, Device, "test", transport);

        worker.RequestAll();
        IReadOnlyList<SyncReport> reports = Await(worker);

        // Zero books is a fact to tell the reader, not a spinner to show
        // them — and no book means no request.
        SyncReport finished = Assert.Single(reports, r => r.Kind == SyncReportKind.Finished);
        Assert.Equal(0, finished.Books);
        Assert.Empty(transport.Calls);
    }

    [Fact]
    public void ABookWithAServiceReachesItThroughTheHostsTransport()
    {
        string dir = Shelf(Fixture.Scratch());
        long book;
        using (var library = new Library(dir))
        {
            book = library.Books()[0].Id;
            library.SetSyncTargets(book, "https://example.invalid/progression/1", null);
        }

        var transport = new FakeTransport();
        using var worker = new SyncWorker(dir, Device, "test", transport);
        worker.RequestAll();
        IReadOnlyList<SyncReport> reports = Await(worker);

        // The whole path: worker thread → the callback this process
        // supplied → a report back on the queue.
        Assert.Contains(transport.Calls, c => c.Url == "https://example.invalid/progression/1");
        SyncReport report = Assert.Single(reports, r => r.Kind == SyncReportKind.Book);
        Assert.Equal(book, report.Book);

        SyncReport finished = Assert.Single(reports, r => r.Kind == SyncReportKind.Finished);
        Assert.Equal(1, finished.Books);
    }

    [Fact]
    public void AnUnreachableServiceIsOneBooksProblemAndNotTheBatchs()
    {
        string dir = Shelf(Fixture.Scratch());
        using (var library = new Library(dir))
        {
            foreach (ShelfBook b in library.Books())
            {
                library.SetSyncTargets(b.Id, $"https://example.invalid/progression/{b.Id}", null);
            }
        }

        var transport = new FakeTransport();
        using var worker = new SyncWorker(dir, Device, "test", transport);
        worker.RequestAll();
        IReadOnlyList<SyncReport> reports = Await(worker);

        // Both books reported, and the batch still finished. One dead host
        // must not read as a dead batch — which is why a failure inside a
        // book lives in the report rather than stopping the run.
        Assert.Equal(2, reports.Count(r => r.Kind == SyncReportKind.Book));
        Assert.Equal(2, Assert.Single(reports, r => r.Kind == SyncReportKind.Finished).Books);
        Assert.All(
            reports.Where(r => r.Kind == SyncReportKind.Book),
            r => Assert.Equal(PositionOutcome.Failed, r.Position));
        Assert.All(
            reports.Where(r => r.Kind == SyncReportKind.Book),
            r => Assert.NotNull(r.Detail));
    }

    [Fact]
    public void AskingForOneBookAsksForOnlyThatBook()
    {
        string dir = Shelf(Fixture.Scratch());
        long first;
        using (var library = new Library(dir))
        {
            IReadOnlyList<ShelfBook> books = library.Books();
            first = books[0].Id;
            foreach (ShelfBook b in books)
            {
                library.SetSyncTargets(b.Id, $"https://example.invalid/progression/{b.Id}", null);
            }
        }

        var transport = new FakeTransport();
        using var worker = new SyncWorker(dir, Device, "test", transport);
        worker.RequestBook(first);
        IReadOnlyList<SyncReport> reports = Await(worker);

        SyncReport report = Assert.Single(reports, r => r.Kind == SyncReportKind.Book);
        Assert.Equal(first, report.Book);
        Assert.DoesNotContain(transport.Calls, c => c.Url.EndsWith($"/{first + 1}"));
    }

    [Fact]
    public void ABookThatHasLeftTheShelfIsReportedRatherThanSkipped()
    {
        string dir = Shelf(Fixture.Scratch());
        long gone;
        using (var library = new Library(dir))
        {
            gone = library.Books()[0].Id;
            library.DeleteBook(gone);
        }

        var transport = new FakeTransport();
        using var worker = new SyncWorker(dir, Device, "test", transport);
        worker.RequestBook(gone);
        IReadOnlyList<SyncReport> reports = Await(worker);

        SyncReport failed = Assert.Single(reports, r => r.Kind == SyncReportKind.BookFailed);
        Assert.Equal(gone, failed.Book);
        Assert.False(string.IsNullOrEmpty(failed.Detail));
    }

    [Fact]
    public void TheWakerFiresOnTheWorkerThreadAndOnlyNudges()
    {
        string dir = Shelf(Fixture.Scratch());
        using (var library = new Library(dir))
        {
            library.SetSyncTargets(
                library.Books()[0].Id, "https://example.invalid/progression/1", null);
        }

        int wakes = 0;
        int mainThread = Environment.CurrentManagedThreadId;
        int wakeThread = mainThread;
        using var worker = new SyncWorker(
            dir, Device, "test", new FakeTransport(),
            onWake: () =>
            {
                Interlocked.Increment(ref wakes);
                Volatile.Write(ref wakeThread, Environment.CurrentManagedThreadId);
            });

        worker.RequestAll();
        Await(worker);

        Assert.True(wakes > 0, "the worker should have nudged at least once");
        // The contract's own words: it fires on the worker thread. A host
        // that touched its UI here would be touching it from the wrong one.
        Assert.NotEqual(mainThread, Volatile.Read(ref wakeThread));
    }

    [Fact]
    public void ClosingTheWorkerReleasesItsTransportExactlyOnce()
    {
        string dir = Shelf(Fixture.Scratch());
        var transport = new FakeTransport();
        var worker = new SyncWorker(dir, Device, "test", transport);

        Assert.Equal(0, transport.Released);
        worker.Dispose();

        // The close joins the worker thread first, so when it returns
        // nothing is inside a callback and the release is safe — and it
        // happens once, not once per request.
        Assert.Equal(1, transport.Released);
        worker.Dispose();
        Assert.Equal(1, transport.Released);
    }

    [Fact]
    public void AnEmptyQueueIsTheOrdinaryAnswerBetweenWakes()
    {
        string dir = Shelf(Fixture.Scratch());
        using var worker = new SyncWorker(dir, Device, "test", new FakeTransport());

        Assert.Null(worker.NextReport());
        Assert.Empty(worker.DrainReports());
    }
}
