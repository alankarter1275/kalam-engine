using System.Runtime.InteropServices;
using System.Text;

namespace Chapbook;

/// <summary>
/// The loaded library itself: what it is, what it was built with, and
/// where it speaks.
/// </summary>
public static class Engine
{
    /// <summary>
    /// The ABI version the loaded native library implements. A host that
    /// ships its own copy of the DLL should check this against the one it
    /// built against before anything else.
    /// </summary>
    public static uint AbiVersion => Interop.cb_abi_version();

    /// <summary>
    /// What the loaded library was actually built with.
    /// </summary>
    /// <remarks>
    /// This is a runtime question because a header cannot answer it: the
    /// formats are Cargo features, and the DLL on disk may be a narrower
    /// build than the one whose header was read. Asking beats discovering
    /// by absence — a PDF that will not open is otherwise indistinguishable
    /// from a PDF that is broken.
    /// </remarks>
    public static Capabilities Capabilities => (Capabilities)Interop.cb_capabilities();

    /// <summary>
    /// What a key means with no modifiers held, or
    /// <see cref="ReaderAction.None"/> for a key no reader binds.
    /// </summary>
    /// <remarks>
    /// The table is the engine's so a binding is written once rather than
    /// once per host, and it already knows things no desktop shell tends
    /// to: <see cref="Key.TurnPrev"/> and <see cref="Key.TurnNext"/> are a
    /// Kobo's bezel buttons, and on a desktop they are the two thumb
    /// buttons of a mouse.
    /// </remarks>
    public static ReaderAction DefaultAction(Key key) => Interop.cb_key_default_action(key);

    /// <summary>
    /// What a printable character means. Separate from
    /// <see cref="DefaultAction(Key)"/> because a character is what the
    /// keyboard layout produced, not which key was struck.
    /// </summary>
    public static ReaderAction DefaultAction(char c) => Interop.cb_char_default_action(c);

    /// <summary>Whether anything is listening to <see cref="Log"/>.</summary>
    public static bool LogEnabled => Interop.cb_log_enabled() != 0;

    /// <summary>
    /// A record the engine emitted.
    /// </summary>
    /// <param name="Level">How bad it is.</param>
    /// <param name="Target">
    /// The emitting crate — <c>chapbook_reader</c>, <c>chapbook_library</c>
    /// — so a host can filter one subsystem from another.
    /// </param>
    /// <param name="Message">For a person. Free to change; do not match on it.</param>
    public readonly record struct LogRecord(LogLevel Level, string Target, string Message);

    // Held for the process's life because the native side keeps the
    // pointer. A collected delegate is a call into freed memory on the
    // first log line, which is the classic P/Invoke callback defect.
    private static Action<LogRecord>? _sink;
    private static readonly Lock Gate = new();

    /// <summary>
    /// Send the engine's diagnostics somewhere a person will look.
    /// </summary>
    /// <remarks>
    /// <para>
    /// The engine installs no backend and is silent until this is called —
    /// deliberately, because a library that writes to a stream its host did
    /// not choose is a library a host cannot ship. Install it first, before
    /// opening anything, or the reasons an open failed are lost.
    /// </para>
    /// <para>
    /// The callback arrives on whatever thread emitted the record,
    /// including the session's loader thread, so it must be thread-safe.
    /// It must not call back into a session.
    /// </para>
    /// </remarks>
    public static void SetLogCallback(Action<LogRecord>? sink, LogLevel maxLevel = LogLevel.Info)
    {
        lock (Gate)
        {
            _sink = sink;
            unsafe
            {
                nint callback = sink is null
                    ? 0
                    : (nint)(delegate* unmanaged<LogLevel, byte*, byte*, nint, void>)&OnLog;
                ChapbookException.Check(
                    Interop.cb_set_log_callback(callback, 0, sink is null ? LogLevel.Off : maxLevel),
                    nameof(SetLogCallback));
            }
        }
    }

    /// <summary>
    /// Put a host's own line into the same stream, so an app's diagnostics
    /// and the engine's interleave in one place and in order.
    /// </summary>
    public static void Log(LogLevel level, string target, string message) =>
        ChapbookException.Check(Interop.cb_log(level, target, message), nameof(Log));

    [UnmanagedCallersOnly]
    private static unsafe void OnLog(LogLevel level, byte* target, byte* message, nint _)
    {
        // Nothing may unwind back into Rust across this boundary, so the
        // whole body is inside a catch: a host's logging bug becomes a lost
        // line rather than a torn process.
        try
        {
            _sink?.Invoke(new LogRecord(level, Utf8(target), Utf8(message)));
        }
        catch
        {
            // Deliberately swallowed. See above.
        }
    }

    private static unsafe string Utf8(byte* p)
    {
        if (p is null)
        {
            return string.Empty;
        }
        int length = 0;
        while (p[length] != 0)
        {
            length++;
        }
        return Encoding.UTF8.GetString(p, length);
    }
}
