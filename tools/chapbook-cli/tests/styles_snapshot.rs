//! Golden computed-style dumps over the fixture EPUB — M2's regression
//! surface for cascade, specificity, inheritance, and em-resolution.

use std::path::PathBuf;

use chapbook_cli::commands;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/epub/minimal.epub")
}

#[test]
fn styles_chapter1_snapshot() {
    insta::assert_snapshot!(commands::styles(&fixture(), 0).unwrap());
}

#[test]
fn styles_chapter2_snapshot() {
    insta::assert_snapshot!(commands::styles(&fixture(), 1).unwrap());
}
