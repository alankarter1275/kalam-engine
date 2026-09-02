package com.ophymx.chapbook

import android.graphics.Bitmap
import android.os.ParcelFileDescriptor

/** Where the reader is. The pair, never the page alone — see `docs/SHELLS.md`. */
data class Position(val spine: Int, val page: Int)

/** The device-pixel size a bitmap must be before [Session.renderInto] will draw. */
data class RenderSize(val width: Int, val height: Int)

/**
 * One visual line of the current page, with its geometry and locator
 * range — the material an `AccessibilityNodeInfo` tree is built from.
 * The rect is page space (logical units, page top-left); the locator
 * range is `[start, end)` in the same offsets positions and annotations
 * use. The text's length is not the locator span's width.
 */
data class TextRun(
    val text: String,
    val rect: android.graphics.RectF,
    val locatorStart: Int,
    val locatorEnd: Int,
)

/**
 * One word: where it sits in [Session.speakableText] (char offsets) and
 * in locator space. `onRangeStart` from a TTS utterance reports offsets
 * into the string; the locator range is how that progress becomes a
 * highlight via [Session.rangeRects].
 */
data class WordSpan(
    val textStart: Int,
    val textEnd: Int,
    val locatorStart: Int,
    val locatorEnd: Int,
)

/**
 * What the engine did with an action, and what you owe the platform back.
 *
 * Two questions, not one, and neither implies the other. [needsRedraw] says
 * whether to `invalidate()`. [consumed] says what to return from
 * `onKeyDown`/`onKeyUp` — and getting *that* wrong is expensive here,
 * because the default key map binds the volume keys to page turns: a
 * reader that answers "nothing changed" on the last page hands the press
 * back and Android draws its volume slider over the book.
 */
enum class ActionOutcome {
    /** Applied, and something moved. Repaint, and consume the event. */
    Changed,

    /** Applied, nothing moved — last page, font at its stop. Still yours. */
    Unchanged,

    /**
     * Not the engine's: `toggle-menu` always, and `back` with an empty
     * trail. Let the event through, which is how the system Back leaves
     * the reader without this side tracking the history to know when.
     */
    Unhandled;

    val needsRedraw: Boolean get() = this == Changed
    val consumed: Boolean get() = this != Unhandled
}

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
         * A book opened this way reaches the library by content — hashed on
         * open, recorded under the same edition fingerprint a path import
         * gets — so its position and annotations persist. Reopening the
         * *file* next launch is the app's job: take a persistable URI
         * grant, re-resolve it, and hand the descriptor back here.
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

    /**
     * Which edge this book reads from: `"ltr"` or `"rtl"`.
     *
     * The book declares it — EPUB's `page-progression-direction` — and the
     * tap zones already use it. This is here so a shell can show that it
     * did, because a correctly flipped RTL book and a bug look the same
     * from the outside.
     */
    val readingDirection: String get() = Native.readingDirection(handle)

    /**
     * Reconfigure the tap bands as fractions of the page width. [middle] is
     * an action name, or `""` for a band that does nothing.
     *
     * There is no direction parameter on purpose: that one is the book's.
     */
    fun setTapZones(prevFraction: Float, nextFraction: Float, middle: String) =
        Native.setTapZones(handle, prevFraction, nextFraction, middle)

    /**
     * What a tap means, or null.
     *
     * [x] and [y] are **logical units** — view pixels divided by the
     * display density, the same space [setMetrics] is given — in panel
     * coordinates. A rotated panel is undone on the engine's side.
     */
    fun tapAction(x: Float, y: Float): String? =
        Native.tapAction(handle, x, y).ifEmpty { null }

    /** What a key means, or null. Takes an `KeyEvent.KEYCODE_*` value. */
    fun actionForKeyCode(keyCode: Int): String? =
        Native.actionForKeyCode(handle, keyCode).ifEmpty { null }

    /**
     * Apply what a tap or a key meant. You do not have to know which
     * action it was — hand back what you were given and read the outcome.
     */
    fun apply(action: String): ActionOutcome = when (Native.applyAction(handle, action)) {
        0 -> ActionOutcome.Changed
        1 -> ActionOutcome.Unchanged
        else -> ActionOutcome.Unhandled
    }

    // The text surface: what an accessibility tree, TTS, or a dictionary
    // popup consumes. Re-fetch after anything that redraws — a turn, a
    // reflow, a settings change.

    /**
     * The current page's text runs in reading order, or null until the
     * page is laid out. Empty for a page with nothing to speak (a comic).
     */
    fun pageTextRuns(): List<TextRun>? {
        val count = Native.pageTextRunCount(handle)
        if (count < 0) return null
        return (0 until count).mapNotNull { index ->
            val packed = Native.pageTextRunRange(handle, index)
            val rect = Native.pageTextRunRect(handle, index)
            if (packed < 0 || rect.size != 4) return@mapNotNull null
            TextRun(
                text = Native.pageTextRunText(handle, index),
                rect = android.graphics.RectF(
                    rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]),
                locatorStart = (packed ushr 32).toInt(),
                locatorEnd = (packed and 0xffffffffL).toInt(),
            )
        }
    }

    /** The page as one string for a TTS utterance; `""` until laid out. */
    val speakableText: String get() = Native.speakableText(handle)

    /** The word table mapping TTS progress back to locator space. */
    fun pageWords(): List<WordSpan> {
        val flat = Native.pageWords(handle)
        return (flat.indices step 4).map { i ->
            WordSpan(flat[i], flat[i + 1], flat[i + 2], flat[i + 3])
        }
    }

    /**
     * The word under a panel point as a locator range, or null — off
     * text, on whitespace, on bare punctuation. Dictionary lookup's
     * question; feed the range to [rangeRects] or a selection.
     */
    fun wordAt(x: Float, y: Float): Pair<Int, Int>? {
        val packed = Native.wordAt(handle, x, y)
        if (packed < 0) return null
        return (packed ushr 32).toInt() to (packed and 0xffffffffL).toInt()
    }

    /** Page-space rects covering a locator range on the current page. */
    fun rangeRects(start: Int, end: Int): List<android.graphics.RectF> {
        val flat = Native.rangeRects(handle, start, end)
        return (flat.indices step 4).map { i ->
            android.graphics.RectF(
                flat[i], flat[i + 1], flat[i] + flat[i + 2], flat[i + 1] + flat[i + 3])
        }
    }

    /**
     * The library row this session's book was imported into, or null for
     * a book that never reached one — an OPDS stream, or a session opened
     * without a library directory.
     *
     * The join between the reading view and the shelf: opening a book is
     * what adds it, so this is how an app learns which [Book] it just
     * created and can put it in a collection, mark it, or find it again.
     */
    fun bookId(): Long? = Native.sessionBookId(handle).takeIf { it != 0L }

    override fun close() {
        if (handle != 0L) {
            Native.close(handle)
            handle = 0L
        }
    }
}
