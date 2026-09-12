//! Positions inside a book, and what keeps them stable.
//!
//! Design rationale and history: `docs/LOCATORS.md`. Summary: a bare
//! `char_offset` survives relayout (via the layout char_map) but not chapbook
//! releases or replaced files, so every stored position/annotation endpoint
//! carries the layered [`LayeredLocator`] record and is resolved through a
//! fallback chain that degrades to "right page-ish", never "gone".
//!
//! # Normative: the locator text and `char_offset` (version 2)
//!
//! `char_offset` indexes into the **locator text** of a spine item, produced
//! by `chapbook_layout::dom::locator_text`:
//!
//! - **Unit:** Unicode scalar values (Rust `char` count), not bytes, not
//!   UTF-16 code units.
//! - **What counts:** the content of text nodes of the post-parse tree, in
//!   document order, concatenated with nothing added between them. The
//!   post-parse tree includes the MathML fallback rewrite (version 2):
//!   `<math altimg>` becomes an `<img>` (contributing no text),
//!   `<math alttext>` contributes exactly the alttext, and a `<math>` with
//!   neither contributes its token text minus `annotation`/`annotation-xml`
//!   subtrees.
//! - **Excluded:** entire subtrees of `head`, `script`, `style`, and
//!   `template` elements. Generated (`::before`/`::after`) content, `alt`
//!   text, and markup never count.
//! - **Raw, not collapsed:** text-node content is indexed exactly as parsed
//!   (after entity expansion), including source newlines and indentation.
//!   Whitespace collapsing is layout's concern; offsets are independent of
//!   `white-space` handling and collapsing-rule changes.
//!
//! Any change to the above — including the planned exclusion of
//! `display:none` subtrees once the cascade exists — MUST bump
//! [`LOCATOR_VERSION`]. The golden offset-map test in chapbook-layout exists so
//! the bump is a conscious act, not an accident.
//!
//! # Progression units are per-format
//!
//! `char_offset` and the progression denominators count the format's
//! **progression unit**:
//!
//! - **Reflowable text (EPUB):** Unicode scalars of the locator text, as
//!   specified above.
//! - **Image-per-page formats (future CBZ):** the page is the unit — each
//!   spine item is one page and `char_offset` is always `0`, which makes the
//!   within-unit layers (offset, quote, `spine_fraction`) trivially
//!   degenerate: any resolve correctly lands at offset `0`, and
//!   [`resolve_in_text`] has nothing useful to add. Position identity is
//!   carried entirely by `spine_href`/`spine_index`, with
//!   [`book_progression`] computed as `prior_chars = spine_index`,
//!   `total_chars = page count`. Re-anchoring across editions whose page
//!   counts differ is a library-level concern that will use
//!   `book_progression` (nearest page), not the quote chain. Note the
//!   page-*start* convention caps progression at `(n-1)/n` on the last page:
//!   `1.0` is unreachable without an explicit "finished" state, which is a
//!   sync/library concern, not a locator one.
//!
//! Mixing units across formats is fine because progression is only ever
//! compared within one book.
//!
//! # The resolve chain
//!
//! 1. Same file + same [`LOCATOR_VERSION`] → `char_offset` directly.
//! 2. Same file, older version → re-find the quote nearest `spine_fraction`;
//!    on success rewrite the offset at the current version (self-healing
//!    migration).
//! 3. Different file/edition → quote search across the spine biased by
//!    `book_progression`; else `spine_href` + `spine_fraction`.
//! 4. Last resort → `book_progression`. Degrade to "right page-ish", never
//!    "gone".
//!
//! Steps 1–2 and the within-item half of step 4 are [`resolve_in_text`];
//! cross-file/edition orchestration (which spine items to try, edition
//! fingerprints) is chapbook-library's `restore` module.

/// Version of the locator-text extraction function. Stored alongside every
/// persisted position/annotation endpoint; see the module docs for what
/// constitutes a version bump.
///
/// History: 1 → 2 added the MathML altimg/alttext fallback rewrite to the
/// post-parse tree. 2 → 3 (kalam): content documents are parsed as XML
/// first, HTML as the fallback. The XML tree keeps the whitespace text
/// node between `<html>` and `<head>` that the HTML parser discards
/// (one `\n` at the front of the locator text of a typical chapter), and
/// nests self-closed `<a …/>`/`<span …/>` correctly where the HTML
/// algorithm left them open, so the tree behind every offset can differ.
/// Stored offsets at version 2 take the quote path (resolve chain step 2)
/// and heal themselves.
pub const LOCATOR_VERSION: u32 = 3;

/// Number of context characters captured on each side of a position for the
/// [`Quote`] layer.
pub const QUOTE_CONTEXT_CHARS: usize = 32;

/// A position inside a book, stable across relayout.
///
/// This is the light runtime type (layout char_maps, page turns). Anything
/// *persisted* should be a [`LayeredLocator`] captured from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Locator {
    /// Index into the EPUB spine (linear reading order).
    pub spine_index: usize,
    /// Offset into the spine item's locator text (see module docs).
    pub char_offset: u32,
}

impl Locator {
    pub fn new(spine_index: usize, char_offset: u32) -> Self {
        Locator {
            spine_index,
            char_offset,
        }
    }

    /// Start of a spine item.
    pub fn chapter_start(spine_index: usize) -> Self {
        Locator {
            spine_index,
            char_offset: 0,
        }
    }
}

/// Text-quote context around a position (W3C Web Annotation
/// `TextQuoteSelector` shape). For a point locator `exact` is empty and
/// `prefix`/`suffix` flank the point; for an annotation *range*, each
/// endpoint carries its own point-style quote.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Quote {
    pub prefix: String,
    pub exact: String,
    pub suffix: String,
}

/// The full layered position record persisted for every reading position and
/// annotation endpoint. Layers degrade gracefully: exact offset → quote →
/// spine fraction → whole-book progression. See the resolve chain in the
/// module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct LayeredLocator {
    /// Container-root path of the spine item — identity that survives spine
    /// reordering between editions better than the index alone.
    pub spine_href: String,
    pub spine_index: usize,
    /// Exact primary, valid only at `locator_version`.
    pub char_offset: u32,
    pub locator_version: u32,
    /// Context re-anchor layer; mandatory for annotation endpoints.
    pub quote: Quote,
    /// Proportion within this spine item's locator text, in `[0, 1]`.
    pub spine_fraction: f64,
    /// Proportion within the whole book's locator text, in `[0, 1]`.
    /// Denominator: total extracted chars across all spine items;
    /// numerator: extracted chars in prior spine items + `char_offset`.
    pub book_progression: f64,
}

/// Compute `book_progression` — defined here once so every producer computes
/// the same float. Character-count weighting is stable across font size,
/// margins, and relayout; synthetic page counts and file sizes are not.
pub fn book_progression(prior_chars: u64, char_offset: u32, total_chars: u64) -> f64 {
    if total_chars == 0 {
        return 0.0;
    }
    (prior_chars + u64::from(char_offset)) as f64 / total_chars as f64
}

impl LayeredLocator {
    /// Capture the full layered record for a point at `char_offset` within a
    /// spine item's locator `text`.
    ///
    /// `prior_chars`/`total_chars` are the book-wide locator-text char counts
    /// (before this spine item / across all spine items); both fall out of
    /// running the extraction over the spine once.
    pub fn capture(
        spine_href: &str,
        spine_index: usize,
        text: &str,
        char_offset: u32,
        prior_chars: u64,
        total_chars: u64,
    ) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let total = chars.len();
        let at = (char_offset as usize).min(total);
        let prefix: String = chars[at.saturating_sub(QUOTE_CONTEXT_CHARS)..at]
            .iter()
            .collect();
        let suffix: String = chars[at..(at + QUOTE_CONTEXT_CHARS).min(total)]
            .iter()
            .collect();
        LayeredLocator {
            spine_href: spine_href.to_string(),
            spine_index,
            char_offset: at as u32,
            locator_version: LOCATOR_VERSION,
            quote: Quote {
                prefix,
                exact: String::new(),
                suffix,
            },
            spine_fraction: if total == 0 {
                0.0
            } else {
                at as f64 / total as f64
            },
            book_progression: book_progression(prior_chars, at as u32, total_chars),
        }
    }
}

/// How a stored position was recovered; the tier tells the caller whether to
/// rewrite the record (self-healing migration) and how much trust to place in
/// the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedOffset {
    /// Same locator version, offset in range: used directly.
    Exact(u32),
    /// Re-found via the quote layer; caller should rewrite the stored record
    /// at the current [`LOCATOR_VERSION`] with this offset.
    Quote(u32),
    /// Positioned by `spine_fraction` only — approximate; also rewrite.
    Fraction(u32),
}

impl ResolvedOffset {
    pub fn offset(&self) -> u32 {
        match *self {
            ResolvedOffset::Exact(o) | ResolvedOffset::Quote(o) | ResolvedOffset::Fraction(o) => o,
        }
    }

    /// True when the stored record should be rewritten with the recovered
    /// offset at the current version.
    pub fn needs_rewrite(&self) -> bool {
        !matches!(self, ResolvedOffset::Exact(_))
    }
}

/// Resolve a layered locator against the locator `text` of a spine item —
/// steps 1, 2, and the within-item part of step 4 of the resolve chain in
/// the module docs. Cross-file/edition orchestration (which spine items to
/// try, edition fingerprints) belongs to chapbook-library.
///
/// `same_source` is the caller's knowledge of whether `text` came from the
/// same file the locator was captured against; only then is the exact offset
/// trusted.
pub fn resolve_in_text(text: &str, loc: &LayeredLocator, same_source: bool) -> ResolvedOffset {
    let total = text.chars().count();
    if same_source && loc.locator_version == LOCATOR_VERSION && (loc.char_offset as usize) <= total
    {
        return ResolvedOffset::Exact(loc.char_offset);
    }
    if let Some(offset) = find_quote_nearest(text, &loc.quote, loc.spine_fraction) {
        return ResolvedOffset::Quote(offset);
    }
    let approx = (loc.spine_fraction * total as f64).round() as u32;
    ResolvedOffset::Fraction(approx.min(total as u32))
}

/// Find the char offset of the point a [`Quote`] marks, choosing the
/// occurrence nearest `fraction` when the context appears more than once.
/// The needle is `prefix + exact + suffix` and the recovered point sits after
/// `prefix + exact` — for the point-style quotes [`LayeredLocator::capture`]
/// produces (`exact` empty), that is simply the seam between prefix and
/// suffix.
pub fn find_quote_nearest(text: &str, quote: &Quote, fraction: f64) -> Option<u32> {
    let needle = format!("{}{}{}", quote.prefix, quote.exact, quote.suffix);
    if needle.is_empty() {
        return None;
    }
    let point_chars_into_needle = quote.prefix.chars().count() + quote.exact.chars().count();
    let total = text.chars().count();
    let target = fraction * total as f64;

    let mut best: Option<(f64, u32)> = None;
    let mut chars_before = 0usize;
    let mut last_byte = 0usize;
    for (byte_idx, _) in text.match_indices(&needle) {
        // Convert byte index to char offset incrementally (matches are in
        // ascending byte order).
        chars_before += text[last_byte..byte_idx].chars().count();
        last_byte = byte_idx;
        let point = (chars_before + point_chars_into_needle) as u32;
        let distance = (f64::from(point) - target).abs();
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, point));
        }
    }
    best.map(|(_, offset)| offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "It was a truth universally acknowledged that a single reader \
                        in possession of a good book must be in want of a bookmark.";

    fn capture_at(offset: u32) -> LayeredLocator {
        LayeredLocator::capture("OEBPS/ch1.xhtml", 0, TEXT, offset, 0, TEXT.len() as u64)
    }

    #[test]
    fn capture_builds_all_layers() {
        let loc = capture_at(40);
        assert_eq!(loc.char_offset, 40);
        assert_eq!(loc.locator_version, LOCATOR_VERSION);
        assert_eq!(loc.quote.prefix.chars().count(), 32);
        assert_eq!(loc.quote.suffix.chars().count(), 32);
        assert!(loc.quote.exact.is_empty());
        assert!((loc.spine_fraction - 40.0 / TEXT.chars().count() as f64).abs() < 1e-9);
    }

    #[test]
    fn exact_when_same_source_and_version() {
        let loc = capture_at(40);
        assert_eq!(resolve_in_text(TEXT, &loc, true), ResolvedOffset::Exact(40));
    }

    #[test]
    fn quote_refind_survives_prepended_text() {
        let loc = capture_at(40);
        // A new edition adds a preface sentence: exact offsets shift.
        let drifted = format!("A preface was added in this edition. {TEXT}");
        let resolved = resolve_in_text(&drifted, &loc, false);
        let shift = "A preface was added in this edition. ".chars().count() as u32;
        assert_eq!(resolved, ResolvedOffset::Quote(40 + shift));
        assert!(resolved.needs_rewrite());
    }

    #[test]
    fn stale_version_refinds_even_in_same_source() {
        let mut loc = capture_at(40);
        loc.locator_version = 0;
        assert_eq!(resolve_in_text(TEXT, &loc, true), ResolvedOffset::Quote(40));
    }

    #[test]
    fn repeated_context_disambiguated_by_fraction() {
        let repeated = "the cat sat. the cat sat. the cat sat.";
        let quote = Quote {
            prefix: "the ".into(),
            exact: String::new(),
            suffix: "cat".into(),
        };
        // Point after "the " of each occurrence: 4, 17, 30.
        assert_eq!(find_quote_nearest(repeated, &quote, 0.0), Some(4));
        assert_eq!(find_quote_nearest(repeated, &quote, 0.45), Some(17));
        assert_eq!(find_quote_nearest(repeated, &quote, 1.0), Some(30));
    }

    #[test]
    fn fraction_fallback_when_quote_gone() {
        let loc = capture_at(40);
        let unrelated = "Completely different text of some length, nothing shared.";
        let resolved = resolve_in_text(unrelated, &loc, false);
        assert!(matches!(resolved, ResolvedOffset::Fraction(_)));
        assert!((resolved.offset() as usize) <= unrelated.chars().count());
    }

    #[test]
    fn empty_text_is_safe() {
        let loc = capture_at(0);
        assert_eq!(resolve_in_text("", &loc, true), ResolvedOffset::Exact(0));
        let mut stale = loc;
        stale.locator_version = 0;
        assert_eq!(
            resolve_in_text("", &stale, true),
            ResolvedOffset::Fraction(0)
        );
    }

    #[test]
    fn progression_formula() {
        assert_eq!(book_progression(0, 0, 100), 0.0);
        assert_eq!(book_progression(50, 25, 100), 0.75);
        assert_eq!(book_progression(0, 0, 0), 0.0);
    }

    #[test]
    fn multibyte_text_counts_scalars_not_bytes() {
        let text = "naïve café — “curly” ligature-friendly affluence";
        let loc = LayeredLocator::capture("h", 0, text, 10, 0, text.chars().count() as u64);
        assert_eq!(resolve_in_text(text, &loc, true), ResolvedOffset::Exact(10));
        let drifted = format!("Vorwort: {text}");
        let resolved = resolve_in_text(&drifted, &loc, false);
        assert_eq!(resolved.offset(), 10 + "Vorwort: ".chars().count() as u32);
    }
}
