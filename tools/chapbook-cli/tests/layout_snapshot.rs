//! Golden fragment-tree dumps over the fixture EPUB — M3's regression
//! surface for box construction, shaping, and page breaking. Layout is
//! deterministic because the dump uses only the vendored fixture fonts.

use std::path::PathBuf;

use chapbook_cli::commands;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/epub/minimal.epub")
}

#[test]
fn layout_chapter1_snapshot() {
    insta::assert_snapshot!(commands::layout(&fixture(), 0).unwrap());
}

#[test]
fn layout_chapter2_snapshot() {
    insta::assert_snapshot!(commands::layout(&fixture(), 1).unwrap());
}

#[test]
fn layout_illustrated_snapshot() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/epub/illustrated.epub");
    insta::assert_snapshot!(commands::layout(&path, 0).unwrap());
}
