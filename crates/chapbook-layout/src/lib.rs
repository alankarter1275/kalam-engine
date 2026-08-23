//! The pagination-first layout engine — chapbook's differentiator.
//!
//! Pipeline: styled DOM → fragmentation sidecar cascade (servo-mode stylo
//! lacks the break properties) → box tree (CSS 2.1 §9.2 anonymous boxes) →
//! cosmic-text inline layout per inline formatting context → streaming page
//! cursor applying break rules and widows/orphans → [`ChapterLayout`].
//!
//! One spine item is one layout run. Locator offsets (see chapbook-core's
//! locator docs) thread through every line fragment, so positions survive
//! relayout via [`ChapterLayout::page_of`].
//!
//! v1 gaps (documented in the module that owns each): `::before`/`::after`
//! content, text-indent, tables (stacked blocks), floats, `@media`-scoped
//! break rules, keep-with-next (`break-after: avoid`).

mod boxtree;
mod fonts;
mod fragmentation;
mod paginate;
mod style_to_attrs;

use std::collections::HashMap;

use cosmic_text::FontSystem;

use chapbook_core::PageMetrics;
use chapbook_dom::Document;
use chapbook_paint::Page;

pub use fonts::{fixture_font_system, system_font_system};
pub use fragmentation::{BreakRule, FragRules, FragStyle};

/// The paginated result of laying out one spine item.
pub struct ChapterLayout {
    pub pages: Vec<Page>,
    /// Per page: locator-text char offset at which the page starts
    /// (monotonic non-decreasing). The relayout-survival map for
    /// `Locator::char_offset`.
    pub char_map: Vec<u32>,
    /// Element `id` attribute → page index (TOC fragment jumps).
    pub anchors: HashMap<String, usize>,
}

impl ChapterLayout {
    /// Page containing the given locator offset.
    pub fn page_of(&self, char_offset: u32) -> usize {
        page_of(&self.char_map, char_offset)
    }
}

fn page_of(char_map: &[u32], char_offset: u32) -> usize {
    if char_map.is_empty() {
        return 0;
    }
    char_map
        .partition_point(|start| *start <= char_offset)
        .saturating_sub(1)
}

/// Paginate a styled document (the cascade must have run: see
/// `chapbook_style::StyleEngine::style_document`).
///
/// `css_sources` are the same author sheets given to the style engine — the
/// fragmentation sidecar re-reads them for the break properties stylo
/// doesn't carry.
pub fn paginate(
    doc: &Document,
    css_sources: &[String],
    page: &PageMetrics,
    fonts: &mut FontSystem,
) -> ChapterLayout {
    let frag = FragRules::parse(css_sources).resolve(doc);
    let locator = chapbook_dom::locator_offsets(doc);
    let input = boxtree::BoxTreeInput {
        doc,
        frag: &frag,
        locator: &locator,
    };

    let mut paginator = paginate::Paginator::new(fonts, *page);
    if let Some(root) = boxtree::build_box_tree(&input) {
        paginator.place_block(&root, 0.0, page.content_width());
    }
    let (pages, char_map) = paginator.finish();

    // Anchors: element id → page, via each element's locator offset.
    let mut anchors = HashMap::new();
    for id in doc.descendants(doc.root()) {
        if let chapbook_dom::NodeData::Element(el) = &doc.node(id).data {
            if let (Some(id_attr), Some(offset)) = (&el.id, locator.get(&id)) {
                anchors.insert(id_attr.to_string(), page_of(&char_map, *offset));
            }
        }
    }

    ChapterLayout {
        pages,
        char_map,
        anchors,
    }
}
