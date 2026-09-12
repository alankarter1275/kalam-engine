//! Native MathML Core rendering: formulary layouts, prepared per chapter.
//!
//! The DOM already took its EPUB fallback rewrite (`dom::math_fallback`), so
//! locator text and anchors are settled and identical on builds without this
//! module. What happens here is rendering only: each captured block-mode
//! `<math>` source is parsed and laid out by formulary against a MATH-table
//! font from the session's own fontdb, and the box tree turns any node with
//! a prepared layout into a replaced block instead of its fallback form.
//! At paginate time the formula lowers to an ordinary [`chapbook_paint`]
//! line fragment — glyph runs on the math font plus rules as decorations —
//! so the display list, both renderers, selection, and the FFI surface
//! never learn math exists.
//!
//! A formula stays on the fallback path when any of these hold, each a
//! deliberate policy:
//! - **Inline math** (`display` ≠ `block`): a replaced block mid-sentence
//!   reads worse than the fallback's flattened tokens.
//! - **Parse warnings with a publisher fallback present**: the publisher's
//!   altimg/alttext is more faithful than our `mrow` error recovery.
//!   Warnings without a fallback still render — better than token soup.
//! - **Arabic token text with a publisher fallback present**: formulary
//!   lays Arabic out logical-order and unshaped, so the publisher's
//!   fallback is the faithful rendering. Without one, unshaped still
//!   beats token soup.
//! - **Mirrored glyphs** (right-to-left math): a glyph run cannot express
//!   the horizontal flip, and drawing a radical backwards is worse than
//!   the fallback.
//! - **No MATH-table font in the fontdb** (the `mathml` feature embeds
//!   STIX Two Math precisely so this is rare).

use std::collections::HashMap;

use cosmic_text::{fontdb, FontSystem};
use formulary::{Item, LayoutOptions, MathFont};

use crate::dom::{node_tag, Document};

/// Formulas laid out and ready to lower, keyed by node tag.
#[derive(Default)]
pub(crate) struct MathStore(HashMap<u64, PreparedMath>);

impl MathStore {
    pub(crate) fn contains(&self, tag: u64) -> bool {
        self.0.contains_key(&tag)
    }

    pub(crate) fn get(&self, tag: u64) -> Option<&PreparedMath> {
        self.0.get(&tag)
    }
}

/// One laid-out formula. Coordinates are y-down relative to the formula's
/// baseline — the same convention as [`chapbook_paint::Glyph`].
#[derive(Clone)]
pub(crate) struct PreparedMath {
    /// The math face in the session fontdb (what the renderers rasterize).
    pub font: fontdb::ID,
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub items: Vec<Item>,
    /// Line-fragment text: the publisher's alttext when present.
    pub text: String,
    /// Locator-text offset of the formula (every glyph repeats it, so
    /// selection treats the formula as atomic).
    pub locator: u32,
}

/// Lay out every renderable captured `<math>` in the document. Runs per
/// paginate call: font size comes from the cascade, so a settings change
/// re-prepares naturally.
pub(crate) fn prepare(
    doc: &Document,
    fonts: &FontSystem,
    locator: &HashMap<crate::dom::NodeId, u32>,
) -> MathStore {
    let mut store = MathStore::default();
    let candidates: Vec<_> = doc
        .math_sources()
        .filter(|(_, source)| source.block)
        .collect();
    if candidates.is_empty() {
        return store;
    }
    // Parse and screen every candidate first, so the font scan and the
    // face open below run only when a formula will actually lay out.
    struct Screened {
        tag: u64,
        root: formulary::MathRoot,
        font_size: f32,
        text: String,
        locator: u32,
    }
    let mut screened = Vec::new();
    for (id, source) in candidates {
        let root = match formulary::parse(&source.xml) {
            Ok(root) => root,
            Err(err) => {
                log::warn!("MathML did not parse ({err}); using fallback");
                continue;
            }
        };
        if !root.warnings.is_empty() && source.has_fallback {
            log::info!(
                "MathML has unsupported structure ({:?}); preferring the \
                 publisher's fallback",
                root.warnings.first()
            );
            continue;
        }
        if source.has_fallback && root.has_arabic_text() {
            log::info!(
                "MathML carries Arabic token text (formulary would lay it \
                 unshaped); preferring the publisher's fallback"
            );
            continue;
        }
        let Some(style) = doc.primary_styles(id) else {
            continue;
        };
        screened.push(Screened {
            tag: node_tag(id),
            root,
            font_size: crate::style_to_attrs::font_size_px(&style),
            text: source.alttext.clone().unwrap_or_default(),
            locator: locator.get(&id).copied().unwrap_or(0),
        });
    }
    if screened.is_empty() {
        return store;
    }
    let Some(font_id) = math_font(fonts.db()) else {
        log::warn!(
            "document has block MathML but no loaded font carries an \
             OpenType MATH table; falling back to altimg/alttext"
        );
        return store;
    };

    // One face open for the whole document. `with_face_data` on a
    // file-backed face re-reads the file every call, and `MathFont::new`
    // re-parses the MATH table — per-formula would make both O(formulas).
    let prepared = fonts.db().with_face_data(font_id, |data, index| {
        let font = MathFont::new(data, index).ok()?;
        let mut out = Vec::with_capacity(screened.len());
        for c in screened {
            let laid = formulary::layout(
                &c.root,
                &font,
                &LayoutOptions {
                    font_size: c.font_size,
                },
            );
            // Right-to-left math: a glyph run cannot mirror; this formula
            // falls back, the rest still render.
            let mirrored = laid
                .items
                .iter()
                .any(|item| matches!(item, Item::Glyph { mirrored: true, .. }));
            if mirrored {
                continue;
            }
            out.push((
                c.tag,
                PreparedMath {
                    font: font_id,
                    width: laid.width,
                    ascent: laid.ascent,
                    descent: laid.descent,
                    items: laid.items,
                    text: c.text,
                    locator: c.locator,
                },
            ));
        }
        Some(out)
    });
    if let Some(Some(prepared)) = prepared {
        store.0.extend(prepared);
    }
    store
}

/// The math face: the last-loaded face with a usable MATH table. Reverse
/// order is the preference order — a publisher's `@font-face` math font
/// registers after the source faces and the embedded STIX Two Math, and
/// the embedded face (loaded at the end of `build_font_system`) beats any
/// math-capable host font that happened into the source.
/// `MathFont::probe` reads only the table directory, cheap enough that
/// scanning the whole list needs no cache.
fn math_font(db: &fontdb::Database) -> Option<fontdb::ID> {
    let ids: Vec<fontdb::ID> = db.faces().map(|f| f.id).collect();
    ids.into_iter()
        .rev()
        .find(|id| db.with_face_data(*id, MathFont::probe).unwrap_or(false))
}
