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

/// Anchor a selection on the first hit-testable text on the current page.
/// The placed image rect depends on the page's natural size and the
/// margins, so sweep for a hit rather than hardcoding coordinates.
fn sweep_for_text(s: &mut Session) -> (f32, f32) {
    for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            if s.selection_begin(x as f32, y as f32) {
                return (x as f32, y as f32);
            }
        }
    }
    panic!("no hit-testable text on this page");
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

    let (ax, ay) = sweep_for_text(&mut s);
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

#[test]
fn epub_highlight_persists_across_sessions() {
    let source = fixture("epub/illustrated.epub");
    let (start, end, text) = {
        let mut s = open_isolated("epub-highlight", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");
        assert!(s.selection_begin(100.0, 70.0));
        s.selection_drag(400.0, 140.0);
        let (start, end) = s.selected_range().expect("non-empty selection");
        let text = s.selected_text().expect("selection carries text");
        let id = s.add_highlight().expect("highlight is stored");
        assert!(id > 0);
        assert_eq!(s.highlights(0).len(), 1, "visible without a reload");
        (start, end, text)
    };

    let mut s = reopen_isolated("epub-highlight", &source);
    s.set_metrics(metrics());
    let stored = s.highlights(0).to_vec();
    assert_eq!(stored.len(), 1, "highlight survives the session");
    assert_eq!(
        (stored[0].start, stored[0].end),
        (start, end),
        "same edition resolves the exact offsets"
    );
    assert_eq!(stored[0].text.as_deref(), Some(text.as_str()));

    let painted = s.render().expect("page renders");
    // Locator space doesn't move under relayout, so the highlight is
    // unchanged at a different font size.
    s.adjust_font(4.0);
    assert_eq!(s.highlights(0), &stored[..]);
    s.adjust_font(-4.0);

    s.remove_highlight(stored[0].id);
    assert!(s.highlights(0).is_empty(), "delete clears the cache too");
    let plain = s.render().expect("page renders");
    assert_ne!(painted.data(), plain.data(), "the highlight was painted");

    let mut s = reopen_isolated("epub-highlight", &source);
    s.set_metrics(metrics());
    assert!(s.highlights(0).is_empty(), "delete survives the session");
}

#[test]
fn pdf_highlight_resolves_against_the_page_text_layer() {
    let source = fixture("pdf/minimal.pdf");
    let (start, end) = {
        let mut s = open_isolated("pdf-highlight", &source);
        s.set_metrics(metrics());
        // Page 3 carries the text lines.
        s.next_page();
        s.next_page();
        render_loaded(&mut s);
        let (ax, ay) = sweep_for_text(&mut s);
        s.selection_drag(ax + 150.0, ay);
        let range = s.selected_range().expect("selection over PDF text");
        assert!(s.add_highlight().expect("PDF highlight is stored") > 0);
        range
    };

    let mut s = reopen_isolated("pdf-highlight", &source);
    s.set_metrics(metrics());
    s.next_page();
    s.next_page();
    assert_eq!(s.spine(), 2);
    // The page's text layer is what the endpoints resolve against, so
    // nothing resolves until the page has loaded.
    assert!(
        s.highlights(2).is_empty(),
        "unresolved before the page loads"
    );
    render_loaded(&mut s);
    let stored = s.highlights(2).to_vec();
    assert_eq!(stored.len(), 1);
    assert_eq!((stored[0].start, stored[0].end), (start, end));
}

#[test]
fn comic_pages_take_no_highlights() {
    let mut s = open_isolated("cbz-highlight", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert_eq!(s.add_highlight(), None, "no text layer to anchor to");
    assert!(s.highlights(0).is_empty());
}

#[test]
fn display_list_is_the_backend_contract() {
    use chapbook_reader::chapbook_paint::DisplayOp;

    let mut s = open_isolated("epub-displaylist", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    let dl = s.display_list().expect("page has a display list");

    assert_eq!((dl.size.w, dl.size.h), (600.0, 800.0), "page-space size");
    // The page ground is op 0 so a theme is a color choice, not a
    // renderer concern.
    match dl.ops.first().expect("at least the background") {
        DisplayOp::FillRect { rect, color } => {
            assert_eq!((rect.size.w, rect.size.h), (600.0, 800.0));
            assert_eq!(*color, chapbook_core::Theme::Light.background());
        }
        op => panic!("expected the page ground first, got {op:?}"),
    }
    assert!(
        dl.ops
            .iter()
            .any(|op| matches!(op, DisplayOp::GlyphRun { .. })),
        "a text page paints glyphs"
    );

    // The seam is complete: a shell can reproduce render() from the
    // public API alone — ops, fonts, images.
    let mut pixmap = chapbook_reader::tiny_skia::Pixmap::new(600, 800).expect("pixmap");
    let mut renderer = chapbook_reader::chapbook_render_tinyskia::Renderer::new();
    let (fonts, images) = s.paint_resources();
    renderer.render(&dl, fonts, images, 1.0, &mut pixmap);
    let theirs = s.render().expect("render");
    assert_eq!(pixmap.data(), theirs.data(), "same ops, same pixels");
}

#[test]
fn image_units_key_their_ops_into_the_store() {
    use chapbook_reader::chapbook_paint::DisplayOp;

    let mut s = open_isolated("cbz-displaylist", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    let dl = s.display_list().expect("comic page has a display list");
    let resource = dl
        .ops
        .iter()
        .find_map(|op| match op {
            DisplayOp::Image { resource, .. } => Some(*resource),
            _ => None,
        })
        .expect("a comic page is an image op");
    assert!(
        s.image_store().get(resource).is_some(),
        "the op's key resolves in the store the session hands out"
    );
}
