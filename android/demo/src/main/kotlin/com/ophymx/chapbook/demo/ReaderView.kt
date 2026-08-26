package com.ophymx.chapbook.demo

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.view.MotionEvent
import android.view.View
import com.ophymx.chapbook.Session

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
    }

    /**
     * The tap-zone policy `docs/FFI.md` argues belongs in the engine:
     * left third back, right third forward, middle band something else.
     * Here the middle cycles the theme, because sepia is the only colour
     * this demo can put on screen and it is what tests the channel order.
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
        val third = width / 3f
        val moved = when {
            event.x < third -> session.prevPage()
            event.x > third * 2 -> session.nextPage()
            else -> {
                session.cycleTheme()
                invalidate()
                onMoved?.invoke()
                return true
            }
        }
        // Use the return value. Do not compare page numbers across a turn.
        if (moved) {
            invalidate()
            onMoved?.invoke()
        }
        return true
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
