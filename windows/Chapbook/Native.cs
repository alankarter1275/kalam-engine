using System.Runtime.InteropServices;

namespace Chapbook;

// The plain-data types the ABI passes by value, and the enumerations it
// passes as codes. Layout here is a promise about `chapbook.h`, so every
// struct is `[StructLayout(LayoutKind.Sequential)]` and every field is in
// the header's order — a reordering compiles cleanly and misreads every
// value, which is the one bug this file can have.
//
// `size_t` is `nuint`, and the ABI's one-byte `bool` is a `byte` here
// rather than a `bool`. That is not pedantry: .NET's default marshalling
// for `bool` in a struct is a four-byte Win32 `BOOL`, which would misread
// every field after it — and with runtime marshalling disabled for this
// assembly (see `AssemblyInfo.cs`) a non-blittable field is a compile
// error rather than a silent one, which is the trade this file wants.

[StructLayout(LayoutKind.Sequential)]
internal struct NativeMetrics
{
    public float Width;
    public float Height;
    public float MarginTop;
    public float MarginRight;
    public float MarginBottom;
    public float MarginLeft;
    public float DpiScale;
    public Rotation Rotation;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativePosition
{
    public uint Spine;
    public uint Page;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeSettings
{
    public float BaseFontPx;
    public float LineHeight;
    public byte Justify;
    public byte PublisherStyles;
    public Theme Theme;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeRect
{
    public float X;
    public float Y;
    public float W;
    public float H;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeTextRun
{
    public NativeRect Rect;
    public uint LocatorStart;
    public uint LocatorEnd;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeWordSpan
{
    public uint TextStart;
    public uint TextEnd;
    public uint LocatorStart;
    public uint LocatorEnd;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeBookQuery
{
    public nint Search;
    public nint Series;
    public long Collection;
    public ReadingState State;
    public ShelfSort Sort;
    public nuint Limit;
    public nuint Offset;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeBook
{
    public long Id;
    public long AddedAt;
    public long LastRead;
    public long FinishedAt;
    public double Progress;
    public double SeriesIndex;
    public ReadingState State;
    public nuint AuthorCount;
    public nuint CollectionCount;
    public byte HasProgress;
    public byte HasSeriesIndex;
    public byte HasCover;
}

[StructLayout(LayoutKind.Sequential)]
internal struct NativeCollection
{
    public long Id;
    public long AddedAt;
    public nuint Books;
}

/// <summary>
/// The ABI's result code. Zero is success, negative is failure, and the
/// numbers are permanent — the human-readable half is
/// <see cref="ChapbookException.Message"/> and is explicitly free to
/// change, so match on this and never on that.
/// </summary>
public enum Status
{
    Ok = 0,
    Panic = -1,
    NullArgument = -2,
    BufferTooSmall = -4,
    InvalidArgument = -5,
    Unavailable = -6,
    BookOpen = -10,
    BookMalformed = -11,
    ResourceNotFound = -12,
    FixedLayoutUnsupported = -13,
    FormatNotBuilt = -14,
    SpineOutOfRange = -15,
    Parse = -16,
    Style = -17,
    Layout = -18,
    Font = -19,
    Cfi = -20,
    Network = -21,
    Opds = -22,
    LibraryError = -23,
    Credential = -24,
    Panel = -25,
    Io = -26,
}

/// <summary>Which format to read bytes as. <see cref="Guess"/> sniffs.</summary>
public enum BookFormat : uint
{
    Guess = 0,
    Epub = 1,
    Cbz = 2,
    Pdf = 3,
}

/// <summary>What kind of thing the open publication is.</summary>
public enum BookKind : uint
{
    Epub = 0,
    Comic = 1,
    Pdf = 2,
}

/// <summary>
/// Which physical edge reading starts from. The book declares it — EPUB's
/// <c>page-progression-direction</c> — and a host needs it so its own
/// gestures agree with the tap zones.
/// </summary>
public enum ReadingDirection : uint
{
    LeftToRight = 0,
    RightToLeft = 1,
}

/// <summary>The page's colour scheme.</summary>
public enum Theme : uint
{
    Light = 0,
    Sepia = 1,
    Dark = 2,
}

/// <summary>
/// A quarter turn applied on the way to the panel. Not a layout input:
/// turning the panel does not repaginate the book.
/// </summary>
public enum Rotation : uint
{
    None = 0,
    Quarter = 1,
    Half = 2,
    ThreeQuarter = 3,
}

/// <summary>Whether a settings change is this book's or the default.</summary>
public enum SettingsScope : uint
{
    Global = 0,
    ThisBook = 1,
}

/// <summary>
/// A reader intent. A host produces one — from a tap zone, a key lookup,
/// or its own UI — and applies it; it never has to interpret one.
/// </summary>
/// <remarks>
/// The set is open and values are only appended, so a host may persist
/// them. <see cref="None"/> is not an action: it is the "nothing" answer,
/// and applying it is an error.
/// </remarks>
public enum ReaderAction : uint
{
    None = 0,
    NextPage = 1,
    PrevPage = 2,
    NextUnit = 3,
    PrevUnit = 4,
    Back = 5,
    FontUp = 6,
    FontDown = 7,
    CycleTheme = 8,
    ToggleMenu = 9,
}

/// <summary>
/// A key in the engine's vocabulary, not a platform keycode. A host
/// translates its own codes into this; the opinions live in
/// <see cref="Engine.DefaultAction(Key)"/>.
/// </summary>
public enum Key : uint
{
    ArrowLeft = 1,
    ArrowRight = 2,
    ArrowUp = 3,
    ArrowDown = 4,
    PageUp = 5,
    PageDown = 6,
    Space = 7,
    Backspace = 8,
    TurnPrev = 9,
    TurnNext = 10,
    VolumeUp = 11,
    VolumeDown = 12,
}

/// <summary>
/// What the engine did with an action — two answers, because a host needs
/// both and can derive neither from the other.
/// </summary>
public enum ActionOutcome : uint
{
    /// <summary>Applied and something moved: consume the event and repaint.</summary>
    Changed = 0,

    /// <summary>
    /// Applied and nothing moved — the last page, or the font at its stop.
    /// Consume the event anyway; the reader does take this key.
    /// </summary>
    Unchanged = 1,

    /// <summary>
    /// Not the engine's. Let the event through to the platform — which is
    /// also what <see cref="ReaderAction.Back"/> answers at the bottom of
    /// the back trail, so a host forwards the gesture unconditionally
    /// rather than shadowing the history to know when to stop.
    /// </summary>
    Unhandled = 2,
}

/// <summary>How much of a book has been read.</summary>
public enum ReadingState : uint
{
    Any = 0,
    Unread = 1,
    Reading = 2,
    Finished = 3,
}

/// <summary>The order a shelf comes back in.</summary>
public enum ShelfSort : uint
{
    Added = 0,
    Read = 1,
    Title = 2,
    Author = 3,
    Series = 4,
}

/// <summary>How much the engine says, and about what.</summary>
public enum LogLevel
{
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

/// <summary>
/// What the loaded library was built with. A header cannot say which
/// artifact a host actually loaded, which is why this is a runtime
/// question.
/// </summary>
[Flags]
public enum Capabilities : uint
{
    None = 0,
    Library = 1,
    Cbz = 2,
    Pdf = 4,
    Opds = 8,
    BundledHttp = 16,
    Svg = 32,
    MathMl = 64,
}
