//! Headless session behavior over the fixture books — the point of the
//! session extraction: reading logic testable without a window.

use std::path::PathBuf;

use chapbook_core::{BookKind, EdgeSizes, PageMetrics, Size};
use chapbook_reader::Session;

fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// Each test gets its own library dir: tests run in parallel threads and
/// the env var is process-global, so serialize env mutation behind a lock
/// and only touch the var while holding it.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Open a session against a per-test library dir. The env var is
/// process-global and tests run in parallel, so the set-and-open pair
/// holds a lock.
fn open_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, true)
}

/// Reopen against the same per-test library (position-persistence tests).
fn reopen_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, false)
}

fn open_library(name: &str, source: &str, fresh: bool) -> Session {
    let guard = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "chapbook-session-test-{}-{name}",
        std::process::id()
    ));
    if fresh {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);
    let session = Session::open(source).unwrap();
    drop(guard);
    session
}

/// Drive the async load path to completion: render (queues the load),
/// then poll until the unit lands. Panics after ~5s.
fn render_loaded(s: &mut Session) -> chapbook_reader::tiny_skia::Pixmap {
    for _ in 0..200 {
        s.render().expect("render");
        if !s.has_pending_loads() {
            // One more poll+render in case the last result just arrived.
            s.poll_loaded();
            return s.render().expect("render");
        }
        s.poll_loaded();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("unit never finished loading");
}

fn metrics() -> PageMetrics {
    PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
    }
}

#[test]
fn epub_session_renders_navigates_and_selects() {
    let mut s = open_isolated("epub-nav", &fixture("epub/illustrated.epub"));
    assert_eq!(s.kind(), BookKind::Epub);
    s.set_metrics(metrics());
    let pixmap = s.render().expect("page renders");
    assert_eq!((pixmap.width(), pixmap.height()), (600, 800));
    assert!(s.page_count() >= 2);

    // Selection: press near the top text line, drag right and down a line.
    assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
    s.selection_drag(400.0, 140.0);
    let (start, end) = s.selected_range().expect("non-empty selection");
    assert!(end > start);
    // The selected text comes back ready to paste: the locator space is
    // the raw source text, so its line breaks and indentation collapse.
    let text = s.selected_text().expect("selection carries text");
    assert!(
        text.starts_with("e Illustrated Chapter"),
        "selection starts mid-heading: {text:?}"
    );
    assert!(
        text.contains("Text before the picture"),
        "selection runs into the first paragraph: {text:?}"
    );
    assert!(!text.contains('\n'), "pasteable text has no line breaks");
    // The selected page renders with the highlight without panicking.
    s.render().unwrap();
    // Page navigation clears the selection.
    s.next_page();
    assert_eq!(s.selected_range(), None);
    assert_eq!(s.page(), 1);
    s.prev_page();
    assert_eq!(s.page(), 0);

    s.save_position();
}

#[test]
fn cbz_session_pages_through_images() {
    let mut s = open_isolated("cbz-pages", &fixture("cbz/minimal.cbz"));
    assert_eq!(s.kind(), BookKind::Comic);
    assert_eq!(s.spine_len(), 3);
    s.set_metrics(metrics());
    assert_eq!(s.page_count(), 1, "one page per comic unit");

    // Page 1 is solid red (196,64,48): sample the center.
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(
        px.red() > 150 && px.blue() < 90,
        "expected red page: {px:?}"
    );

    // Selection never engages on image pages, so there is nothing to copy.
    assert!(!s.selection_begin(300.0, 400.0));
    assert_eq!(s.selected_text(), None);

    s.next_page();
    assert_eq!(s.spine(), 1, "page turn advances the spine for comics");
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(
        px.green() > 90 && px.red() < 90,
        "expected green page: {px:?}"
    );
    s.next_page();
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(
        px.blue() > 120 && px.red() < 90,
        "expected blue page 10 last: {px:?}"
    );
    // End of book: stays put.
    s.next_page();
    assert_eq!(s.spine(), 2);

    s.save_position();
}

#[test]
fn cbz_position_persists_across_sessions() {
    let source = fixture("cbz/minimal.cbz");
    {
        let mut s = open_isolated("cbz-persist", &source);
        s.set_metrics(metrics());
        s.next_page();
        s.next_page();
        assert_eq!(s.spine(), 2);
        s.save_position();
    }
    let mut s = reopen_isolated("cbz-persist", &source);
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert_eq!(s.spine(), 2, "comic position restores by page progression");
}

#[test]
fn pdf_session_reads_like_an_image_book() {
    let mut s = open_isolated("pdf-read", &fixture("pdf/minimal.pdf"));
    assert_eq!(s.kind(), BookKind::Pdf);
    assert_eq!(s.spine_len(), 3);
    s.set_metrics(metrics());
    // Red first page, blue second; the rect pages carry no text.
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(px.red() > 150 && px.blue() < 100, "red PDF page: {px:?}");
    assert!(!s.selection_begin(300.0, 400.0));
    s.next_page();
    assert_eq!(s.spine(), 1);
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(px.blue() > 120 && px.red() < 100, "blue PDF page: {px:?}");
    s.save_position();
}

#[test]
fn pdf_text_selection_highlights() {
    let mut s = open_isolated("pdf-select", &fixture("pdf/minimal.pdf"));
    s.set_metrics(metrics());
    // Page 3 carries the Helvetica text lines.
    s.next_page();
    s.next_page();
    assert_eq!(s.spine(), 2);
    let before = render_loaded(&mut s);

    // The 300x400pt page scales into a 600x800 window (40px margins):
    // fit height 720 -> factor 1.8, image x-origin 30+40=70... derive from
    // hit-testing instead of hardcoding: sweep for a text hit.
    let mut anchor = None;
    'outer: for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            if s.selection_begin(x as f32, y as f32) {
                anchor = Some((x as f32, y as f32));
                break 'outer;
            }
        }
    }
    let (ax, ay) = anchor.expect("PDF text line is hit-testable");
    s.selection_drag(ax + 150.0, ay);
    let (start, end) = s.selected_range().expect("selection over PDF text");
    assert!(end > start);
    let text = s.selected_text().expect("PDF selection carries text");
    assert!(
        "Hello selection".starts_with(&text),
        "expected a prefix of the first text line, got {text:?}"
    );

    // The highlight visibly changes the render.
    let after = render_loaded(&mut s);
    assert_ne!(before.data(), after.data(), "selection must paint");
    s.selection_clear();
}
