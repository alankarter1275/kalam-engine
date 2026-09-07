using System.Runtime.InteropServices;

namespace Chapbook;

/// <summary>What kind of thing a <see cref="SyncReport"/> is.</summary>
public enum SyncReportKind : uint
{
    /// <summary>
    /// A book reconciled. Failures <i>inside</i> it — an unreachable
    /// service, a refused write — live in the report's fields, because one
    /// dead host must not read as a dead batch.
    /// </summary>
    Book = 0,

    /// <summary>
    /// A book did not reconcile at all: removed from the shelf, or with no
    /// service to talk to. The rest of the batch still ran.
    /// </summary>
    BookFailed = 1,

    /// <summary>
    /// A batch finished, and <see cref="SyncReport.Books"/> says how many
    /// reports preceded it. The signal to stop showing a spinner.
    /// </summary>
    Finished = 2,
}

/// <summary>What happened to one book's reading position.</summary>
public enum PositionOutcome : uint
{
    /// <summary>Nothing to do: no service, or nothing had changed either side.</summary>
    Idle = 0,

    /// <summary>This device's position reached the service.</summary>
    Pushed = 1,

    /// <summary>The service's position was adopted locally.</summary>
    Pulled = 2,

    /// <summary>
    /// The service declined; what it holds is newer. Not a failure — the
    /// next pull brings it down if the local copy is clean by then.
    /// </summary>
    Refused = 3,

    /// <summary>
    /// Both sides moved since they last agreed. Nothing was overwritten.
    /// </summary>
    Conflict = 4,

    /// <summary>
    /// The service could not be reached, or answered something unusable.
    /// Nothing local changed.
    /// </summary>
    Failed = 5,
}

/// <summary>One report from the worker.</summary>
/// <param name="Book">The library row, in <see cref="ShelfBook.Id"/>'s space.</param>
/// <param name="Detail">
/// The position's refusal or failure explained, or a failed book's reason.
/// For a person; free to change, so do not match on it.
/// </param>
/// <param name="MarksCreated">Written to the container as new.</param>
/// <param name="MarksUpdated">This device's edits written over the container's copy.</param>
/// <param name="MarksDeleted">Deletes carried out on the container.</param>
/// <param name="MarksAdopted">Pulled down as marks this device had not seen.</param>
/// <param name="MarksRefreshed">Already known here, brought up to date.</param>
/// <param name="MarksMerged">
/// Conflicts settled by re-reading the container — both edits survive and
/// nothing was overwritten.
/// </param>
/// <param name="MarksConflicts">Still owing a write. The next pass tries again.</param>
/// <param name="MarksError">
/// The container could not be reached; whatever was pushed before it
/// failed stands. <c>null</c> when the mark half ran to the end.
/// </param>
/// <param name="Books">
/// <see cref="SyncReportKind.Finished"/> only: how many reports the batch
/// produced.
/// </param>
public readonly record struct SyncReport(
    SyncReportKind Kind,
    long Book,
    PositionOutcome Position,
    string? Detail,
    int MarksCreated,
    int MarksUpdated,
    int MarksDeleted,
    int MarksAdopted,
    int MarksRefreshed,
    int MarksMerged,
    int MarksConflicts,
    string? MarksError,
    int Books);

/// <summary>
/// Reconciles a shelf with the services its books answer to — the position
/// with an OPDS Progression endpoint, marks with a Web Annotation
/// container.
/// </summary>
/// <remarks>
/// <para>
/// Ask with <see cref="RequestAll"/> or <see cref="RequestBook"/>; reports
/// arrive through <see cref="NextReport"/> or <see cref="DrainReports"/>,
/// one per book and then a <see cref="SyncReportKind.Finished"/>. Which
/// services a book has is recorded per book with
/// <see cref="Library.SetSyncTargets"/> off the catalogue entry it was
/// downloaded from.
/// </para>
/// <para>
/// The worker holds <b>its own connection</b> to the library, so it
/// coexists with open sessions and a <see cref="Library"/> on the same
/// directory. It also owns a thread: <see cref="Dispose"/> joins it, which
/// blocks for the book in flight, so dispose it from somewhere that can
/// afford the wait.
/// </para>
/// <para>
/// No credential crosses this boundary. A service behind authentication
/// wants an <see cref="HttpClient"/> whose handler attaches its own.
/// </para>
/// </remarks>
public sealed class SyncWorker : IDisposable
{
    private nint _handle;
    private GCHandle _waker;

    /// <summary>Open a worker over a library directory.</summary>
    /// <param name="libraryDirectory">
    /// The same directory the sessions were configured with.
    /// </param>
    /// <param name="deviceId">
    /// How a progression service tells this device's positions from
    /// another's. Mint one once — a <see cref="Guid"/> is fine — store it,
    /// and pass the same one forever. A device that mints a new id each launch
    /// looks like a new device every launch.
    /// </param>
    /// <param name="deviceName">For people to read.</param>
    /// <param name="transport">
    /// The host's networking. <c>null</c> asks for the bundled transport,
    /// which a build without one declines honestly.
    /// </param>
    /// <param name="onWake">
    /// Fires <b>on the worker thread</b>, once per queued report, and must
    /// only nudge the host's own loop to come and drain. <c>null</c> means
    /// the host polls on its own clock.
    /// </param>
    public SyncWorker(
        string libraryDirectory,
        string deviceId,
        string deviceName,
        HttpTransport? transport = null,
        Action? onWake = null)
    {
        nint transportUser = transport is null ? 0 : TransportBridge.Pin(transport);
        _waker = onWake is null ? default : GCHandle.Alloc(onWake);
        unsafe
        {
            Status status = Interop.cb_sync_open(
                libraryDirectory,
                deviceId,
                deviceName,
                transport is null ? 0 : (nint)TransportBridge.Get,
                transport is null ? 0 : (nint)TransportBridge.Send,
                transport is null ? 0 : (nint)TransportBridge.Finalize,
                transportUser,
                onWake is null ? 0 : (nint)(delegate* unmanaged<nint, void>)&OnWake,
                _waker.IsAllocated ? GCHandle.ToIntPtr(_waker) : 0,
                out _handle);

            if (status != Status.Ok)
            {
                // On failure the engine has already run the finalizer — its
                // ownership rule — so the transport handle is not ours to
                // free here. The waker never reached the engine, so it is.
                if (_waker.IsAllocated)
                {
                    _waker.Free();
                }
                throw ChapbookException.From(status, nameof(SyncWorker));
            }
        }
    }

    private nint Live() =>
        _handle != 0 ? _handle : throw new ObjectDisposedException(nameof(SyncWorker));

    /// <summary>
    /// Ask for every book with a service to reconcile.
    /// </summary>
    /// <remarks>
    /// A shelf where nothing syncs finishes immediately with zero books,
    /// which is a fact to tell the reader rather than a spinner to show
    /// them.
    /// </remarks>
    public void RequestAll() =>
        ChapbookException.Check(Interop.cb_sync_request_all(Live()), nameof(RequestAll));

    /// <summary>Ask for one book.</summary>
    /// <remarks>
    /// A book with no service, or one that has left the shelf, reports
    /// <see cref="SyncReportKind.BookFailed"/> rather than being silently
    /// skipped.
    /// </remarks>
    public void RequestBook(long book) =>
        ChapbookException.Check(Interop.cb_sync_request_book(Live(), book), nameof(RequestBook));

    /// <summary>
    /// The next report, oldest first, or <c>null</c> when there is none —
    /// the ordinary answer between wakes, not an error.
    /// </summary>
    public SyncReport? NextReport()
    {
        Status status = Interop.cb_sync_next(Live(), out NativeSyncReport r);
        if (status == Status.Unavailable)
        {
            return null;
        }
        ChapbookException.Check(status, nameof(NextReport));
        // The strings are borrowed from the handle and die on the next
        // call; marshalling them here is the copy that has to happen.
        return new SyncReport(
            r.Kind,
            r.Book,
            r.Position,
            r.Detail == 0 ? null : Marshal.PtrToStringUTF8(r.Detail),
            (int)r.MarksCreated,
            (int)r.MarksUpdated,
            (int)r.MarksDeleted,
            (int)r.MarksAdopted,
            (int)r.MarksRefreshed,
            (int)r.MarksMerged,
            (int)r.MarksConflicts,
            r.MarksError == 0 ? null : Marshal.PtrToStringUTF8(r.MarksError),
            (int)r.Books);
    }

    /// <summary>Everything reported since the last drain, oldest first.</summary>
    public IReadOnlyList<SyncReport> DrainReports()
    {
        var reports = new List<SyncReport>();
        while (NextReport() is { } report)
        {
            reports.Add(report);
        }
        return reports;
    }

    [UnmanagedCallersOnly]
    private static void OnWake(nint user)
    {
        try
        {
            if (GCHandle.FromIntPtr(user).Target is Action wake)
            {
                wake();
            }
        }
        catch
        {
            // Nothing may unwind into Rust; a host's bug becomes a missed
            // nudge, and the next drain still finds the report.
        }
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            // Joins the worker thread. When this returns nothing is still
            // inside a callback, which is what makes freeing the waker on
            // the next line safe.
            Interop.cb_sync_close(_handle);
            _handle = 0;
        }
        if (_waker.IsAllocated)
        {
            _waker.Free();
        }
    }
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeSyncReport
{
    public SyncReportKind Kind;
    public long Book;
    public PositionOutcome Position;
    public nint Detail;
    public nuint MarksCreated;
    public nuint MarksUpdated;
    public nuint MarksDeleted;
    public nuint MarksAdopted;
    public nuint MarksRefreshed;
    public nuint MarksMerged;
    public nuint MarksConflicts;
    public nint MarksError;
    public nuint Books;
}
