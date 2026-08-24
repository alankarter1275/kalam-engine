//! Golden snapshots of the M1 subcommand outputs against the checked-in
//! fixture EPUB. `cargo insta review` to update after intentional changes.

use std::path::PathBuf;

use chapbook_cli::commands;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/epub/minimal.epub")
}

#[test]
fn meta_snapshot() {
    insta::assert_snapshot!(commands::meta(&fixture()).unwrap());
}

#[test]
fn toc_snapshot() {
    insta::assert_snapshot!(commands::toc(&fixture()).unwrap());
}

/// `toc` opened every path as an EPUB, so asking a PDF or a comic for its
/// contents died in the zip reader. Both carry a real toc now — a PDF's
/// outline, a CBZ's ComicInfo bookmarks — and both route through the same
/// `Publication`.
#[test]
fn toc_reads_image_books_too() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let pdf = commands::toc(&dir.join("pdf/minimal.pdf")).unwrap();
    assert!(pdf.contains("Red plate  [page-1]"), "{pdf}");
    assert!(pdf.contains("  Text page  [page-3]"), "{pdf}");
    let cbz = commands::toc(&dir.join("cbz/minimal.cbz")).unwrap();
    assert!(cbz.contains("The Escapement  [page2.png]"), "{cbz}");
}

#[test]
fn text_snapshot() {
    insta::assert_snapshot!(commands::text(&fixture(), None).unwrap());
}
