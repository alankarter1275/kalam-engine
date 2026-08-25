package com.ophymx.chapbook

/**
 * The raw JNI surface. Nothing outside this package should call it —
 * [Session] is the type worth holding.
 *
 * This mirrors `crates/chapbook-jni/src/android.rs` by name. It is a spike:
 * when the C ABI exists these become calls into it rather than into
 * `chapbook-reader` directly.
 */
internal object Native {
    init {
        System.loadLibrary("chapbook_jni")
    }

    external fun open(path: String, libraryDir: String): Long
    external fun close(handle: Long)
    external fun faceCount(handle: Long): Int
    external fun setMetrics(handle: Long, width: Float, height: Float, margin: Float, scale: Float)
    external fun nextPage(handle: Long): Boolean
    external fun prevPage(handle: Long): Boolean
    external fun position(handle: Long): Long
    external fun title(handle: Long): String
    external fun renderInto(handle: Long, bitmap: android.graphics.Bitmap): Int
    external fun conformance(path: String): String
}
