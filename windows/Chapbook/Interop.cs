using System.Runtime.InteropServices;

namespace Chapbook;

/// <summary>
/// The C ABI, transcribed. Nothing above this file names a raw pointer.
/// </summary>
/// <remarks>
/// <para>
/// Every declaration here corresponds to one in
/// <c>crates/chapbook-ffi/include/chapbook.h</c>, which is the contract —
/// a checked-in cbindgen golden, guarded by a test, and the artifact this
/// binding compiles against in the only sense a P/Invoke can. Nothing
/// generates this file, because a generated binding would still have to be
/// read by whoever debugged it, and because the shapes below are small
/// enough to state.
/// </para>
/// <para>
/// Three rules of that ABI decide everything in this file.
/// <b>Codes are the contract and strings are not</b>, so every fallible
/// call returns <see cref="Status"/> and the human-readable half is
/// fetched separately. <b>Nothing crosses owned</b>, so there is no
/// free-string entry point and every string is written into a buffer the
/// caller sized — which is why the string helpers below all run the same
/// ask-then-fill dance. And <b>nothing unwinds out</b>, so a Rust panic
/// arrives as <see cref="Status.Panic"/> rather than as a torn process.
/// </para>
/// <para>
/// <c>StringMarshalling.Utf8</c> on every string-taking call is not
/// decoration: the ABI documents its <c>const char*</c> as UTF-8, and the
/// default for a <c>string</c> parameter is ANSI, which silently mangles
/// any path holding a character outside the active code page. The ABI's
/// one-byte <c>bool</c> crosses as a <c>byte</c> for the same class of
/// reason — see <c>Native.cs</c>.
/// </para>
/// </remarks>
internal static partial class Interop
{
    /// <summary>
    /// The DLL name as it appears in every <c>LibraryImport</c> below.
    /// Windows resolves it as <c>chapbook_ffi.dll</c> beside the assembly
    /// or under <c>runtimes/win-x64/native/</c> in a package.
    /// </summary>
    internal const string Library = "chapbook_ffi";

    // ---- Engine ----

    [LibraryImport(Library)]
    internal static partial uint cb_abi_version();

    [LibraryImport(Library)]
    internal static partial uint cb_capabilities();

    [LibraryImport(Library)]
    internal static partial Status cb_last_error_message(byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_set_log_callback(
        nint callback, nint user, LogLevel maxLevel);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_log(LogLevel level, string target, string message);

    [LibraryImport(Library)]
    internal static partial byte cb_log_enabled();

    // ---- Font sources ----

    [LibraryImport(Library)]
    internal static partial nint cb_font_source_host();

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial nint cb_font_source_embedded(string dir, string family);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_font_source_add_dir(nint fonts, string dir);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_font_source_set_generics(
        nint fonts, string serif, string sansSerif, string monospace, string cursive, string fantasy);

    [LibraryImport(Library)]
    internal static partial Status cb_font_source_use_platform_generics(nint fonts);

    [LibraryImport(Library)]
    internal static partial void cb_font_source_free(nint fonts);

    // ---- Configuration ----

    [LibraryImport(Library)]
    internal static partial nint cb_config_new(nint fonts);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_config_set_library_dir(nint config, string dir);

    [LibraryImport(Library)]
    internal static partial Status cb_config_set_cache_budget(nint config, nuint bytes);

    [LibraryImport(Library)]
    internal static partial void cb_config_free(nint config);

    // ---- Opening and closing ----

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial nint cb_session_open_path(string path, nint config);

    [LibraryImport(Library)]
    internal static partial nint cb_session_open_bytes(
        byte[] bytes, nuint len, BookFormat format, nint config);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial nint cb_session_open_url(string url, nint config);

    [LibraryImport(Library)]
    internal static partial void cb_session_close(nint session);

    // ---- Metrics, navigation, position ----

    [LibraryImport(Library)]
    internal static partial Status cb_session_set_metrics(nint session, NativeMetrics metrics);

    [LibraryImport(Library)]
    internal static partial Status cb_session_next_page(
        nint session, out byte moved);

    [LibraryImport(Library)]
    internal static partial Status cb_session_prev_page(
        nint session, out byte moved);

    [LibraryImport(Library)]
    internal static partial Status cb_session_next_unit(
        nint session, out byte moved);

    [LibraryImport(Library)]
    internal static partial Status cb_session_prev_unit(
        nint session, out byte moved);

    [LibraryImport(Library)]
    internal static partial Status cb_session_position(nint session, out NativePosition position);

    [LibraryImport(Library)]
    internal static partial Status cb_session_spine_len(nint session, out nuint len);

    [LibraryImport(Library)]
    internal static partial Status cb_session_page_count(nint session, out nuint count);

    [LibraryImport(Library)]
    internal static partial Status cb_session_title(
        nint session, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_session_book_kind(nint session, out BookKind kind);

    [LibraryImport(Library)]
    internal static partial Status cb_session_reading_direction(
        nint session, out ReadingDirection direction);

    // ---- Input ----

    [LibraryImport(Library)]
    internal static partial Status cb_session_set_tap_zones(
        nint session, float prevFraction, float nextFraction, ReaderAction middle);

    [LibraryImport(Library)]
    internal static partial Status cb_session_tap_action(
        nint session, float x, float y, out ReaderAction action);

    [LibraryImport(Library)]
    internal static partial ReaderAction cb_key_default_action(Key key);

    [LibraryImport(Library)]
    internal static partial ReaderAction cb_char_default_action(uint codepoint);

    [LibraryImport(Library)]
    internal static partial Status cb_session_apply(
        nint session, ReaderAction action, out ActionOutcome outcome);

    // ---- Settings and fonts ----

    [LibraryImport(Library)]
    internal static partial Status cb_session_settings(nint session, out NativeSettings settings);

    [LibraryImport(Library)]
    internal static partial Status cb_session_set_settings(
        nint session, NativeSettings settings, SettingsScope scope);

    [LibraryImport(Library)]
    internal static partial Status cb_session_font_family_count(nint session, out nuint count);

    [LibraryImport(Library)]
    internal static partial Status cb_session_font_family_at(
        nint session, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_session_font_family(
        nint session, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_session_set_font_family(
        nint session, string? family, SettingsScope scope);

    [LibraryImport(Library)]
    internal static partial Status cb_session_font_face_count(nint session, out nuint count);

    [LibraryImport(Library)]
    internal static partial Status cb_session_font_unresolved(
        nint session, byte[]? buf, nuint cap, out nuint needed);

    // ---- Rendering ----

    [LibraryImport(Library)]
    internal static partial Status cb_session_render_size(
        nint session, out uint width, out uint height);

    [LibraryImport(Library)]
    internal static partial Status cb_session_render_into(
        nint session, nint pixels, nuint len, uint width, uint height, nuint stride);

    // ---- The text surface ----

    [LibraryImport(Library)]
    internal static partial Status cb_session_page_text_run_count(nint session, out nuint count);

    [LibraryImport(Library)]
    internal static partial Status cb_session_page_text_run(
        nint session, nuint index, out NativeTextRun run);

    [LibraryImport(Library)]
    internal static partial Status cb_session_page_text_run_text(
        nint session, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_session_range_rects(
        nint session, uint start, uint end, NativeRect[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_session_page_speakable_text(
        nint session, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_session_page_word_count(nint session, out nuint count);

    [LibraryImport(Library)]
    internal static partial Status cb_session_page_word(
        nint session, nuint index, out NativeWordSpan span);

    [LibraryImport(Library)]
    internal static partial Status cb_session_word_at(
        nint session, float x, float y, out uint start, out uint end);

    // ---- Lifecycle ----

    [LibraryImport(Library)]
    internal static partial Status cb_session_suspend(nint session);

    [LibraryImport(Library)]
    internal static partial Status cb_session_release_caches(nint session);

    [LibraryImport(Library)]
    internal static partial Status cb_session_cache_bytes(nint session, out nuint bytes);

    [LibraryImport(Library)]
    internal static partial Status cb_session_cache_budget(nint session, out nuint bytes);

    [LibraryImport(Library)]
    internal static partial Status cb_session_set_waker(nint session, nint wake, nint user);

    [LibraryImport(Library)]
    internal static partial Status cb_session_poll_loaded(
        nint session, out byte changed);

    [LibraryImport(Library)]
    internal static partial Status cb_session_next_event(
        nint session, out NativeSessionEvent evt);

    [LibraryImport(Library)]
    internal static partial Status cb_session_has_pending_loads(
        nint session, out byte pending);

    // ---- The library ----

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_library_open(string dir, out nint library);

    [LibraryImport(Library)]
    internal static partial void cb_library_close(nint library);

    [LibraryImport(Library)]
    internal static partial Status cb_library_default_dir(
        byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_library_query(
        nint library, NativeBookQuery query, out nint shelf);

    [LibraryImport(Library)]
    internal static partial void cb_shelf_free(nint shelf);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_len(nint shelf, out nuint len);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_book(nint shelf, nuint index, out NativeBook book);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_title(
        nint shelf, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_author(
        nint shelf, nuint index, nuint author, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_series(
        nint shelf, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_language(
        nint shelf, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_identifier(
        nint shelf, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_fingerprint(
        nint shelf, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_file_path(
        nint shelf, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_cover_path(
        nint shelf, nuint index, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_collection_id(
        nint shelf, nuint index, nuint slot, out long id);

    [LibraryImport(Library)]
    internal static partial Status cb_shelf_collection_name(
        nint shelf, nuint index, nuint slot, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_library_delete_book(nint library, long book);

    [LibraryImport(Library)]
    internal static partial Status cb_library_set_finished(
        nint library, long book, byte finished);

    [LibraryImport(Library)]
    internal static partial Status cb_session_book_id(nint session, out long book);

    [LibraryImport(Library)]
    internal static partial Status cb_library_collections(
        nint library, NativeCollection[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library)]
    internal static partial Status cb_library_collection_name(
        nint library, long collection, byte[]? buf, nuint cap, out nuint needed);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_library_create_collection(
        nint library, string name, out long id);

    [LibraryImport(Library, StringMarshalling = StringMarshalling.Utf8)]
    internal static partial Status cb_library_rename_collection(
        nint library, long collection, string name);

    [LibraryImport(Library)]
    internal static partial Status cb_library_delete_collection(nint library, long collection);

    [LibraryImport(Library)]
    internal static partial Status cb_library_add_to_collection(
        nint library, long book, long collection);

    [LibraryImport(Library)]
    internal static partial Status cb_library_remove_from_collection(
        nint library, long book, long collection);
}
