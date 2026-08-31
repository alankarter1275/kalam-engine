//! chapbook's stored annotation onto a Web Annotation, and back.
//!
//! # The selector stack is the layered locator
//!
//! docs/LOCATORS.md says the layered record was shaped to serialize into
//! this format field-for-field. Concretely, one endpoint becomes three
//! selectors, written most-durable-first:
//!
//! | layer | selector |
//! |---|---|
//! | `quote` | `TextQuoteSelector` — `exact`, `prefix`, `suffix` |
//! | `char_offset` | `TextPositionSelector` — with `chapbook:locatorVersion` |
//! | `book_progression` | `ProgressSelector` — Readium's fraction |
//!
//! and the spine item rides on the target as `chapbook:spineHref` /
//! `chapbook:spineIndex`, because the `source` is the *publication* — that
//! is what another client will have opened, and what the catalog entry's
//! annotation-service link is scoped to.
//!
//! **`TextPositionSelector` is written with its version and read with
//! suspicion.** A raw offset is only meaningful against one extraction of
//! one edition, so it travels stamped with `chapbook:locatorVersion` and is
//! only believed back when that stamp matches this build. A peer's
//! unstamped position selector is another app's offsets into text we did
//! not extract; taking it would silently mark the wrong words, which is the
//! exact failure the quote layer exists to prevent. It is kept in `extra`,
//! not obeyed.
//!
//! # A range is two endpoints, and W3C has one selector stack
//!
//! A highlight has a start and an end. The data model's `TextQuoteSelector`
//! covers a *run* — `exact` is the whole quoted text — so the range's own
//! shape lives there naturally, and the endpoint locators go alongside as
//! `chapbook:start` / `chapbook:end` position selectors. Another client
//! reads the quote and gets the right words; chapbook reads its own members
//! and gets its exact offsets back.

use std::collections::BTreeMap;

use chapbook_core::{LayeredLocator, Quote, LOCATOR_VERSION};
use chapbook_library::AnnotationKind;
use serde_json::Value;

use crate::model::{Annotation, Selector, Target, CONTEXT};

/// Members chapbook writes that the data model does not define. Namespaced
/// so another client can see whose they are and leave them alone — which
/// is the same courtesy this crate extends to theirs.
pub const NS: &str = "chapbook";

/// A mark as chapbook holds it, independent of the library row.
///
/// Not `chapbook_library::Annotation`: that carries a local `id` and
/// `created_at`/`updated_at` as unix integers, which are storage details.
/// This is the same fact in the shape the wire wants.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub kind: AnnotationKind,
    pub start: LayeredLocator,
    /// Ranged kinds have an end; a bookmark is a point.
    pub end: Option<LayeredLocator>,
    /// The note's text. A highlight's quoted words are in the locator, not
    /// here.
    pub text: Option<String>,
    /// `#rrggbb` as written, or `None` to follow the reader's theme.
    pub color: Option<String>,
    /// ISO 8601, the host's to supply.
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// The motivation each kind is written with.
fn motivation(kind: AnnotationKind) -> &'static str {
    match kind {
        // W3C defines all three; a bookmark really is `bookmarking`.
        AnnotationKind::Bookmark => "bookmarking",
        AnnotationKind::Highlight => "highlighting",
        AnnotationKind::Note => "commenting",
    }
}

fn kind_of(motivation: Option<&str>, has_text: bool) -> AnnotationKind {
    match motivation {
        Some("bookmarking") => AnnotationKind::Bookmark,
        Some("commenting") | Some("replying") => AnnotationKind::Note,
        Some("highlighting") => AnnotationKind::Highlight,
        // No motivation is legal. A body means somebody wrote something.
        _ if has_text => AnnotationKind::Note,
        _ => AnnotationKind::Highlight,
    }
}

/// Serialize a mark as an annotation against `source`, the publication IRI
/// the container is scoped to.
pub fn to_annotation(mark: &Mark, source: &str) -> Annotation {
    let mut selectors = Vec::new();

    // The quote layer. For a range, `exact` is the quoted run, which for a
    // highlight is exactly what the start locator's `exact` holds.
    let quote = &mark.start.quote;
    if !quote.exact.is_empty() || !quote.prefix.is_empty() || !quote.suffix.is_empty() {
        selectors.push(Selector::TextQuote {
            exact: quote.exact.clone(),
            prefix: non_empty(&quote.prefix),
            suffix: non_empty(&quote.suffix),
            extra: BTreeMap::new(),
        });
    }

    // The offset layer, stamped with the version that makes it meaningful.
    let end_offset = mark
        .end
        .as_ref()
        .map(|e| u64::from(e.char_offset))
        .unwrap_or_else(|| u64::from(mark.start.char_offset));
    let mut position_extra = BTreeMap::new();
    position_extra.insert(
        format!("{NS}:locatorVersion"),
        Value::from(mark.start.locator_version),
    );
    selectors.push(Selector::TextPosition {
        start: u64::from(mark.start.char_offset),
        end: end_offset,
        extra: position_extra,
    });

    // The fraction, which is the layer that survives a different edition.
    selectors.push(Selector::Progress {
        value: mark.start.book_progression,
        extra: BTreeMap::new(),
    });

    let mut target_extra = BTreeMap::new();
    target_extra.insert(
        format!("{NS}:spineHref"),
        Value::from(mark.start.spine_href.clone()),
    );
    target_extra.insert(
        format!("{NS}:spineIndex"),
        Value::from(mark.start.spine_index as u64),
    );
    target_extra.insert(
        format!("{NS}:spineFraction"),
        Value::from(mark.start.spine_fraction),
    );
    if let Some(end) = &mark.end {
        // The end endpoint in full, so a chapbook on the other side gets
        // its own layers back rather than re-deriving them from a quote.
        target_extra.insert(format!("{NS}:end"), locator_value(end));
    }

    let mut extra = BTreeMap::new();
    if let Some(color) = &mark.color {
        // `oa:styleClass` is for a class name, not a value, so the colour
        // goes in a namespaced member rather than being bent into one.
        extra.insert(format!("{NS}:color"), Value::from(color.clone()));
    }

    Annotation {
        context: Some(Value::from(CONTEXT)),
        id: None,
        kind: Value::from("Annotation"),
        motivation: Some(Value::from(motivation(mark.kind))),
        body: None,
        body_value: mark.text.clone(),
        target: Target {
            kind: Value::from("SpecificResource"),
            source: source.to_string(),
            selector: selectors,
            extra: target_extra,
        },
        created: mark.created.clone(),
        modified: mark.modified.clone(),
        creator: None,
        extra,
    }
}

/// Read an annotation as a mark.
///
/// Never fails: an annotation from another client may carry none of
/// chapbook's members and still be perfectly usable through the quote and
/// progress layers, which is the whole point of the stack. What cannot be
/// recovered degrades rather than erroring.
pub fn from_annotation(annotation: &Annotation) -> Mark {
    let target = &annotation.target;

    let quote = target
        .selector
        .iter()
        .find_map(|s| match s {
            Selector::TextQuote {
                exact,
                prefix,
                suffix,
                ..
            } => Some(Quote {
                prefix: prefix.clone().unwrap_or_default(),
                exact: exact.clone(),
                suffix: suffix.clone().unwrap_or_default(),
            }),
            _ => None,
        })
        .unwrap_or_default();

    let progression = target
        .selector
        .iter()
        .find_map(|s| match s {
            Selector::Progress { value, .. } => Some(*value),
            _ => None,
        })
        .unwrap_or(0.0);

    // Offsets are only believed when the peer stamped them with the
    // version this build extracts at. See the module docs.
    let position = target.selector.iter().find_map(|s| match s {
        Selector::TextPosition { start, end, extra } => {
            let version = extra.get(&format!("{NS}:locatorVersion"))?.as_u64()?;
            (version == u64::from(LOCATOR_VERSION)).then_some((*start, *end))
        }
        _ => None,
    });

    let spine_href = target
        .extra
        .get(&format!("{NS}:spineHref"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let spine_index = target
        .extra
        .get(&format!("{NS}:spineIndex"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let spine_fraction = target
        .extra
        .get(&format!("{NS}:spineFraction"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);

    let start = LayeredLocator {
        spine_href,
        spine_index,
        char_offset: position.map(|(start, _)| start as u32).unwrap_or(0),
        // Claiming this build's version for an offset we did not take is
        // how a drifted highlight becomes a confident wrong one. Without a
        // matching stamp the record says so, and the resolve chain falls
        // through to the quote.
        locator_version: if position.is_some() {
            LOCATOR_VERSION
        } else {
            0
        },
        quote,
        spine_fraction,
        book_progression: progression,
    };

    let end = target
        .extra
        .get(&format!("{NS}:end"))
        .and_then(|value| locator_from_value(value, &start))
        .or_else(|| {
            position.and_then(|(s, e)| {
                (e > s).then(|| LayeredLocator {
                    char_offset: e as u32,
                    ..start.clone()
                })
            })
        });

    let text = annotation.body_value.clone().or_else(|| {
        annotation
            .body
            .as_ref()
            .and_then(|b| b.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });

    Mark {
        kind: kind_of(
            annotation.motivation.as_ref().and_then(|m| match m {
                Value::String(s) => Some(s.as_str()),
                // `motivation` may be an array; the first is enough.
                Value::Array(items) => items.first().and_then(Value::as_str),
                _ => None,
            }),
            text.is_some(),
        ),
        start,
        end,
        text,
        color: annotation
            .extra
            .get(&format!("{NS}:color"))
            .and_then(Value::as_str)
            .map(str::to_string),
        created: annotation.created.clone(),
        modified: annotation.modified.clone(),
    }
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn locator_value(loc: &LayeredLocator) -> Value {
    serde_json::json!({
        "charOffset": loc.char_offset,
        "locatorVersion": loc.locator_version,
        "spineHref": loc.spine_href,
        "spineIndex": loc.spine_index,
        "spineFraction": loc.spine_fraction,
        "bookProgression": loc.book_progression,
        "quote": {
            "prefix": loc.quote.prefix,
            "exact": loc.quote.exact,
            "suffix": loc.quote.suffix,
        },
    })
}

/// The inverse, falling back to `start`'s spine for anything absent.
fn locator_from_value(value: &Value, start: &LayeredLocator) -> Option<LayeredLocator> {
    let object = value.as_object()?;
    let version = object
        .get("locatorVersion")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    Some(LayeredLocator {
        spine_href: object
            .get("spineHref")
            .and_then(Value::as_str)
            .unwrap_or(&start.spine_href)
            .to_string(),
        spine_index: object
            .get("spineIndex")
            .and_then(Value::as_u64)
            .unwrap_or(start.spine_index as u64) as usize,
        char_offset: object
            .get("charOffset")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        locator_version: if version == LOCATOR_VERSION {
            version
        } else {
            0
        },
        quote: object
            .get("quote")
            .and_then(Value::as_object)
            .map(|q| Quote {
                prefix: q
                    .get("prefix")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                exact: q
                    .get("exact")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                suffix: q
                    .get("suffix")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .unwrap_or_default(),
        spine_fraction: object
            .get("spineFraction")
            .and_then(Value::as_f64)
            .unwrap_or(start.spine_fraction),
        book_progression: object
            .get("bookProgression")
            .and_then(Value::as_f64)
            .unwrap_or(start.book_progression),
    })
}
