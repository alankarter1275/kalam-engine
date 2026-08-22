//! Golden tests for the locator-text extraction — the normative offset space
//! behind every persisted reading position and annotation.
//!
//! If a snapshot here changes, stored offsets shift for every user: per
//! `docs/LOCATORS.md` and `chapbook_core::locator`, that requires bumping
//! `chapbook_core::LOCATOR_VERSION` in the same change. Do not casually
//! `cargo insta accept` this file.

use std::path::PathBuf;

use chapbook_core::LOCATOR_VERSION;
use chapbook_dom::{locator_text, parse_xhtml};

fn fixture_chapter(name: &str) -> chapbook_dom::Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/epub/src/minimal/OEBPS")
        .join(name);
    let bytes = std::fs::read(&path).unwrap();
    parse_xhtml(&bytes, &format!("OEBPS/{name}")).unwrap()
}

/// Offsets of anchor phrases in the fixture chapter, plus totals — a compact
/// fingerprint of the extraction function. `char_offset` positions anchor to
/// exactly these numbers.
#[test]
fn golden_offset_map_chapter1() {
    let doc = fixture_chapter("chapter1.xhtml");
    let text = locator_text(&doc);

    let anchors = [
        "Chapter One: A Beginning",
        "It was a truth universally",
        "nested emphasis",  // inside <em> inside <li>
        "the next chapter", // inside <a>
        "annotated span",   // inside <span class="note">
        "Block quotations", // inside <blockquote><p>
        "Second list item", // inside <li>
    ];
    let mut lines = vec![format!("locator_version: {LOCATOR_VERSION}")];
    lines.push(format!("total_chars: {}", text.chars().count()));
    for a in anchors {
        let byte = text
            .find(a)
            .unwrap_or_else(|| panic!("anchor missing: {a}"));
        let char_offset = text[..byte].chars().count();
        lines.push(format!("{char_offset:>5}  {a}"));
    }
    insta::assert_snapshot!(lines.join("\n"));
}

#[test]
fn raw_not_collapsed() {
    let doc = fixture_chapter("chapter1.xhtml");
    let text = locator_text(&doc);
    // Source line-wraps inside <p> survive raw: "fixture\n    file" style runs.
    assert!(
        text.contains('\n'),
        "locator text must preserve source newlines (raw, not collapsed)"
    );
    assert!(
        text.contains("fixture\n    file"),
        "raw text should keep the exact source wrap inside the first paragraph"
    );
}

#[test]
fn head_style_excluded_but_nbsp_counts() {
    let doc = parse_xhtml(
        b"<html><head><title>T</title><style>p{}</style></head>\
          <body><p>a&nbsp;b</p><script>var x;</script></body></html>",
        "t.xhtml",
    )
    .unwrap();
    assert_eq!(locator_text(&doc), "a\u{a0}b");
}

#[test]
fn offsets_count_scalars_not_bytes() {
    let doc = parse_xhtml(
        "<html><body><p>café</p><p id=\"x\">next</p></body></html>".as_bytes(),
        "t.xhtml",
    )
    .unwrap();
    let text = locator_text(&doc);
    assert_eq!(text, "cafénext");
    let p2 = doc.element_by_id("x").unwrap();
    // "café" is 4 chars (5 bytes); the second paragraph starts at char 4.
    assert_eq!(chapbook_dom::locator_offset_of(&doc, p2), Some(4));
}

/// End-to-end: capture a layered locator in fixture text, drift the text,
/// re-anchor through the quote layer.
#[test]
fn layered_capture_and_reanchor_roundtrip() {
    let doc = fixture_chapter("chapter1.xhtml");
    let text = locator_text(&doc);
    let total = text.chars().count() as u64;

    let byte = text.find("annotated span").unwrap();
    let offset = text[..byte].chars().count() as u32;
    let loc =
        chapbook_core::LayeredLocator::capture("OEBPS/chapter1.xhtml", 0, &text, offset, 0, total);

    // Same source resolves exactly.
    let r = chapbook_core::resolve_in_text(&text, &loc, true);
    assert_eq!(r, chapbook_core::ResolvedOffset::Exact(offset));

    // A "new edition" with an inserted editorial note re-anchors via quote.
    let drifted = text.replacen(
        "A second paragraph",
        "An editorial note was inserted here. A second paragraph",
        1,
    );
    let r = chapbook_core::resolve_in_text(&drifted, &loc, false);
    assert!(r.needs_rewrite());
    let drift = "An editorial note was inserted here. ".chars().count() as u32;
    assert_eq!(r.offset(), offset + drift);
}
