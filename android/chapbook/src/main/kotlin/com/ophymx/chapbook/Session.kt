package com.ophymx.chapbook

import android.graphics.Bitmap
import android.os.ParcelFileDescriptor

/** Where the reader is. The pair, never the page alone — see `docs/SHELLS.md`. */
data class Position(val spine: Int, val page: Int)

/** The device-pixel size a bitmap must be before [Session.renderInto] will draw. */
data class RenderSize(val width: Int, val height: Int)

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
         * Send the engine's diagnostics to logcat under the tag `chapbook`.
         *
         * Call once, before opening anything. The engine is silent until a
         * host installs a backend, and on Android silent used to mean the
         * failures only a device hits were the ones nobody could see.
         */
        fun initLogging(verbose: Boolean = false) = Native.initLogging(verbose)

        /**
         * Opens a book from a filesystem path.
         *
         * `libraryDir` should be `context.filesDir` — it is where positions,
         * annotations and settings live, and it is an argument because a
         * sandboxed app's answer is not reachable any other way.
         *
         * Returns null if the book will not open; logcat says why.
         */
        fun open(path: String, libraryDir: String): Session? {
            val handle = Native.open(path, libraryDir)
            return if (handle == 0L) null else Session(handle)
        }

        /**
         * Opens a book from a `content://` URI's file descriptor — what the
         * storage access framework actually hands an app, with no path and
         * usually no extension. The format comes from the bytes.
         *
         * Takes ownership of [pfd]: it is detached here and closed by the
         * session, so the caller must not close it or use it again.
         *
         * A book opened this way does not reach the library — no path means
         * nothing to fingerprint — so it opens at the beginning every time.
         */
        fun openFd(pfd: ParcelFileDescriptor, libraryDir: String): Session? {
            val handle = Native.openFd(pfd.detachFd(), libraryDir)
            return if (handle == 0L) null else Session(handle)
        }

        /** Run the conformance harness over a book. It opens its own sessions. */
        fun conformance(path: String, libraryDir: String): String =
            Native.conformance(path, libraryDir)
    }

    /**
     * What the font source produced: faces loaded, and any generic family
     * pointed at a name no loaded face carries.
     *
     * Zero faces means every page paginates blank, taking navigation,
     * search and the table of contents with it — still the single most
     * useful string on this screen.
     */
    val fontReport: String get() = Native.fontReport(handle)

    val title: String get() = Native.title(handle)

    val position: Position
        get() {
            val packed = Native.position(handle)
            return Position((packed ushr 32).toInt(), (packed and 0xffffffffL).toInt())
        }

    /** Null until [setMetrics] has been called. */
    val renderSize: RenderSize?
        get() {
            val packed = Native.renderSize(handle)
            if (packed < 0) return null
            return RenderSize((packed ushr 32).toInt(), (packed and 0xffffffffL).toInt())
        }

    val cacheBytes: Long get() = Native.cacheBytes(handle)
    val cacheBudget: Long get() = Native.cacheBudget(handle)

    fun setMetrics(width: Float, height: Float, margin: Float, scale: Float) =
        Native.setMetrics(handle, width, height, margin, scale)

    /** Returns whether the position moved. Do not derive this from [position]. */
    fun nextPage(): Boolean = Native.nextPage(handle)

    /** Returns whether the position moved. */
    fun prevPage(): Boolean = Native.prevPage(handle)

    /** Next theme. Repaints; the position does not move. */
    fun cycleTheme() = Native.cycleTheme(handle)

    /**
     * Save the position and drop everything reconstructible. Call from
     * `onStop`, which is the last callback Android guarantees.
     */
    fun suspend() = Native.suspendSession(handle)

    /** Call from `onTrimMemory`. The current page is rebuilt on the next draw. */
    fun releaseCaches() = Native.releaseCaches(handle)

    /**
     * Draws the current page into [bitmap], which must be `ARGB_8888` and
     * exactly [renderSize]. The engine rasterizes into the bitmap's own
     * pixels; nothing is copied. Returns 0, or a negative code described in
     * `chapbook-jni`.
     */
    fun renderInto(bitmap: Bitmap): Int = Native.renderInto(handle, bitmap)

    override fun close() {
        if (handle != 0L) {
            Native.close(handle)
            handle = 0L
        }
    }
}
