//! `LayeredLocator` onto an OPDS Progression document and back.
//!
//! The claim being tested is the one docs/LOCATORS.md makes — that the
//! layered record was shaped to serialize straight into this protocol —
//! and the load-bearing part of it is the quote layer surviving as a text
//! fragment, since `progression` alone is a chapter-sized guess.
#![cfg(feature = "progression")]

use chapbook_core::{LayeredLocator, Quote, LOCATOR_VERSION};
use chapbook_opds::progression::{from_progression, to_progression, RemotePosition};
use opds_client::progression::{Device, Progression};

fn device() -> Device {
    Device {
        id: "urn:uuid:0d3d5b64-test".into(),
        name: "chapbook".into(),
    }
}

fn point(prefix: &str, suffix: &str) -> LayeredLocator {
    LayeredLocator {
        spine_href: "OEBPS/chapter4.xhtml".into(),
        spine_index: 3,
        char_offset: 1200,
        locator_version: LOCATOR_VERSION,
        quote: Quote {
            prefix: prefix.into(),
            exact: String::new(),
            suffix: suffix.into(),
        },
        spine_fraction: 0.25,
        book_progression: 0.42,
    }
}

/// A reading position: `exact` is empty, so the point is written as the
/// text starting at it, and the match's own start is the point.
#[test]
fn a_point_becomes_a_prefixed_text_fragment() {
    let doc = to_progression(
        &point("the harbour was ", "quiet that morning"),
        device(),
        "2026-08-30T12:00:00Z",
        Some("Chapter Four".into()),
    );

    assert_eq!(doc.progression, 0.42);
    assert_eq!(
        doc.references[0],
        "OEBPS/chapter4.xhtml#:~:text=the%20harbour%20was%20-,quiet%20that%20morning"
    );
    assert_eq!(
        doc.references[1], "OEBPS/chapter4.xhtml",
        "the bare spine item is the coarser refinement"
    );
    assert!(
        !doc.references[0].contains("1200"),
        "char_offset must not cross: it is meaningless off this device"
    );
}

/// An annotation endpoint has a real `exact`, so both context terms are
/// written and the suffix is the trailing `-suffix`, not the match.
#[test]
fn a_range_endpoint_writes_both_context_terms() {
    let mut loc = point("before ", " after");
    loc.quote.exact = "the quoted words".into();
    let doc = to_progression(&loc, device(), "2026-08-30T12:00:00Z", None);
    assert_eq!(
        doc.references[0],
        "OEBPS/chapter4.xhtml#:~:text=before%20-,the%20quoted%20words,-%20after"
    );
}

/// The whole point of the exercise: what one device writes, another reads
/// back as the same quote and spine item.
#[test]
fn a_position_round_trips_through_the_wire_shape() {
    for (prefix, exact, suffix) in [
        ("the harbour was ", "", "quiet that morning"),
        ("before ", "the quoted words", " after"),
        ("", "", "opens the chapter"),
        // Every character the directive syntax gives meaning to, plus
        // non-Latin text, which is what percent-encoding is really for.
        ("a, b - c & d", "e,f-g&h", "الفصل الرابع"),
    ] {
        let mut loc = point(prefix, suffix);
        loc.quote.exact = exact.into();
        let doc = to_progression(&loc, device(), "2026-08-30T12:00:00Z", None);
        let back = from_progression(&doc);

        assert_eq!(back.progression, loc.book_progression);
        assert_eq!(back.spine_href.as_deref(), Some("OEBPS/chapter4.xhtml"));
        assert_eq!(
            back.quote
                .as_ref()
                .map(|q| (q.prefix.as_str(), q.exact.as_str(), q.suffix.as_str())),
            Some(if exact.is_empty() {
                // A point's anchoring text is its suffix, and that is
                // where it comes back as the match.
                (prefix, suffix, "")
            } else {
                (prefix, exact, suffix)
            }),
            "round trip failed for {prefix:?}/{exact:?}/{suffix:?}"
        );
    }
}

/// A point with nothing after it — the end of the last chapter — has no
/// text to match. Anchoring on the prefix instead would put the match
/// start *before* the point, which is a wrong position rather than a
/// missing one, so the quote layer drops and `progression` carries it.
#[test]
fn a_point_at_the_end_of_a_book_degrades_to_the_fraction() {
    let doc = to_progression(
        &point("ends the chapter", ""),
        device(),
        "2026-08-30T12:00:00Z",
        None,
    );
    let back = from_progression(&doc);
    assert_eq!(back.quote, None);
    assert_eq!(back.spine_href.as_deref(), Some("OEBPS/chapter4.xhtml"));
    assert_eq!(back.progression, 0.42);
}

/// A quote with nothing after the point cannot start a match, so no
/// directive is written rather than an empty one.
#[test]
fn a_quote_with_no_anchoring_text_writes_no_directive() {
    let mut loc = point("", "");
    loc.quote.exact = String::new();
    let doc = to_progression(&loc, device(), "2026-08-30T12:00:00Z", None);
    assert_eq!(doc.references, vec!["OEBPS/chapter4.xhtml".to_string()]);
}

/// A peer that speaks only `progression` — the draft's floor — still
/// yields a usable position.
#[test]
fn a_bare_progression_is_still_a_position() {
    let doc = Progression {
        title: None,
        modified: "2026-08-30T12:00:00Z".into(),
        device: device(),
        progression: 0.6,
        references: Vec::new(),
    };
    assert_eq!(
        from_progression(&doc),
        RemotePosition {
            progression: 0.6,
            spine_href: None,
            quote: None,
            title: None,
            device: device(),
            modified: "2026-08-30T12:00:00Z".into(),
        }
    );
}

/// The references a peer sends are not ours. An id fragment names an
/// element and carries no text to re-anchor against; a text fragment
/// later in the array must still win over it.
#[test]
fn the_most_specific_reference_wins_wherever_it_sits() {
    let doc = Progression {
        title: None,
        modified: "2026-08-30T12:00:00Z".into(),
        device: device(),
        progression: 0.6,
        references: vec![
            "chapter1.html#par26".into(),
            "#page=87".into(),
            "chapter1.html#:~:text=It%20was%20expected".into(),
        ],
    };
    let back = from_progression(&doc);
    assert_eq!(back.spine_href.as_deref(), Some("chapter1.html"));
    assert_eq!(
        back.quote.map(|q| q.exact),
        Some("It was expected".to_string())
    );
}

/// With no text fragment anywhere, the first reference that names a path
/// still gives a chapter to open.
#[test]
fn a_reference_without_a_directive_still_names_a_chapter() {
    let doc = Progression {
        title: None,
        modified: "2026-08-30T12:00:00Z".into(),
        device: device(),
        progression: 0.6,
        references: vec!["#t=849.250".into(), "chapter1.html#par26".into()],
    };
    let back = from_progression(&doc);
    assert_eq!(back.spine_href.as_deref(), Some("chapter1.html"));
    assert_eq!(back.quote, None);
}
