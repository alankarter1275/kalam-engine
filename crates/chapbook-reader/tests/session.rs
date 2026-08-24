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

fn isolate_library() {
    let dir = std::env::temp_dir().join(format!("chapbook-session-test-{}", std::process::id()));
    std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);
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
    isolate_library();
    let mut s = Session::open(&fixture("epub/illustrated.epub")).unwrap();
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
    isolate_library();
    let mut s = Session::open(&fixture("cbz/minimal.cbz")).unwrap();
    assert_eq!(s.kind(), BookKind::Comic);
    assert_eq!(s.spine_len(), 3);
    s.set_metrics(metrics());
    assert_eq!(s.page_count(), 1, "one page per comic unit");

    let pixmap = s.render().expect("comic page renders");
    // Page 1 is solid red (196,64,48): sample the center.
    let px = pixmap.pixel(300, 400).unwrap();
    assert!(
        px.red() > 150 && px.blue() < 90,
        "expected red page: {px:?}"
    );

    // Selection never engages on image pages.
    assert!(!s.selection_begin(300.0, 400.0));

    s.next_page();
    assert_eq!(s.spine(), 1, "page turn advances the spine for comics");
    let px = s.render().unwrap().pixel(300, 400).unwrap();
    assert!(
        px.green() > 90 && px.red() < 90,
        "expected green page: {px:?}"
    );
    s.next_page();
    let px = s.render().unwrap().pixel(300, 400).unwrap();
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
    isolate_library();
    let source = fixture("cbz/minimal.cbz");
    {
        let mut s = Session::open(&source).unwrap();
        s.set_metrics(metrics());
        s.next_page();
        s.next_page();
        assert_eq!(s.spine(), 2);
        s.save_position();
    }
    let mut s = Session::open(&source).unwrap();
    s.set_metrics(metrics());
    s.render();
    assert_eq!(s.spine(), 2, "comic position restores by page progression");
}

#[test]
fn pdf_session_reads_like_an_image_book() {
    isolate_library();
    let mut s = Session::open(&fixture("pdf/minimal.pdf")).unwrap();
    assert_eq!(s.kind(), BookKind::Pdf);
    assert_eq!(s.spine_len(), 2);
    s.set_metrics(metrics());
    // Red first page, blue second, no text selection.
    let px = s.render().unwrap().pixel(300, 400).unwrap();
    assert!(px.red() > 150 && px.blue() < 100, "red PDF page: {px:?}");
    assert!(!s.selection_begin(300.0, 400.0));
    s.next_page();
    assert_eq!(s.spine(), 1);
    let px = s.render().unwrap().pixel(300, 400).unwrap();
    assert!(px.blue() > 120 && px.red() < 100, "blue PDF page: {px:?}");
    s.save_position();
}
