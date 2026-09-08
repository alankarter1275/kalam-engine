//! The Web Annotation Data Model, as much of it as a reading app needs.
//!
//! Hand-written types rather than a generic JSON-LD layer: the profile
//! chapbook writes is narrow and the profile it must *survive* reading is
//! wide, and those want opposite things. What we write is exact; what we
//! read keeps whatever we did not model, so a round trip through this
//! crate never destroys another client's data.
//!
//! That last part is the load-bearing decision. A sync client that drops
//! fields it does not understand is a sync client that quietly deletes the
//! other app's annotations the first time it touches one — and the damage
//! looks like the server's fault. Every type here carries an `extra` map
//! for the members it did not name, flattened back out on serialization.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The JSON-LD context every Web Annotation carries.
pub const CONTEXT: &str = "http://www.w3.org/ns/anno.jsonld";

/// The media type for an annotation and for a container.
pub const MEDIA_TYPE: &str = "application/ld+json; profile=\"http://www.w3.org/ns/anno.jsonld\"";

/// One annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    #[serde(rename = "@context", default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    /// The server-minted IRI. Absent on a document being created — the
    /// container mints it and returns it in `Location`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default = "annotation_type")]
    pub kind: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motivation: Option<Value>,
    /// `body` proper — a `TextualBody` for a note, absent for a bare
    /// highlight or bookmark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// The shorthand form, which is what a plain string body is.
    #[serde(rename = "bodyValue", default, skip_serializing_if = "Option::is_none")]
    pub body_value: Option<String>,
    pub target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<Value>,
    /// Everything else the document carried. See the module docs: this is
    /// what stops a sync from deleting another client's fields.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

fn annotation_type() -> Value {
    Value::String("Annotation".into())
}

/// What the annotation is about.
///
/// Always the `SpecificResource` form here, because chapbook's anchor is a
/// selector stack and a bare IRI cannot carry one. `type` is written
/// explicitly even though the specification's own examples omit it: a
/// server that infers it is doing the reader a favour, and one that does
/// not used to drop the whole target on the floor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    #[serde(rename = "type", default = "specific_resource")]
    pub kind: Value,
    /// The publication this anchors into.
    pub source: String,
    /// The model allows one selector or an array of them, and real
    /// servers use both — webanno serializes a lone selector as a bare
    /// object. Read either; always write the array. Found live: every
    /// single-selector mark in a container parsed as nothing, and a
    /// listing full of them looked exactly like an empty container.
    #[serde(
        default,
        deserialize_with = "one_or_many",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub selector: Vec<Selector>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

/// `selector` as the model defines it: a single object stands for a
/// stack of one. Branching on the JSON shape rather than an untagged
/// enum, because [`Selector::Other`] would happily swallow an array.
fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<Selector>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let items = match value {
        Value::Array(items) => items,
        Value::Null => Vec::new(),
        one => vec![one],
    };
    items
        .into_iter()
        .map(|item| serde_json::from_value(item).map_err(serde::de::Error::custom))
        .collect()
}

fn specific_resource() -> Value {
    Value::String("SpecificResource".into())
}

/// One layer of the anchor.
///
/// Untagged-by-hand rather than `#[serde(tag = "type")]`: an unknown
/// selector type has to survive the round trip as [`Selector::Other`]
/// rather than fail the whole document, and a tagged enum cannot express
/// that without a catch-all that eats the known variants' errors too.
#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    /// The quote layer: the text and what surrounds it.
    TextQuote {
        exact: String,
        prefix: Option<String>,
        suffix: Option<String>,
        extra: BTreeMap<String, Value>,
    },
    /// Character offsets into the source's text.
    TextPosition {
        start: u64,
        end: u64,
        extra: BTreeMap<String, Value>,
    },
    /// A fragment identifier — an EPUB CFI, a page number, an element id.
    Fragment {
        value: String,
        conforms_to: Option<String>,
        extra: BTreeMap<String, Value>,
    },
    /// Readium's whole-publication fraction. Not a W3C selector type; it
    /// is what the Readium profile writes and what makes a position
    /// portable when nothing else resolves.
    Progress {
        value: f64,
        extra: BTreeMap<String, Value>,
    },
    /// A selector this crate does not model, kept verbatim.
    Other(Value),
}

/// `conformsTo` for an EPUB CFI fragment selector.
pub const CFI_CONFORMS_TO: &str = "http://www.idpf.org/epub/linking/cfi/epub-cfi.html";

impl Selector {
    /// The `type` member, for a caller choosing among a stack.
    pub fn type_name(&self) -> Option<&str> {
        match self {
            Selector::TextQuote { .. } => Some("TextQuoteSelector"),
            Selector::TextPosition { .. } => Some("TextPositionSelector"),
            Selector::Fragment { .. } => Some("FragmentSelector"),
            Selector::Progress { .. } => Some("ProgressSelector"),
            Selector::Other(value) => value.get("type").and_then(Value::as_str),
        }
    }
}

impl Serialize for Selector {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = match self {
            Selector::TextQuote {
                exact,
                prefix,
                suffix,
                extra,
            } => {
                let mut map = serde_json::Map::new();
                map.insert("type".into(), "TextQuoteSelector".into());
                map.insert("exact".into(), exact.clone().into());
                if let Some(prefix) = prefix {
                    map.insert("prefix".into(), prefix.clone().into());
                }
                if let Some(suffix) = suffix {
                    map.insert("suffix".into(), suffix.clone().into());
                }
                merge(&mut map, extra);
                Value::Object(map)
            }
            Selector::TextPosition { start, end, extra } => {
                let mut map = serde_json::Map::new();
                map.insert("type".into(), "TextPositionSelector".into());
                map.insert("start".into(), (*start).into());
                map.insert("end".into(), (*end).into());
                merge(&mut map, extra);
                Value::Object(map)
            }
            Selector::Fragment {
                value,
                conforms_to,
                extra,
            } => {
                let mut map = serde_json::Map::new();
                map.insert("type".into(), "FragmentSelector".into());
                map.insert("value".into(), value.clone().into());
                if let Some(conforms_to) = conforms_to {
                    map.insert("conformsTo".into(), conforms_to.clone().into());
                }
                merge(&mut map, extra);
                Value::Object(map)
            }
            Selector::Progress { value, extra } => {
                let mut map = serde_json::Map::new();
                map.insert("type".into(), "ProgressSelector".into());
                map.insert("value".into(), (*value).into());
                merge(&mut map, extra);
                Value::Object(map)
            }
            Selector::Other(value) => value.clone(),
        };
        value.serialize(serializer)
    }
}

fn merge(map: &mut serde_json::Map<String, Value>, extra: &BTreeMap<String, Value>) {
    for (key, value) in extra {
        map.insert(key.clone(), value.clone());
    }
}

impl<'de> Deserialize<'de> for Selector {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let Some(object) = value.as_object() else {
            return Ok(Selector::Other(value));
        };
        let type_name = object.get("type").and_then(Value::as_str).unwrap_or("");
        // `take_rest` is what keeps an unmodelled member — a
        // `chapbook:locatorVersion`, another client's annotation-tool id —
        // alive across the round trip.
        let rest = |named: &[&str]| -> BTreeMap<String, Value> {
            object
                .iter()
                .filter(|(k, _)| !named.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        Ok(match type_name {
            "TextQuoteSelector" => match object.get("exact").and_then(Value::as_str) {
                Some(exact) => Selector::TextQuote {
                    exact: exact.to_string(),
                    prefix: object
                        .get("prefix")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    suffix: object
                        .get("suffix")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    extra: rest(&["type", "exact", "prefix", "suffix"]),
                },
                // A quote selector with no `exact` is not one. Keeping it
                // verbatim beats inventing an empty quote that would then
                // match at offset zero.
                None => Selector::Other(value.clone()),
            },
            "TextPositionSelector" => {
                match (
                    object.get("start").and_then(Value::as_u64),
                    object.get("end").and_then(Value::as_u64),
                ) {
                    (Some(start), Some(end)) => Selector::TextPosition {
                        start,
                        end,
                        extra: rest(&["type", "start", "end"]),
                    },
                    _ => Selector::Other(value.clone()),
                }
            }
            "FragmentSelector" => match object.get("value").and_then(Value::as_str) {
                Some(fragment) => Selector::Fragment {
                    value: fragment.to_string(),
                    conforms_to: object
                        .get("conformsTo")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    extra: rest(&["type", "value", "conformsTo"]),
                },
                None => Selector::Other(value.clone()),
            },
            // Readium has written both spellings.
            "ProgressSelector" | "ProgressionSelector" => {
                match object.get("value").and_then(Value::as_f64) {
                    Some(fraction) => Selector::Progress {
                        value: fraction,
                        extra: rest(&["type", "value"]),
                    },
                    None => Selector::Other(value.clone()),
                }
            }
            _ => Selector::Other(value.clone()),
        })
    }
}
