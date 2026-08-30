package com.ophymx.chapbook

import android.graphics.Rect
import android.os.Bundle
import android.view.MotionEvent
import android.view.View
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityNodeProvider

/**
 * The text surface as Android's accessibility tree.
 *
 * A rasterized page is a picture, and a picture of text is unusable with
 * a screen reader. This provider exposes one virtual node per
 * [Session.pageTextRuns] run — text plus on-screen bounds — which is what
 * TalkBack walks, explores by touch, and reads. It is the Android half of
 * the same shape the GTK viewer proves against AT-SPI: the engine hands
 * out runs, the platform wraps them in its own tree.
 *
 * A host view owes three delegations and one notification:
 *
 * ```
 * override fun getAccessibilityNodeProvider() = pageAccessibility
 * override fun dispatchHoverEvent(e) =
 *     pageAccessibility.dispatchHoverEvent(e) || super.dispatchHoverEvent(e)
 * // after anything that may have replaced the page (post-draw is easiest):
 * pageAccessibility.pageChanged()
 * ```
 *
 * `pageChanged` compares against the last seen page text, so calling it
 * after every draw is the intended pattern — a repaint no-ops, a turn
 * rebuilds the tree and announces the new page. All engine-side geometry
 * is logical units; nodes scale by the display density on the way out,
 * matching the metrics contract the host already follows.
 */
class PageAccessibility(private val host: View, private val session: Session) :
    AccessibilityNodeProvider() {

    private var runs: List<TextRun> = emptyList()
    private var seenText: String? = null
    private var focused = NONE
    private var hovered = NONE

    /**
     * Re-read the page if its text changed: rebuild the virtual tree,
     * tell accessibility services the subtree moved, and announce the new
     * page so a turn is audible rather than discoverable.
     */
    fun pageChanged() {
        val text = session.speakableText
        if (text == seenText) return
        seenText = text
        runs = session.pageTextRuns() ?: emptyList()
        focused = NONE
        hovered = NONE
        host.sendAccessibilityEvent(AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED)
        if (text.isNotEmpty()) host.announceForAccessibility(text)
    }

    /**
     * Explore-by-touch: under TalkBack, finger movement arrives as hover
     * events, and the service reads whichever virtual node the finger is
     * over. Returns whether the event was over page text.
     */
    fun dispatchHoverEvent(event: MotionEvent): Boolean {
        when (event.action) {
            MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_MOVE -> {
                val density = host.resources.displayMetrics.density
                val x = event.x / density
                val y = event.y / density
                val over = runs.indexOfFirst { it.rect.contains(x, y) }
                setHovered(if (over >= 0) over else NONE)
                return over >= 0
            }
            MotionEvent.ACTION_HOVER_EXIT -> {
                setHovered(NONE)
                return false
            }
        }
        return false
    }

    private fun setHovered(id: Int) {
        if (hovered == id) return
        val was = hovered
        hovered = id
        // Enter before exit, per the framework's own ordering.
        if (id != NONE) sendEvent(id, AccessibilityEvent.TYPE_VIEW_HOVER_ENTER)
        if (was != NONE) sendEvent(was, AccessibilityEvent.TYPE_VIEW_HOVER_EXIT)
    }

    override fun createAccessibilityNodeInfo(virtualViewId: Int): AccessibilityNodeInfo? {
        if (virtualViewId == HOST_VIEW_ID) {
            @Suppress("DEPRECATION")
            val info = AccessibilityNodeInfo.obtain(host)
            // The view's own default state — bounds, package, visibility —
            // then this tree's children on top. View's initializer does not
            // consult the provider, so this does not recurse.
            host.onInitializeAccessibilityNodeInfo(info)
            for (index in runs.indices) info.addChild(host, index)
            return info
        }
        val run = runs.getOrNull(virtualViewId) ?: return null
        @Suppress("DEPRECATION")
        val info = AccessibilityNodeInfo.obtain()
        info.packageName = host.context.packageName
        info.className = "android.widget.TextView"
        info.setSource(host, virtualViewId)
        info.setParent(host)
        info.text = run.text
        info.isVisibleToUser = true
        info.isEnabled = true
        info.isFocusable = false
        info.setBoundsInScreen(screenBounds(run))
        if (focused == virtualViewId) {
            info.isAccessibilityFocused = true
            info.addAction(AccessibilityNodeInfo.ACTION_CLEAR_ACCESSIBILITY_FOCUS)
        } else {
            info.addAction(AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS)
        }
        return info
    }

    override fun performAction(virtualViewId: Int, action: Int, arguments: Bundle?): Boolean {
        if (virtualViewId == HOST_VIEW_ID) return host.performAccessibilityAction(action, arguments)
        if (virtualViewId !in runs.indices) return false
        when (action) {
            AccessibilityNodeInfo.ACTION_ACCESSIBILITY_FOCUS -> {
                if (focused == virtualViewId) return false
                focused = virtualViewId
                sendEvent(virtualViewId, AccessibilityEvent.TYPE_VIEW_ACCESSIBILITY_FOCUSED)
                return true
            }
            AccessibilityNodeInfo.ACTION_CLEAR_ACCESSIBILITY_FOCUS -> {
                if (focused != virtualViewId) return false
                focused = NONE
                sendEvent(
                    virtualViewId,
                    AccessibilityEvent.TYPE_VIEW_ACCESSIBILITY_FOCUS_CLEARED,
                )
                return true
            }
        }
        return false
    }

    /** A run's bounds in screen pixels: logical rect × density + view origin. */
    private fun screenBounds(run: TextRun): Rect {
        val density = host.resources.displayMetrics.density
        val origin = IntArray(2)
        host.getLocationOnScreen(origin)
        return Rect(
            origin[0] + (run.rect.left * density).toInt(),
            origin[1] + (run.rect.top * density).toInt(),
            origin[0] + (run.rect.right * density).toInt(),
            origin[1] + (run.rect.bottom * density).toInt(),
        )
    }

    private fun sendEvent(virtualViewId: Int, type: Int) {
        val parent = host.parent ?: return
        @Suppress("DEPRECATION")
        val event = AccessibilityEvent.obtain(type)
        event.packageName = host.context.packageName
        event.className = "android.widget.TextView"
        event.setSource(host, virtualViewId)
        runs.getOrNull(virtualViewId)?.let { event.text.add(it.text) }
        parent.requestSendAccessibilityEvent(host, event)
    }

    private companion object {
        const val NONE = Int.MIN_VALUE
    }
}
