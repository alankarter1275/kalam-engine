//! The locator text: the normative offset space for `Locator::char_offset`.
//!
//! The definition lives in `chapbook_core::locator` (module docs) and is
//! versioned by `chapbook_core::LOCATOR_VERSION`; `docs/LOCATORS.md` has the
//! rationale. Changing what this function produces shifts every stored
//! reading position and annotation — the golden offset-map test in
//! `tests/locator_text.rs` makes that a conscious, version-bumped act.
//!
//! This is deliberately NOT [`extract_text`]: that output is
//! display/snapshot text with collapsed whitespace and block separators,
//! free to evolve. Locator text is raw text-node content, concatenated with
//! nothing added, so offsets are independent of whitespace-collapsing rules.

use markup5ever::local_name;

use super::tree::{Document, NodeData, NodeId};

/// Extract the locator text of a document (see module docs).
///
/// Version 1 semantics: content of text nodes in document order, excluding
/// entire `head`, `script`, `style`, and `template` subtrees; nothing added
/// between nodes; no whitespace collapsing. (Excluding `display:none`
/// subtrees requires the cascade and will be a `LOCATOR_VERSION` bump.)
pub fn locator_text(doc: &Document) -> String {
    let mut out = String::new();
    if let Some(html) = doc.document_element() {
        walk(doc, html, &mut out);
    }
    out
}

/// Char offset (Unicode scalars) into the locator text at which `node`'s
/// text content begins; `None` for nodes in excluded subtrees. The
/// building block for layout's `char_map` (M3).
pub fn locator_offset_of(doc: &Document, node: NodeId) -> Option<u32> {
    let html = doc.document_element()?;
    let mut count = 0u32;
    let mut found = None;
    walk_count(doc, html, node, &mut count, &mut found);
    found
}

/// Stable opaque tag for a node, for producers that must reference nodes
/// without exposing document-model types (chapbook-paint fragment tags).
pub fn node_tag(id: NodeId) -> u64 {
    use slotmap::Key;
    id.data().as_ffi()
}

/// One-pass map of every non-excluded node (elements *and* text nodes) to
/// the locator-text char offset at which its content begins. What layout
/// uses to stamp lines and anchors with locator offsets without an O(n²)
/// per-node walk.
pub fn locator_offsets(doc: &Document) -> std::collections::HashMap<NodeId, u32> {
    let mut map = std::collections::HashMap::new();
    if let Some(html) = doc.document_element() {
        let mut count = 0u32;
        walk_map(doc, html, &mut count, &mut map);
    }
    map
}

fn walk_map(
    doc: &Document,
    id: NodeId,
    count: &mut u32,
    map: &mut std::collections::HashMap<NodeId, u32>,
) {
    if excluded(doc, id) {
        return;
    }
    map.insert(id, *count);
    match &doc.node(id).data {
        NodeData::Text(text) => *count += text.chars().count() as u32,
        _ => {
            for child in &doc.node(id).children {
                walk_map(doc, *child, count, map);
            }
        }
    }
}

/// One `<a href>` and the locator-offset span of the text it wraps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Half-open locator range `[start, end)` covered by the link's text.
    pub start: u32,
    pub end: u32,
    /// The `href` as written. Resolving it against the containing document
    /// is the caller's job — see `chapbook_epub::resolve_href`.
    pub href: String,
}

/// Hyperlinks by locator range, in document order.
///
/// Paint carries no DOM types by design, so a link cannot ride on a
/// fragment; it rides on the offset space every glyph already carries
/// instead, which also means a hit test survives relayout for free.
/// Anchors wrapping no text — an image link — have an empty range and are
/// dropped, since there is nothing to hit-test against.
pub fn links(doc: &Document) -> Vec<Link> {
    let mut out = Vec::new();
    if let Some(html) = doc.document_element() {
        let mut count = 0;
        collect_links(doc, html, &mut count, &mut out);
    }
    out
}

fn collect_links(doc: &Document, id: NodeId, count: &mut u32, out: &mut Vec<Link>) {
    if excluded(doc, id) {
        return;
    }
    if let NodeData::Text(text) = &doc.node(id).data {
        *count += text.chars().count() as u32;
        return;
    }
    let href = match &doc.node(id).data {
        NodeData::Element(el) if *el.local_name() == local_name!("a") => {
            el.attr(&local_name!("href")).map(str::to_string)
        }
        _ => None,
    };
    let start = *count;
    for child in &doc.node(id).children {
        collect_links(doc, *child, count, out);
    }
    if let Some(href) = href {
        if *count > start {
            out.push(Link {
                start,
                end: *count,
                href,
            });
        }
    }
}

pub(crate) fn excluded(doc: &Document, id: NodeId) -> bool {
    match &doc.node(id).data {
        NodeData::Element(el) => matches!(
            *el.local_name(),
            local_name!("head")
                | local_name!("script")
                | local_name!("style")
                | local_name!("template")
        ),
        _ => false,
    }
}

fn walk(doc: &Document, id: NodeId, out: &mut String) {
    match &doc.node(id).data {
        NodeData::Text(text) => out.push_str(text),
        _ if excluded(doc, id) => {}
        _ => {
            for child in &doc.node(id).children {
                walk(doc, *child, out);
            }
        }
    }
}

fn walk_count(
    doc: &Document,
    id: NodeId,
    target: NodeId,
    count: &mut u32,
    found: &mut Option<u32>,
) {
    if found.is_some() {
        return;
    }
    if excluded(doc, id) {
        return;
    }
    if id == target {
        *found = Some(*count);
        return;
    }
    match &doc.node(id).data {
        NodeData::Text(text) => *count += text.chars().count() as u32,
        _ => {
            for child in &doc.node(id).children {
                walk_count(doc, *child, target, count, found);
            }
        }
    }
}
