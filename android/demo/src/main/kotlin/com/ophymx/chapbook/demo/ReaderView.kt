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

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        if (w <= 0 || h <= 0) return
        val density = resources.displayMetrics.density
        // Metrics are in logical units and the scale carries the DPI, so the
        // engine lays out at a readable size and rasterizes at the panel's.
        session.setMetrics(w / density, h / density, 24f, density)
        bitmap?.recycle()
        bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val target = bitmap ?: return
        // Straight into the bitmap's own pixels: tiny-skia hands back
        // premultiplied RGBA8888 and an ARGB_8888 bitmap is premultiplied
        // RGBA in memory, so the binding memcpys rather than converting.
        lastRender = session.renderInto(target)
        if (lastRender == 0) canvas.drawBitmap(target, 0f, 0f, null)
    }

    /** The tap-zone policy `docs/FFI.md` argues belongs in the engine. */
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.action != MotionEvent.ACTION_UP) return true
        val moved = if (event.x < width / 3f) session.prevPage() else session.nextPage()
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
