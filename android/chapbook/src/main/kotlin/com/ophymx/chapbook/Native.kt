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
}
