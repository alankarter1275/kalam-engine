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

#[test]
fn text_snapshot() {
    insta::assert_snapshot!(commands::text(&fixture(), None).unwrap());
}
