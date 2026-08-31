//! The join between chapbook's [`LayeredLocator`] and an OPDS Progression
//! 1.0 document: what a reading position looks like on the wire, and what
//! another device's position looks like coming back.
//!
//! Behind the non-default `progression` feature, for the reason
//! `opds_client::progression` gives: the spec is an unreleased draft.
//!
//! # What crosses and what does not
//!
//! The document carries two things chapbook can use. `progression` is the
//! whole-book fraction, which every client can act on and which
//! [`LayeredLocator::book_progression`] already is — same definition,
//! character-weighted, so it needs no conversion. `references` are
//! advisory refinements, and the draft's own list includes a **text
//! fragment** (`chapter1.html#:~:text=…`) — which is the quote layer,
//! written in somebody else's syntax.
//!
//! What deliberately does not cross is `char_offset`. It is only valid at
//! one `locator_version` against one edition's extracted text, and the
//! device on the other end has neither. Sending it would invite a peer to
//! trust a number that means nothing to it. This is the same reason
//! docs/LOCATORS.md gives for the layers existing at all: text positions
//! never leave the device.
//!
//! # Why a point becomes `text=prefix-,suffix`
//!
//! A reading position is a point, and [`LayeredLocator::capture`] leaves
//! `quote.exact` empty for one — the context is the text on either side.
//! A text fragment cannot express an empty match, so the point is written
//! as the text *starting* at it, disambiguated by what precedes it:
//! `text=<prefix>-,<suffix>`. A Text Fragments consumer anchors at the
//! start of the match, which is exactly the point we meant. An annotation
//! endpoint, whose `exact` is the quoted run, writes the full
//! `text=<prefix>-,<exact>,-<suffix>`.

use chapbook_core::{LayeredLocator, Quote};

// The wire half, re-exported so this module is the whole of position sync
// from inside chapbook. It also has to be: the crate glob-re-exports
// `opds_client`, and a module declared here shadows the one that arrives
// by glob, so without this `chapbook_opds::progression::Progression` would
// silently not exist.
pub use opds_client::progression::{
    Device, Progression, ProgressionRefusal, ProgressionUpdate, RefusalReason,
    MEDIA_TYPE_PROGRESSION, REL_PROGRESSION,
};

/// Another device's position, reduced to what this one can actually
/// resolve.
///
/// Not a [`LayeredLocator`]: rebuilding one would mean inventing a
/// `char_offset`, a `locator_version` and a `spine_index` the document
/// never carried, and every one of those is a lie the resolve chain would
/// then trust. What comes back is a fraction, and — when the peer sent a
/// text-fragment reference — the spine item and quote to re-anchor
/// against. That is precisely the input `chapbook_core::find_quote_nearest`
/// takes.
#[derive(Debug, Clone, PartialEq)]
pub struct RemotePosition {
    /// Whole-book fraction; the layer that always survives.
    pub progression: f64,
    /// Spine item the reference named, if it named one.
    pub spine_href: Option<String>,
    /// The quote layer, recovered from a text-fragment reference. A point
    /// has an empty `exact` — the same shape `capture` produces.
    pub quote: Option<Quote>,
    /// The peer's human label for the point, for "last read on …".
    pub title: Option<String>,
    pub device: Device,
    /// ISO 8601, compared as written — the conflict rule's input.
    pub modified: String,
}

/// Serialize a position for the wire.
///
/// `modified` and `device` are the host's to supply: the crate has no
/// clock and no device identity (see `opds_client::progression::Device`).
pub fn to_progression(
    loc: &LayeredLocator,
    device: Device,
    modified: impl Into<String>,
    title: Option<String>,
) -> Progression {
    let mut references = Vec::new();
    if let Some(fragment) = text_fragment(&loc.quote) {
        references.push(format!("{}#{fragment}", loc.spine_href));
    }
    // The bare spine item, as the coarser refinement. The draft gives
    // `references` no ordering guarantee and tells a consumer to prefer
    // the most specific that resolves, so this costs nothing and gives a
    // peer that cannot do text fragments a chapter to open.
    references.push(loc.spine_href.clone());
    Progression {
        title,
        modified: modified.into(),
        device,
        progression: loc.book_progression,
        references,
    }
}

/// Read a peer's position, preferring the most specific reference that
/// parses. `progression` is always present, so this never fails.
pub fn from_progression(doc: &Progression) -> RemotePosition {
    let mut spine_href = None;
    let mut quote = None;
    for reference in &doc.references {
        let (path, fragment) = match reference.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (reference.as_str(), None),
        };
        let parsed = fragment.and_then(parse_text_fragment);
        // A reference with a usable quote wins outright; one without only
        // fills in a spine item nothing better has claimed.
        if parsed.is_some() {
            spine_href = non_empty(path);
            quote = parsed;
            break;
        }
        if spine_href.is_none() {
            spine_href = non_empty(path);
        }
    }
    RemotePosition {
        progression: doc.progression,
        spine_href,
        quote,
        title: doc.title.clone(),
        device: doc.device.clone(),
        modified: doc.modified.clone(),
    }
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Build the `:~:text=` directive for a quote, or `None` when there is no
/// anchoring text at all — a quote with neither `exact` nor `suffix`
/// cannot start a match, and an empty directive is worse than none.
fn text_fragment(quote: &Quote) -> Option<String> {
    let start = if quote.exact.is_empty() {
        &quote.suffix
    } else {
        &quote.exact
    };
    if start.is_empty() {
        return None;
    }
    let mut terms = Vec::new();
    if !quote.prefix.is_empty() {
        terms.push(format!("{}-", encode_term(&quote.prefix)));
    }
    terms.push(encode_term(start));
    // The suffix is the match's own text when the quote is a point, so it
    // must not also be written as the trailing `-suffix` context term.
    if !quote.exact.is_empty() && !quote.suffix.is_empty() {
        terms.push(format!("-{}", encode_term(&quote.suffix)));
    }
    Some(format!(":~:text={}", terms.join(",")))
}

/// Recover a quote from a fragment that may carry a text directive.
/// Returns `None` for a fragment with no `text=` directive — an id
/// fragment (`#par26`) names an element, not a run of text.
fn parse_text_fragment(fragment: &str) -> Option<Quote> {
    let directives = fragment.strip_prefix(":~:")?;
    let directive = directives
        .split('&')
        .find_map(|d| d.strip_prefix("text="))?;

    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut middle: Vec<String> = Vec::new();
    let terms: Vec<&str> = directive.split(',').collect();
    for (i, term) in terms.iter().enumerate() {
        let first = i == 0;
        let last = i + 1 == terms.len();
        if first && term.ends_with('-') && terms.len() > 1 {
            prefix = decode_term(&term[..term.len() - 1]);
        } else if last && term.starts_with('-') && !middle.is_empty() {
            suffix = decode_term(&term[1..]);
        } else {
            middle.push(decode_term(term));
        }
    }
    if middle.is_empty() {
        return None;
    }
    // `text=start,end` is a range whose endpoints are elided text; the
    // start is the anchor, which is all a re-anchor needs.
    let exact = middle.remove(0);
    if exact.is_empty() {
        return None;
    }
    Some(Quote {
        prefix,
        exact,
        suffix,
    })
}

/// Percent-encode a text-fragment term. The syntax gives `,`, `-` and `&`
/// meaning, so they cannot travel raw; everything outside the unreserved
/// set is encoded rather than reasoned about.
fn encode_term(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' => out.push(byte as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode_term(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = hex_pair(bytes[i + 1], bytes[i + 2]) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        // A `+` is a form-encoding habit, not this syntax; leave it alone.
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(hi: u8, lo: u8) -> Option<u8> {
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    Some(nibble(hi)? << 4 | nibble(lo)?)
}
