namespace Chapbook;

/// <summary>
/// Where a session's faces come from, what its five CSS generics mean, and
/// what it tries when a glyph is missing.
/// </summary>
/// <remarks>
/// <para>
/// A font source is required rather than defaulted, because "no fonts"
/// fails silently: a session with an empty database lays out, renders and
/// paginates every book to one blank page. There is nothing to navigate
/// to, nothing for search to find, and no error anywhere.
/// </para>
/// <para>
/// On Windows <see cref="Host"/> is the right answer — fontdb reads the
/// system faces and the five generics resolve to families Windows
/// actually ships. <see cref="Embedded"/> is for a build that means to
/// carry its own typography and not depend on the machine's.
/// </para>
/// <para>
/// The handle is consumed by <see cref="SessionConfiguration"/>, which is
/// consumed by opening a session — on failure as much as on success. This
/// type owns it only until then.
/// </para>
/// </remarks>
public sealed class FontSource : IDisposable
{
    internal nint Handle;

    private FontSource(nint handle)
    {
        if (handle == 0)
        {
            throw ChapbookException.From(Status.Unavailable, nameof(FontSource));
        }
        Handle = handle;
    }

    /// <summary>The machine's own fonts, scanned by fontdb.</summary>
    public static FontSource Host() => new(Interop.cb_font_source_host());

    /// <summary>
    /// Faces from a directory, scanned recursively, with one family named
    /// as the default. What a build that ships its own typography wants.
    /// </summary>
    public static FontSource Embedded(string directory, string family) =>
        new(Interop.cb_font_source_embedded(directory, family));

    /// <summary>Add another directory of faces.</summary>
    public FontSource AddDirectory(string directory)
    {
        ChapbookException.Check(
            Interop.cb_font_source_add_dir(Live(), directory), nameof(AddDirectory));
        return this;
    }

    /// <summary>
    /// Name all five CSS generics. All five, because a build can get one
    /// right and the others wrong, and an unnamed generic resolves to
    /// missing glyphs rather than to an error.
    /// </summary>
    public FontSource SetGenerics(
        string serif, string sansSerif, string monospace, string cursive, string fantasy)
    {
        ChapbookException.Check(
            Interop.cb_font_source_set_generics(
                Live(), serif, sansSerif, monospace, cursive, fantasy),
            nameof(SetGenerics));
        return this;
    }

    /// <summary>
    /// Take the engine's own table for the target it was built for. On
    /// Windows that means the same as the host's answer, because fontdb
    /// already gets it right here.
    /// </summary>
    public FontSource UsePlatformGenerics()
    {
        ChapbookException.Check(
            Interop.cb_font_source_use_platform_generics(Live()), nameof(UsePlatformGenerics));
        return this;
    }

    private nint Live() =>
        Handle != 0 ? Handle : throw new ObjectDisposedException(nameof(FontSource));

    internal nint Consume()
    {
        nint handle = Live();
        Handle = 0;
        return handle;
    }

    public void Dispose()
    {
        if (Handle != 0)
        {
            Interop.cb_font_source_free(Handle);
            Handle = 0;
        }
    }
}

/// <summary>
/// Everything a session needs that is not the book: fonts, where the
/// library lives, and how much it may cache.
/// </summary>
/// <remarks>
/// Every capability arrives here rather than being reached for inside the
/// engine. That is the point of the seam: a host that must substitute one
/// — its own font set, a library in a packaged app's data folder — changes
/// this and nothing else.
/// </remarks>
public sealed partial class SessionConfiguration : IDisposable
{
    internal nint Handle;

    /// <summary>
    /// Build a configuration, taking ownership of the font source.
    /// </summary>
    public SessionConfiguration(FontSource fonts)
    {
        ArgumentNullException.ThrowIfNull(fonts);
        Handle = Interop.cb_config_new(fonts.Consume());
        if (Handle == 0)
        {
            throw ChapbookException.From(Status.Unavailable, nameof(SessionConfiguration));
        }
    }

    /// <summary>
    /// Where the shelf, positions and annotations live.
    /// </summary>
    /// <remarks>
    /// Leaving it unset asks the platform's own convention, which on
    /// Windows is <c>%APPDATA%\chapbook</c> (else <c>%LOCALAPPDATA%</c>).
    /// A packaged app with a redirected AppData gets its own container for
    /// free; a host that wants a different one — a portable install, a
    /// test — says so here.
    /// </remarks>
    public SessionConfiguration WithLibraryDirectory(string directory)
    {
        ChapbookException.Check(
            Interop.cb_config_set_library_dir(Live(), directory), nameof(WithLibraryDirectory));
        return this;
    }

    /// <summary>
    /// Cap the laid-out chapters and decoded page images held at once.
    /// </summary>
    /// <remarks>
    /// The default is a desktop's. The only cost of a small budget is a
    /// slower page-back: eviction never drops the unit on screen, and
    /// everything else is re-read on demand.
    /// </remarks>
    public SessionConfiguration WithCacheBudget(long bytes)
    {
        ChapbookException.Check(
            Interop.cb_config_set_cache_budget(Live(), (nuint)bytes), nameof(WithCacheBudget));
        return this;
    }

    internal nint Live() =>
        Handle != 0 ? Handle : throw new ObjectDisposedException(nameof(SessionConfiguration));

    internal nint Consume()
    {
        nint handle = Live();
        Handle = 0;
        return handle;
    }

    public void Dispose()
    {
        if (Handle != 0)
        {
            Interop.cb_config_free(Handle);
            Handle = 0;
        }
    }
}
