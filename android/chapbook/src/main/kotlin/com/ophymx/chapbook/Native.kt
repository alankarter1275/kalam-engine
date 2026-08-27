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
}
