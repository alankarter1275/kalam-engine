package com.ophymx.chapbook.demo

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.view.Gravity
import android.view.ViewGroup
import android.view.Window
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import com.ophymx.chapbook.Session
import java.io.File

/**
 * The demo: one book, one view, one status line, and a conformance report.
 *
 * There is no layout XML and no AndroidX on purpose. This is a spike whose
 * job is to answer questions about the binding, so everything on screen is
 * either a page of a book or a fact about the binding.
 */
class MainActivity : Activity() {

    private var session: Session? = null
    private var reader: ReaderView? = null
    private lateinit var status: TextView
    private lateinit var root: LinearLayout

    /** The bundled asset, and what the conformance harness reopens. */
    private var bookPath: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The default theme's action bar overlays the content, which put the
        // status line underneath it and made it look absent. A reader wants
        // the whole window anyway.
        requestWindowFeature(Window.FEATURE_NO_TITLE)

        // Before anything else: the engine has no voice until a host gives
        // it one, and every failure below reports through it.
        Session.initLogging(verbose = false)

        root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        // Explicit colours: the platform theme decides the default text
        // colour and the reader paints its own paper, so on a dark-themed
        // device the status line was white on white and simply gone.
        status = TextView(this).apply {
            textSize = 11f
            // Top padding clears the system status bar: with no action
            // bar the window starts at y=0.
            setPadding(16, 72, 16, 16)
            setBackgroundColor(0xFF202020.toInt())
            setTextColor(0xFFE0E0E0.toInt())
        }
        root.addView(status, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)

        // Rung 5 is a button because it has to be: a `content://` URI comes
        // back from the system picker and from nowhere else.
        root.addView(
            Button(this).apply {
                text = "open a document (content:// — rung 5)"
                textSize = 11f
                setOnClickListener { pickDocument() }
            },
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT,
        )

        val book = copyAssetToFiles("book.epub")
        bookPath = book.absolutePath
        show(Session.open(book.absolutePath, filesDir.absolutePath), "could not open ${book.absolutePath}")
    }

    /** Put a freshly opened session on screen, replacing whatever was there. */
    private fun show(opened: Session?, whenNull: String) {
        if (opened == null) {
            status.text = whenNull
            setContentView(root)
            return
        }
        session?.close()
        reader?.let(root::removeView)

        session = opened
        val view = ReaderView(this, opened)
        reader = view
        view.onMoved = { refreshStatus() }
        // A long press on the page runs the conformance harness — rung 4.
        // It opens its own sessions, so it takes the path, not the handle.
        view.onLongPress = { bookPath?.let(::showConformance) }
        root.addView(
            view,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0).apply { weight = 1f },
        )
        setContentView(root)
        refreshStatus()
    }

    private fun refreshStatus() {
        val s = session ?: return
        val p = s.position
        status.text = buildString {
            append(s.title)
            append("\nunit ${p.spine}, page ${p.page}")
            append("   ·   ${s.fontReport}")
            append("   ·   render: ${reader?.renderStatus() ?: "-"}")
            append("\ncache ${s.cacheBytes / 1024}k of ${s.cacheBudget / 1024}k")
            append("   ·   tap right to turn, left to go back; long-press for conformance")
        }
    }

    /**
     * Rung 5: ask the system for a document and open its descriptor.
     *
     * The MIME filter is every type, deliberately — a provider is free to
     * report `application/octet-stream` for a perfectly good EPUB, and the
     * point of the rung is that the bytes decide the format.
     */
    @Suppress("DEPRECATION")
    private fun pickDocument() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "*/*"
        }
        startActivityForResult(intent, REQUEST_DOCUMENT)
    }

    // `startActivityForResult` is what a spike with no AndroidX has; the
    // replacement lives in the activity-result API, which is AndroidX.
    @Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_DOCUMENT || resultCode != RESULT_OK) return
        val uri = data?.data ?: return
        val pfd = try {
            contentResolver.openFileDescriptor(uri, "r")
        } catch (e: Exception) {
            status.text = "could not open $uri: $e"
            return
        }
        if (pfd == null) {
            status.text = "no descriptor for $uri"
            return
        }
        // The harness needs a path to reopen and this has none, so
        // long-press keeps pointing at the bundled book.
        show(Session.openFd(pfd, filesDir.absolutePath), "could not read $uri")
    }

    /** Show the harness's report. This tests the binding, not the build. */
    private fun showConformance(path: String) {
        val report = TextView(this).apply {
            text = Session.conformance(path, filesDir.absolutePath)
            textSize = 11f
            setPadding(24, 24, 24, 24)
            gravity = Gravity.START
        }
        setContentView(ScrollView(this).apply { addView(report) })
    }

    /**
     * Assets are not files, so the spike makes one. The button above is the
     * case a real app has: a `content://` URI, no path, no extension.
     */
    private fun copyAssetToFiles(name: String): File {
        val out = File(filesDir, name)
        if (!out.exists()) {
            assets.open(name).use { input -> out.outputStream().use(input::copyTo) }
        }
        return out
    }

    override fun onStop() {
        super.onStop()
        // The only guaranteed callback there is: save the position and let
        // go of everything that can be rebuilt, while there is still a
        // process to do it from.
        session?.suspend()
    }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        // Android asks and does not negotiate. Every level is treated the
        // same because the session has one thing to give up and giving it
        // up costs a re-layout of the current page, not a lost position.
        session?.releaseCaches()
    }

    override fun onDestroy() {
        super.onDestroy()
        session?.close()
        session = null
    }

    private companion object {
        const val REQUEST_DOCUMENT = 1
    }
}
