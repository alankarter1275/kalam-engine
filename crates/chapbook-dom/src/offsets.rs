//! The locator text: the normative offset space for `Locator::char_offset`.
//!
//! The definition lives in `chapbook_core::locator` (module docs) and is
//! versioned by `chapbook_core::LOCATOR_VERSION`; `docs/LOCATORS.md` has the
//! rationale. Changing what this function produces shifts every stored
//! reading position and annotation — the golden offset-map test in
//! `tests/locator_text.rs` makes that a conscious, version-bumped act.
//!
//! This is deliberately NOT [`crate::extract_text`]: that output is
//! display/snapshot text with collapsed whitespace and block separators,
//! free to evolve. Locator text is raw text-node content, concatenated with
//! nothing added, so offsets are independent of whitespace-collapsing rules.

use markup5ever::local_name;

use crate::tree::{Document, NodeData, NodeId};

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

fn excluded(doc: &Document, id: NodeId) -> bool {
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
