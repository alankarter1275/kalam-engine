package com.ophymx.chapbook

/**
 * The raw JNI surface. Nothing outside this package should call it —
 * [Session] is the type worth holding.
 *
 * This mirrors `crates/chapbook-jni/src/android.rs` by name. It is a spike:
 * when the C ABI exists these become calls into it rather than into
 * `chapbook-reader` directly.
 *
 * Nothing here references the Rust side at compile time and nothing there
 * references this, so a renamed function is an `UnsatisfiedLinkError` on
 * first call rather than a build failure. `android/build-jni.sh` compares
 * these declarations against the `.so`'s exported symbols for that reason.
 */
internal object Native {
    init {
        System.loadLibrary("chapbook_jni")
    }

    external fun initLogging(verbose: Boolean)

    external fun open(path: String, libraryDir: String): Long

    /** Takes ownership of [fd]; the caller must have detached it. */
    external fun openFd(fd: Int, libraryDir: String): Long

    external fun close(handle: Long)
    external fun fontReport(handle: Long): String

    /** Named for Kotlin's sake: `suspend` is a modifier here. */
    external fun suspendSession(handle: Long)

    external fun releaseCaches(handle: Long)
    external fun cacheBytes(handle: Long): Long
    external fun cacheBudget(handle: Long): Long
    external fun setMetrics(handle: Long, width: Float, height: Float, margin: Float, scale: Float)
    external fun nextPage(handle: Long): Boolean
    external fun prevPage(handle: Long): Boolean
    external fun cycleTheme(handle: Long)
    external fun position(handle: Long): Long
    external fun title(handle: Long): String
    external fun renderSize(handle: Long): Long
    external fun renderInto(handle: Long, bitmap: android.graphics.Bitmap): Int
    external fun conformance(path: String, libraryDir: String): String

    // Input. Actions cross as their engine names — "next-page" and so on —
    // rather than as ordinals, because `Action` is non-exhaustive on the
    // Rust side and nothing here would notice it being reordered. The empty
    // string is "no action". See `chapbook_core::input`.

    /** `"ltr"` or `"rtl"`, off the book. */
    external fun readingDirection(handle: Long): String

    /** `middle` is an action name, or `""` for a band that does nothing. */
    external fun setTapZones(handle: Long, prevFraction: Float, nextFraction: Float, middle: String)

    /** Logical units — view pixels over density — in panel space. */
    external fun tapAction(handle: Long, x: Float, y: Float): String

    /** Takes an `android.view.KeyEvent.KEYCODE_*` value. */
    external fun actionForKeyCode(handle: Long, keyCode: Int): String

    /** 0 changed, 1 unchanged, 2 not the engine's, -1 unusable. */
    external fun applyAction(handle: Long, action: String): Int

    // The text surface: the page's text with geometry, for accessibility
    // trees, TTS word highlighting, and dictionary lookup. Ranges pack
    // like `position` — `start shl 32 or end` — and tables flatten into
    // primitive arrays so TTS reads a page in one crossing.

    /** Runs on the current page; -1 until laid out, 0 for a comic. */
    external fun pageTextRunCount(handle: Long): Int

    /** Locator range packed `start shl 32 or end`; -1 on a bad index. */
    external fun pageTextRunRange(handle: Long, index: Int): Long

    /** Page-space `[x, y, w, h]`; empty on a bad index. */
    external fun pageTextRunRect(handle: Long, index: Int): FloatArray

    external fun pageTextRunText(handle: Long, index: Int): String

    /** The page as one speakable string for TTS; `""` until laid out. */
    external fun speakableText(handle: Long): String

    /**
     * Four ints per word: textStart, textEnd (char offsets into
     * [speakableText]), locatorStart, locatorEnd.
     */
    external fun pageWords(handle: Long): IntArray

    /** Word under a panel point, packed like a range; -1 for none. */
    external fun wordAt(handle: Long, x: Float, y: Float): Long

    /** Four floats per rect covering a locator range on this page. */
    external fun rangeRects(handle: Long, start: Int, end: Int): FloatArray
}
