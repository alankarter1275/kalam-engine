package com.ophymx.chapbook

import android.graphics.Bitmap

/** Where the reader is. The pair, never the page alone — see `docs/SHELLS.md`. */
data class Position(val spine: Int, val page: Int)

/**
 * One open book.
 *
 * Held by the app for as long as it is reading, and [close]d exactly once.
 * The underlying session is `Send` but not `Sync`: it may move between
 * threads, and must never be touched from two at the same time. This class
 * does not enforce that — the C ABI's header will say it, and a real
 * binding should.
 */
class Session private constructor(private var handle: Long) : AutoCloseable {

    companion object {
        /**
         * Opens a book from a filesystem path.
         *
         * `libraryDir` should be `context.filesDir`; it becomes
         * `CHAPBOOK_LIBRARY_DIR`, which is how positions and annotations
         * find somewhere to live without an API for it yet.
         *
         * Returns null if the book will not open.
         */
        fun open(path: String, libraryDir: String): Session? {
            val handle = Native.open(path, libraryDir)
            return if (handle == 0L) null else Session(handle)
        }

        /** Run the conformance harness over a book. It opens its own sessions. */
        fun conformance(path: String): String = Native.conformance(path)
    }

    /**
     * How many font faces the session found. Zero means every page will be
     * blank — the single most useful number on this whole screen, because
     * fontdb does not look at `/system/fonts` and the session has no API to
     * be told about them.
     */
    val faceCount: Int get() = Native.faceCount(handle)

    val title: String get() = Native.title(handle)

    val position: Position
        get() {
            val packed = Native.position(handle)
            return Position((packed ushr 32).toInt(), (packed and 0xffffffffL).toInt())
        }

    fun setMetrics(width: Float, height: Float, margin: Float, scale: Float) =
        Native.setMetrics(handle, width, height, margin, scale)

    /** Returns whether the position moved. Do not derive this from [position]. */
    fun nextPage(): Boolean = Native.nextPage(handle)

    /** Returns whether the position moved. */
    fun prevPage(): Boolean = Native.prevPage(handle)

    /**
     * Draws the current page into [bitmap], which must be `ARGB_8888` and
     * exactly the size the metrics asked for. Returns 0, or a negative code
     * described in `chapbook-jni`.
     */
    fun renderInto(bitmap: Bitmap): Int = Native.renderInto(handle, bitmap)

    override fun close() {
        if (handle != 0L) {
            Native.close(handle)
            handle = 0L
        }
    }
}
