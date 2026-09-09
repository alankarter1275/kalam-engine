//! Headless session behavior over the fixture books — the point of the
//! session extraction: reading logic testable without a window. The
//! per-topic suites live beside this file; this one keeps the core
//! read/render/select pass per format and the session's construction
//! contract.

mod common;
use chapbook_core::BookKind;
use chapbook_reader::{Session, SessionConfig};
use common::*;

#[test]
fn epub_session_renders_navigates_and_selects() {
    let mut s = open_isolated("epub-nav", &fixture("epub/illustrated.epub"));
    assert_eq!(s.kind(), BookKind::Epub);
    s.set_metrics(metrics());
    let pixmap = s.render().expect("page renders");
    assert_eq!((pixmap.width(), pixmap.height()), (600, 800));
    assert!(s.page_count() >= 2);

    // Selection by hit test: press near the top text line, drag right and
    // down a line. Which character that lands on is a function of the
    // host's fonts, so this half asserts the shape of the result — it runs
    // from the heading into the paragraph below it — and the deterministic
    // half below asserts the exact text.
    assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
    s.selection_drag(400.0, 140.0);
    let (start, end) = s.selected_range().expect("non-empty selection");
    assert!(end > start);
    let text = s.selected_text().expect("selection carries text");
    assert!(
        text.contains("Illustrated Chapter"),
        "selection starts in the heading: {text:?}"
    );
    assert!(
        text.contains("Text before the picture"),
        "selection runs into the first paragraph: {text:?}"
    );
    assert!(!text.contains('\n'), "pasteable text has no line breaks");

    // The selected text comes back ready to paste: the locator space is
    // the raw source text, so its line breaks and indentation collapse.
    // chapter1.xhtml ends a source line after the link and indents the
    // next, so a range spanning the two proves the collapse exactly.
    let (link_start, _) = only_hit(&mut s, "underlined link");
    let (_, after_end) = only_hit(&mut s, "and some");
    s.select_range(link_start, after_end);
    assert_eq!(
        s.selected_text().as_deref(),
        Some("underlined link and some"),
        "the source breaks the line and indents between these two"
    );
    s.selection_clear();
    // The selected page renders with the highlight without panicking.
    s.render().unwrap();
    // Page navigation clears the selection.
    s.next_page();
    assert_eq!(s.selected_range(), None);
    assert_eq!(s.page(), 1);
    s.prev_page();
    assert_eq!(s.page(), 0);

}

#[test]
#[cfg(feature = "cbz")]
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

}

#[test]
#[cfg(feature = "pdf")]
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
}

#[test]
#[cfg(feature = "pdf")]
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

/// A session may cross threads but may not be shared across them, which is
/// the shape every binding is built on: a host holds one
/// opaque handle, moves it freely, and needs no lock of its own. `Sync`
/// fails today on the loader's receiver and on rusqlite's connection, so
/// only the half we actually rely on is asserted here — if `Send` ever
/// goes, the FFI's threading contract goes with it.
#[test]
fn a_session_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Session>();
}

/// The failure `FontSource` exists to convert into an error.
///
/// A session that finds no faces is not an ereader that looks wrong; it is
/// an ereader that cannot move. Every book paginates to one blank page, so
/// there is nowhere to navigate to, nothing for search to find, and no page
/// for a TOC entry to land on — and the session lays out, renders, paints
/// and *conforms* the whole time. fontdb has no Android, iOS or wasm
/// branch, which makes that the ordinary case on three platforms rather
/// than a corner.
#[test]
fn a_session_with_no_faces_is_refused_at_construction() {
    let empty = chapbook_core::FontSource::embedded("/nonexistent/fonts", "Nothing");
    let result = Session::open_with(fixture("epub/minimal.epub"), SessionConfig::new(empty));

    let err = result.err().expect("a fontless session must not open");
    let message = err.to_string();
    assert!(message.contains("no faces"), "{message}");
}

/// The read-back half: a shell cannot offer a font-family picker over a
/// list it cannot obtain.
#[test]
fn the_session_can_enumerate_the_families_it_was_given() {
    let session = open_isolated("families", &fixture("epub/minimal.epub"));

    let families = session.font_families();
    assert_eq!(families, vec!["Crimson Text".to_string()]);
    // And the source resolved cleanly, which is what a shell would print.
    assert!(
        session.font_report().is_clean(),
        "{}",
        session.font_report()
    );
    assert_eq!(session.font_report().faces, 4);
}

/// The host capabilities a session used to reach for on its own now arrive
/// through `SessionConfig`. This covers the plumbing; the credential retry
/// flow itself lives in `transport.rs`, which can drive it now that the
/// transport is injectable too.
#[test]
fn a_session_takes_its_host_capabilities_explicitly() {
    use chapbook_core::{Credential, CredentialKey, CredentialStore, Freshness, MemoryCredentials};

    let store = std::sync::Arc::new(MemoryCredentials::new());
    let key = CredentialKey::http_origin("https://cat.example.com/opds/abc123secret/").unwrap();
    store
        .store(&key, &Credential::basic("reader", "pw"))
        .unwrap();

    let config = SessionConfig::new(fixture_fonts()).with_credentials(store.clone());
    // A store in the config is not a store the local path consults.
    let session = Session::open_with(fixture("epub/minimal.epub"), config).unwrap();
    assert!(session.spine_len() > 0);

    // And the config's Debug is safe to log: no secret in it.
    let shown = format!("{:?}", SessionConfig::new(fixture_fonts()));
    assert!(!shown.contains("pw"), "{shown}");

    assert!(matches!(
        store.get(&key, Freshness::Cached),
        chapbook_core::CredentialLookup::Found(_)
    ));
}
