//! chapbook's mark onto a Web Annotation and back, and — the part that
//! matters more — what happens to somebody else's annotation on the way
//! through.

use std::collections::BTreeMap;

use chapbook_annotations::mapping::{from_annotation, to_annotation, Mark, NS};
use chapbook_annotations::model::{Annotation, Selector};
use chapbook_core::{LayeredLocator, Quote, LOCATOR_VERSION};
use chapbook_library::AnnotationKind;
use serde_json::{json, Value};

const SOURCE: &str = "https://library.example.com/files/moby-dick.epub";

fn locator(offset: u32) -> LayeredLocator {
    LayeredLocator {
        spine_href: "OEBPS/chapter4.xhtml".into(),
        spine_index: 3,
        char_offset: offset,
        locator_version: LOCATOR_VERSION,
        quote: Quote {
            prefix: "the harbour was ".into(),
            exact: "quiet that morning".into(),
            suffix: " and the".into(),
        },
        spine_fraction: 0.25,
        book_progression: 0.42,
    }
}

fn highlight() -> Mark {
    Mark {
        kind: AnnotationKind::Highlight,
        start: locator(1200),
        end: Some(locator(1218)),
        text: Some("the chase begins here".into()),
        color: Some("#ffcc00".into()),
        created: Some("2026-08-30T12:00:00Z".into()),
        modified: Some("2026-08-30T12:00:00Z".into()),
    }
}

/// Through JSON, not just through the types — the wire is the thing being
/// claimed about.
fn round_trip(mark: &Mark) -> Mark {
    let document = to_annotation(mark, SOURCE);
    let json = serde_json::to_string(&document).unwrap();
    let parsed: Annotation = serde_json::from_str(&json).unwrap();
    from_annotation(&parsed)
}

#[test]
fn a_highlight_survives_the_wire_intact() {
    assert_eq!(round_trip(&highlight()), highlight());
}

#[test]
fn every_kind_survives_with_its_motivation() {
    for kind in [
        AnnotationKind::Bookmark,
        AnnotationKind::Highlight,
        AnnotationKind::Note,
    ] {
        let mark = Mark {
            kind,
            end: if kind == AnnotationKind::Bookmark {
                None
            } else {
                Some(locator(1218))
            },
            ..highlight()
        };
        assert_eq!(round_trip(&mark), mark, "{kind:?} did not survive");
    }
}

/// The three layers, in the order the resolve chain wants them.
#[test]
fn the_selector_stack_is_the_layered_locator() {
    let document = to_annotation(&highlight(), SOURCE);
    let names: Vec<Option<&str>> = document
        .target
        .selector
        .iter()
        .map(Selector::type_name)
        .collect();
    assert_eq!(
        names,
        vec![
            Some("TextQuoteSelector"),
            Some("TextPositionSelector"),
            Some("ProgressSelector")
        ]
    );
    assert_eq!(document.target.source, SOURCE);
    assert_eq!(
        document.target.extra.get(&format!("{NS}:spineHref")),
        Some(&Value::from("OEBPS/chapter4.xhtml"))
    );
}

/// An offset is only meaningful against one extraction. A peer that sent
/// one without saying which must not be believed — a wrong highlight is
/// worse than a re-anchored one.
#[test]
fn an_unstamped_position_selector_is_not_believed() {
    let foreign: Annotation = serde_json::from_value(json!({
        "@context": "http://www.w3.org/ns/anno.jsonld",
        "type": "Annotation",
        "motivation": "highlighting",
        "target": {
            "source": SOURCE,
            "selector": [
                {"type": "TextQuoteSelector", "exact": "quiet that morning",
                 "prefix": "the harbour was ", "suffix": " and the"},
                {"type": "TextPositionSelector", "start": 99_999, "end": 100_017}
            ]
        }
    }))
    .unwrap();

    let mark = from_annotation(&foreign);
    assert_eq!(mark.start.char_offset, 0, "an unstamped offset was taken");
    assert_eq!(
        mark.start.locator_version, 0,
        "an unstamped offset must not claim this build's version"
    );
    assert_eq!(
        mark.start.quote.exact, "quiet that morning",
        "the quote layer is what crosses between clients"
    );
    assert_eq!(
        mark.end, None,
        "no trustworthy end without a trusted offset"
    );
}

/// Same rule when the stamp is there but names a different extraction.
#[test]
fn a_position_from_another_locator_version_is_not_believed() {
    let stale: Annotation = serde_json::from_value(json!({
        "type": "Annotation",
        "target": {
            "source": SOURCE,
            "selector": [
                {"type": "TextQuoteSelector", "exact": "quiet"},
                {"type": "TextPositionSelector", "start": 1200, "end": 1218,
                 "chapbook:locatorVersion": LOCATOR_VERSION + 7}
            ]
        }
    }))
    .unwrap();
    let mark = from_annotation(&stale);
    assert_eq!(mark.start.char_offset, 0);
    assert_eq!(mark.start.locator_version, 0);
    assert_eq!(
        mark.start.quote.exact, "quiet",
        "the quote still crosses; only the offset is refused"
    );

    // The same document stamped with *this* build's version is believed —
    // without which the assertions above would hold for the wrong reason.
    let matching: Annotation = serde_json::from_value(json!({
        "type": "Annotation",
        "target": {
            "source": SOURCE,
            "selector": [
                {"type": "TextQuoteSelector", "exact": "quiet"},
                {"type": "TextPositionSelector", "start": 1200, "end": 1218,
                 "chapbook:locatorVersion": LOCATOR_VERSION}
            ]
        }
    }))
    .unwrap();
    let mark = from_annotation(&matching);
    assert_eq!(mark.start.char_offset, 1200);
    assert_eq!(mark.start.locator_version, LOCATOR_VERSION);
    assert_eq!(mark.end.map(|e| e.char_offset), Some(1218));
}

/// A Thorium-shaped annotation: no chapbook members at all. It has to come
/// back usable, because the quote and the fraction are the layers that were
/// designed to cross.
#[test]
fn a_foreign_annotation_is_still_a_usable_mark() {
    let foreign: Annotation = serde_json::from_value(json!({
        "@context": "http://www.w3.org/ns/anno.jsonld",
        "id": "urn:uuid:9f1b",
        "type": "Annotation",
        "motivation": "commenting",
        "body": {"type": "TextualBody", "value": "a note from elsewhere",
                 "format": "text/plain"},
        "target": {
            "source": SOURCE,
            "selector": [
                {"type": "TextQuoteSelector", "exact": "Call me Ishmael",
                 "prefix": "Loomings. ", "suffix": ". Some years"},
                {"type": "ProgressionSelector", "value": 0.031},
                {"type": "FragmentSelector",
                 "conformsTo": "http://www.idpf.org/epub/linking/cfi/epub-cfi.html",
                 "value": "epubcfi(/6/14!/4/2/2/1:0)"}
            ]
        }
    }))
    .unwrap();

    let mark = from_annotation(&foreign);
    assert_eq!(mark.kind, AnnotationKind::Note);
    assert_eq!(mark.text.as_deref(), Some("a note from elsewhere"));
    assert_eq!(mark.start.quote.exact, "Call me Ishmael");
    assert_eq!(mark.start.quote.prefix, "Loomings. ");
    assert_eq!(
        mark.start.book_progression, 0.031,
        "Readium's other spelling of the progress selector must be read"
    );
}

/// The rule the crate is built around: a container is shared, and a sync
/// that drops members it does not model deletes another client's data on a
/// success response.
#[test]
fn members_this_crate_does_not_model_survive_a_round_trip() {
    let original = json!({
        "@context": "http://www.w3.org/ns/anno.jsonld",
        "id": "urn:uuid:9f1b",
        "type": "Annotation",
        "motivation": "highlighting",
        "creator": {"type": "Person", "name": "Somebody Else"},
        "thorium:pageNumber": 87,
        "audience": {"type": "schema:Person"},
        "target": {
            "type": "SpecificResource",
            "source": SOURCE,
            "thorium:headings": ["Chapter 4"],
            "selector": [
                {"type": "TextQuoteSelector", "exact": "Call me Ishmael",
                 "thorium:normalized": true},
                {"type": "DomRangeSelector", "startContainer": "/div[1]"}
            ]
        }
    });

    let parsed: Annotation = serde_json::from_value(original.clone()).unwrap();
    let written = serde_json::to_value(&parsed).unwrap();
    assert_eq!(written, original, "a member was lost or moved");
}

/// An unknown selector type is kept verbatim rather than dropped or
/// erroring the whole document.
#[test]
fn an_unmodelled_selector_is_kept_whole() {
    let document: Annotation = serde_json::from_value(json!({
        "type": "Annotation",
        "target": {"source": SOURCE, "selector": [
            {"type": "SvgSelector", "value": "<svg/>"}
        ]}
    }))
    .unwrap();
    assert!(matches!(
        document.target.selector.first(),
        Some(Selector::Other(_))
    ));
    assert_eq!(document.target.selector[0].type_name(), Some("SvgSelector"));
}

/// A malformed selector — the type says quote, the member that makes it one
/// is missing — must not become an empty quote, which would match at offset
/// zero and mark the start of the chapter.
#[test]
fn a_quote_selector_without_its_quote_is_not_invented() {
    let document: Annotation = serde_json::from_value(json!({
        "type": "Annotation",
        "target": {"source": SOURCE, "selector": [
            {"type": "TextQuoteSelector", "prefix": "only context"}
        ]}
    }))
    .unwrap();
    assert!(matches!(
        document.target.selector.first(),
        Some(Selector::Other(_))
    ));
    assert_eq!(from_annotation(&document).start.quote, Quote::default());
}

/// A bookmark is a point: no end, and nothing should invent one.
#[test]
fn a_bookmark_stays_a_point() {
    let mark = Mark {
        kind: AnnotationKind::Bookmark,
        end: None,
        text: None,
        color: None,
        ..highlight()
    };
    let document = to_annotation(&mark, SOURCE);
    let position = document
        .target
        .selector
        .iter()
        .find_map(|s| match s {
            Selector::TextPosition { start, end, .. } => Some((*start, *end)),
            _ => None,
        })
        .unwrap();
    assert_eq!(position, (1200, 1200), "a point has no width");
    assert_eq!(round_trip(&mark).end, None);
}

/// Colour is not a `styleClass`, and the two must not be confused: one is a
/// value, the other names a stylesheet rule.
#[test]
fn colour_travels_as_its_own_member() {
    let document = to_annotation(&highlight(), SOURCE);
    assert_eq!(
        document.extra.get(&format!("{NS}:color")),
        Some(&Value::from("#ffcc00"))
    );
    let mut without = highlight();
    without.color = None;
    assert!(!to_annotation(&without, SOURCE)
        .extra
        .contains_key(&format!("{NS}:color")));
}

/// `motivation` is legally an array.
#[test]
fn a_motivation_array_still_names_the_kind() {
    let document: Annotation = serde_json::from_value(json!({
        "type": "Annotation",
        "motivation": ["bookmarking", "linking"],
        "target": {"source": SOURCE, "selector": []}
    }))
    .unwrap();
    assert_eq!(from_annotation(&document).kind, AnnotationKind::Bookmark);
}

#[test]
fn an_extra_map_is_only_written_when_it_has_something_in_it() {
    let mut bare = highlight();
    bare.color = None;
    let json = serde_json::to_value(to_annotation(&bare, SOURCE)).unwrap();
    let object = json.as_object().unwrap();
    assert!(!object.contains_key("extra"), "flatten leaked a field name");
    let _: BTreeMap<String, Value> = BTreeMap::new();
}
