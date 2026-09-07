package com.ophymx.chapbook.demo

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.view.MotionEvent
import android.view.View
import android.view.accessibility.AccessibilityNodeProvider
import com.ophymx.chapbook.PageAccessibility
import com.ophymx.chapbook.Session
import com.ophymx.chapbook.SessionEvent

/**
 * The whole shell, in one view.
 *
 * Five steps, the same five `docs/SHELLS.md` describes: hand the session
 * page metrics, ask it to draw, put the pixels on screen, feed it input,
 * and use what the navigation calls return.
 */
class ReaderView(context: Context, private val session: Session) : View(context) {

    private var bitmap: Bitmap? = null
    private var lastRender: Int = 0

    /** Called after every turn so the activity can update its status line. */
    var onMoved: (() -> Unit)? = null

    /** Called on a long press — the demo runs conformance from here. */
    var onLongPress: (() -> Unit)? = null

    private var downAt: Long = 0

    /** The last thing the engine was asked to do, for the status line. */
    var lastAction: String? = null
        private set

    // The page's text runs as TalkBack's virtual tree; see the class docs
    // for the three delegations this view owes it.
    private val a11y = PageAccessibility(this, session)

    init {
        // Thirds, with the middle cycling the theme instead of opening a
        // menu this demo does not have. Sepia is the only colour on screen
        // whose red and blue channels differ, so it is the one thing that
        // can tell premultiplied RGBA from BGRA — worth keeping a band for.
        session.setTapZones(1f / 3f, 1f / 3f, "cycle-theme")
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_YES
        // Image books decode off the UI thread, and the waker is how
        // their pages reach the screen: it fires on the loader thread, so
        // it only posts; the post polls, and a visible change repaints. A
        // failed page surfaces on the status line instead of staying a
        // placeholder with no explanation.
        session.setWaker(
            Runnable {
                post {
                    if (session.pollLoaded()) invalidate()
                    for (event in session.drainEvents()) {
                        if (event is SessionEvent.UnitFailed) {
                            lastAction = "page ${event.spine + 1} failed: ${event.message}"
                            onMoved?.invoke()
                        }
                    }
                }
            }
        )
    }

    override fun getAccessibilityNodeProvider(): AccessibilityNodeProvider = a11y

    override fun dispatchHoverEvent(event: MotionEvent): Boolean =
        a11y.dispatchHoverEvent(event) || super.dispatchHoverEvent(event)

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        if (w <= 0 || h <= 0) return
        val density = resources.displayMetrics.density
        // Metrics are in logical units and the scale carries the DPI, so the
        // engine lays out at a readable size and rasterizes at the panel's.
        session.setMetrics(w / density, h / density, 24f, density)
        bitmap?.recycle()
        // Ask the session how big the surface has to be rather than assuming
        // it is the view's size. Logical units are the view's pixels divided
        // by the density and the device size is that multiplied back, and
        // the round trip through a float truncation does not always land on
        // the pixel it started from. `render_into` refuses a buffer that is
        // the wrong size — correctly — so a view that guesses draws nothing
        // and cannot say why.
        val size = session.renderSize ?: return
        bitmap = Bitmap.createBitmap(size.width, size.height, Bitmap.Config.ARGB_8888)
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val target = bitmap ?: return
        // Straight into the bitmap's own pixels: `AndroidBitmap_lockPixels`
        // hands back its backing store and the engine rasterizes there.
        // tiny-skia's output is premultiplied RGBA8888 and that is what
        // ARGB_8888 holds, so nothing converts and nothing is copied.
        lastRender = session.renderInto(target)
        if (lastRender == 0) canvas.drawBitmap(target, 0f, 0f, null)
        // Every content change funnels through a draw — including the very
        // first page, which no input handler ever sees — so this is the one
        // place accessibility needs telling. From a post, not mid-draw, and
        // pageChanged itself no-ops unless the page's text actually moved.
        post { a11y.pageChanged() }
    }

    /**
     * Input goes through the engine, which is the whole point of the seam:
     * this method decides that a press which did not become a long press is
     * a tap, and nothing else. Which band the tap fell in, which edge the
     * book reads from, and what either means are `chapbook_core::input`'s.
     *
     * A long press runs the conformance harness.
     */
    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.action) {
            MotionEvent.ACTION_DOWN -> {
                downAt = event.eventTime
                return true
            }
            MotionEvent.ACTION_UP -> {}
            else -> return true
        }
        if (event.eventTime - downAt > 600) {
            onLongPress?.invoke()
            return true
        }
        // Logical units, not view pixels: the engine was given the page box
        // in the same space, and handing it device pixels would put every
        // tap in the last band on a 3x screen.
        val density = resources.displayMetrics.density
        val action = session.tapAction(event.x / density, event.y / density) ?: return true
        applyAndRedraw(action)
        return true
    }

    /**
     * Forwarded from the activity, which is where volume keys arrive.
     *
     * Returns whether the event was consumed, and that answer has to be the
     * engine's: the default key map takes the volume keys for page turns,
     * so a `false` here is Android putting its volume slider over the book.
     * The last page of every book is exactly where a shell that returned
     * "did anything move" would get this wrong.
     */
    fun handleKey(keyCode: Int): Boolean {
        val action = session.actionForKeyCode(keyCode) ?: return false
        return applyAndRedraw(action)
    }

    /**
     * Whether this key is bound, without acting on it.
     *
     * `onKeyUp` needs this. Consuming the down and letting the up through
     * still lets the system act on a volume press, so both halves have to
     * be claimed — but acting on both would turn two pages per press.
     */
    fun bindsKey(keyCode: Int): Boolean = session.actionForKeyCode(keyCode) != null

    /** Apply, repaint if it moved, and report whether the event was ours. */
    private fun applyAndRedraw(action: String): Boolean {
        lastAction = action
        val outcome = session.apply(action)
        if (outcome.needsRedraw) invalidate()
        // The status line wants updating even when nothing moved, because
        // "nothing moved" is what it is reporting.
        onMoved?.invoke()
        return outcome.consumed
    }

    fun renderStatus(): String = when (lastRender) {
        0 -> "ok"
        -1 -> "no session"
        -2 -> "nothing to render"
        -3 -> "bitmap format or size wrong"
        -4 -> "could not lock pixels"
        else -> "unknown ($lastRender)"
    }
}
