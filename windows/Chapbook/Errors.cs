using System.Text;

namespace Chapbook;

/// <summary>
/// Something the engine refused, carrying the ABI's permanent code and its
/// impermanent message.
/// </summary>
/// <remarks>
/// Branch on <see cref="Status"/>; log <see cref="Exception.Message"/>.
/// That split is the ABI's, not this binding's: the numbers are the
/// contract and the strings are explicitly free to change between
/// versions, so a host that matched on text would break on an upgrade
/// that broke nothing.
/// </remarks>
public sealed class ChapbookException : Exception
{
    /// <summary>The permanent code. Never <see cref="Chapbook.Status.Ok"/>.</summary>
    public Status Status { get; }

    internal ChapbookException(Status status, string message) : base(message) => Status = status;

    /// <summary>
    /// Build from a failed call, reading the message the engine left for
    /// this thread.
    /// </summary>
    /// <remarks>
    /// The message is per-thread and is overwritten by the next failing
    /// call, so it is read here — immediately after the failure, on the
    /// thread that caused it — and never later.
    /// </remarks>
    internal static ChapbookException From(Status status, string call)
    {
        string? detail = LastErrorMessage();
        return new ChapbookException(
            status,
            detail is null ? $"{call} failed ({status})" : $"{call}: {detail}");
    }

    /// <summary>Throw unless the call succeeded.</summary>
    internal static void Check(Status status, string call)
    {
        if (status != Status.Ok)
        {
            throw From(status, call);
        }
    }

    private static string? LastErrorMessage()
    {
        Status status = Interop.cb_last_error_message(null, 0, out nuint needed);
        if (status != Status.BufferTooSmall || needed <= 1)
        {
            return null;
        }
        byte[] buffer = new byte[needed];
        return Interop.cb_last_error_message(buffer, needed, out _) == Status.Ok
            ? Encoding.UTF8.GetString(buffer, 0, (int)needed - 1)
            : null;
    }
}

/// <summary>
/// The ABI's one string shape, run once here rather than at each of the
/// dozen call sites that need it.
/// </summary>
/// <remarks>
/// Ask with a zero-capacity buffer, allocate what the engine asked for,
/// ask again. <c>needed</c> counts the NUL the engine writes, so the
/// string is one byte shorter than the buffer. Nothing crosses owned in
/// either direction, which is the reason this ABI has no free-string call
/// and cannot be made to free a host's allocation with the wrong
/// allocator.
/// </remarks>
internal delegate Status StringOut(byte[]? buffer, nuint capacity, out nuint needed);

internal static class Strings
{
    /// <summary>The value, or <c>null</c> when the engine reports it absent.</summary>
    internal static string? Read(StringOut call, string name)
    {
        Status sizing = call(null, 0, out nuint needed);
        if (sizing == Status.Unavailable)
        {
            // Absent, not broken: no title, no series, no unresolved
            // generic to report.
            return null;
        }
        if (sizing != Status.BufferTooSmall)
        {
            ChapbookException.Check(sizing, name);
        }
        if (needed <= 1)
        {
            // Just the NUL: an empty string, which is a value and not an
            // absence.
            return string.Empty;
        }
        byte[] buffer = new byte[needed];
        ChapbookException.Check(call(buffer, needed, out _), name);
        return Encoding.UTF8.GetString(buffer, 0, (int)needed - 1);
    }
}
