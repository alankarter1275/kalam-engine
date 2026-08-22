use std::path::PathBuf;

use chapbook_epub::Book;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/epub")
        .join(name)
}

fn minimal() -> Book {
    Book::open(&fixture("minimal.epub")).expect("fixture EPUB should open")
}

#[test]
fn metadata() {
    let book = minimal();
    let md = book.metadata();
    assert_eq!(md.title.as_deref(), Some("The Minimal Book"));
    assert_eq!(md.authors, vec!["Ada Fixture".to_string()]);
    assert_eq!(md.language.as_deref(), Some("en"));
    assert_eq!(md.epub_version, "3.0");
    assert!(!book.is_fixed_layout());
}

#[test]
fn spine_order_and_hrefs() {
    let book = minimal();
    let spine = book.spine();
    assert_eq!(spine.len(), 2);
    assert_eq!(spine[0].href, "OEBPS/chapter1.xhtml");
    assert_eq!(spine[1].href, "OEBPS/chapter2.xhtml");
    assert!(spine.iter().all(|s| s.linear));
    assert!(spine
        .iter()
        .all(|s| s.media_type == "application/xhtml+xml"));
}

#[test]
fn toc_structure() {
    let book = minimal();
    let toc = book.toc();
    assert_eq!(toc.len(), 2);
    assert_eq!(toc[0].label, "Chapter One: A Beginning");
    assert_eq!(toc[0].spine_index, Some(0));
    assert_eq!(toc[1].spine_index, Some(1));
    assert_eq!(toc[1].children.len(), 1);
    assert_eq!(toc[1].children[0].fragment.as_deref(), Some("part2"));
}

#[test]
fn chapter_bytes_and_resource_resolution() {
    let book = minimal();
    let ch1 = book.chapter_xhtml(0).unwrap();
    assert!(std::str::from_utf8(&ch1)
        .unwrap()
        .contains("<h1>Chapter One"));

    // Stylesheet referenced relative to the chapter document.
    let css = book.resource("OEBPS/chapter1.xhtml", "style.css").unwrap();
    assert_eq!(css.media_type, "text/css");
    assert!(std::str::from_utf8(&css.data)
        .unwrap()
        .contains("blockquote"));
}

#[test]
fn missing_resources_error_cleanly() {
    let book = minimal();
    assert!(book.chapter_xhtml(99).is_err());
    assert!(book.resource("OEBPS/chapter1.xhtml", "nope.png").is_err());
}
