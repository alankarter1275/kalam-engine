package com.ophymx.chapbook.demo

import android.app.Activity
import android.os.Bundle
import android.view.Gravity
import android.view.ViewGroup
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
    private lateinit var status: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val book = copyAssetToFiles("book.epub")
        val opened = Session.open(book.absolutePath, filesDir.absolutePath)

        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        status = TextView(this).apply {
            textSize = 11f
            setPadding(16, 16, 16, 16)
        }
        root.addView(status, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)

        if (opened == null) {
            status.text = "could not open ${book.absolutePath}"
            setContentView(root)
            return
        }
        session = opened

        val reader = ReaderView(this, opened)
        reader.onMoved = { refreshStatus(reader) }
        root.addView(
            reader,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                0,
            ).apply { weight = 1f },
        )

        // Long-press anywhere on the status line runs the conformance
        // harness — rung 4. It opens its own sessions, so it takes the path.
        status.setOnLongClickListener {
            showConformance(book.absolutePath)
            true
        }

        setContentView(root)
        refreshStatus(reader)
    }

    private fun refreshStatus(reader: ReaderView) {
        val s = session ?: return
        val p = s.position
        status.text = buildString {
            append(s.title)
            append("\nunit ${p.spine}, page ${p.page}")
            append("   ·   faces: ${s.faceCount}")
            append("   ·   render: ${reader.renderStatus()}")
            append("\ntap right to turn, left to go back; long-press here for conformance")
        }
    }

    /** Show the harness's report. This tests the binding, not the build. */
    private fun showConformance(path: String) {
        val report = TextView(this).apply {
            text = Session.conformance(path)
            textSize = 11f
            setPadding(24, 24, 24, 24)
            gravity = Gravity.START
        }
        setContentView(ScrollView(this).apply { addView(report) })
    }

    /**
     * Assets are not files, so the spike makes one. A real app would take a
     * `content://` URI and never have a path at all, which is rung 5 and the
     * reason `docs/FFI.md` wants typed sources.
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
        // The only guaranteed callback there is. Today there is nothing to
        // call — `docs/FFI.md` wants `suspend()` here.
        }

    override fun onDestroy() {
        super.onDestroy()
        session?.close()
        session = null
    }
}
