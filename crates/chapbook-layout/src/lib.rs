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
//! v1 gaps (documented in the module that owns each): floats,
//! `@media`-scoped break rules, keep-with-next (`break-after: avoid`),
//! hyphenation (cosmic-text has no soft-hyphen support), justified first
//! lines of indented paragraphs (see `shape_inline_indented`), non-string
//! `content` items, table rowspan / vertical-align / header repetition
//! (see `crate::table`).

mod boxtree;
mod fonts;
mod fragmentation;
mod paginate;
mod style_to_attrs;
mod table;
mod webfonts;

use std::collections::HashMap;

use cosmic_text::FontSystem;

use chapbook_core::PageMetrics;
use chapbook_dom::Document;
use chapbook_paint::{ImageStore, Page};

pub use fonts::{fixture_font_system, system_font_system};
pub use fragmentation::{BreakRule, FragRules, FragStyle};
pub use webfonts::{extract_font_faces, register_font, FontFace};

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
    images: &ImageStore,
) -> ChapterLayout {
    let frag = FragRules::parse(css_sources).resolve(doc);
    let locator = chapbook_dom::locator_offsets(doc);
    let input = boxtree::BoxTreeInput {
        doc,
        frag: &frag,
        locator: &locator,
        images,
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

/// Decode every `<img>`'s bytes into an [`ImageStore`] keyed by node tag.
/// `fetch` resolves an `src` attribute (as written) to raw bytes — callers
/// close over their container (e.g. `Book::resource` against the chapter
/// path). Undecodable or unresolvable images are skipped; layout degrades
/// them to nothing.
pub fn collect_images(
    doc: &Document,
    mut fetch: impl FnMut(&str) -> Option<Vec<u8>>,
) -> ImageStore {
    let mut store = ImageStore::default();
    for id in doc.descendants(doc.root()) {
        let chapbook_dom::NodeData::Element(el) = &doc.node(id).data else {
            continue;
        };
        if *el.local_name() != markup5ever::local_name!("img") {
            continue;
        }
        let Some(src) = el.attr(&markup5ever::local_name!("src")) else {
            continue;
        };
        let Some(bytes) = fetch(src) else { continue };
        let Ok(decoded) = image::load_from_memory(&bytes) else {
            continue;
        };
        let rgba = decoded.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        store.insert(chapbook_dom::node_tag(id), w, h, rgba.into_raw());
    }
    store
}
