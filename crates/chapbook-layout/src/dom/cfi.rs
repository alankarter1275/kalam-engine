//! CFI ↔ locator-offset conversion over a parsed content document.
//!
//! The syntax layer ([`chapbook_core::Cfi`]) knows nothing about
//! documents; this module walks the DOM. Two mappings, inverse of each
//! other up to clamping:
//!
//! - [`cfi_for_offset`]: a locator-text char offset becomes element steps
//!   (even indices count *all* element children, including `head` and
//!   `script` — CFI addresses the document, not the locator text) plus a
//!   character-data step and offset. Element `id`s ride along as
//!   assertions for robustness in other readers.
//! - [`offset_for_cfi`]: steps walk back down, with the spec's assertion
//!   correction (a matching `id` wins over a stale index), and the
//!   character-data span maps back into locator space.
//!
//! CFI counts characters of the raw text-node data — the same unit as
//! locator text — so offsets convert without re-encoding. Comments never
//! contribute characters and do not split a character-data span; only
//! element children do.

use chapbook_core::{Cfi, CfiStep};

use super::offsets::{excluded, locator_offset_of, locator_offsets};
use super::tree::{Document, NodeData, NodeId};

/// Generate a point CFI for a locator-text char offset. `None` when the
/// document has no text at all.
pub fn cfi_for_offset(doc: &Document, spine_index: usize, char_offset: u32) -> Option<Cfi> {
    let (text_node, local) = text_node_at(doc, char_offset)?;
    let parent = doc.node(text_node).parent?;

    // Character-data step: the span index among the parent's children, and
    // the offset within the whole span (consecutive text siblings merge;
    // comments neither count nor split).
    let mut elements_before = 0u32;
    let mut prefix_chars = 0u32;
    for &child in &doc.node(parent).children {
        if child == text_node {
            break;
        }
        match &doc.node(child).data {
            NodeData::Element(_) => {
                elements_before += 1;
                prefix_chars = 0;
            }
            NodeData::Text(t) => prefix_chars += t.chars().count() as u32,
            _ => {}
        }
    }
    let mut doc_steps = vec![CfiStep {
        index: 2 * elements_before + 1,
        assertion: None,
    }];
    let span_offset = prefix_chars + local;

    // Element steps from the text node's parent up to — but excluding —
    // the root element: CFI paths start at the document's root element
    // (`/4` after `!` is `<body>`, `<html>` is implicit).
    let root = doc.document_element()?;
    let mut node = parent;
    while node != root {
        let p = doc.node(node).parent?;
        let mut index = 0u32;
        for &child in &doc.node(p).children {
            if matches!(doc.node(child).data, NodeData::Element(_)) {
                index += 1;
            }
            if child == node {
                break;
            }
        }
        let assertion = match &doc.node(node).data {
            NodeData::Element(el) => el.id.as_ref().map(|a| a.to_string()),
            _ => None,
        };
        doc_steps.push(CfiStep {
            index: 2 * index,
            assertion,
        });
        node = p;
    }
    doc_steps.reverse();

    Some(Cfi {
        package_steps: Cfi::package_steps_for_spine(spine_index),
        doc_steps,
        char_offset: Some(span_offset),
    })
}

/// Resolve a CFI's content-document part to a locator-text char offset.
/// The caller is responsible for handing over the document the CFI's
/// package part addresses (see [`Cfi::spine_index`]).
pub fn offset_for_cfi(doc: &Document, cfi: &Cfi) -> Option<u32> {
    // Paths start at the document's root element (see `cfi_for_offset`).
    let mut node = doc.document_element()?;
    for (i, step) in cfi.doc_steps.iter().enumerate() {
        if step.index % 2 == 0 && step.index > 0 {
            node = element_child(doc, node, step)?;
        } else {
            // Character-data step: terminal by construction.
            if i + 1 != cfi.doc_steps.len() {
                return None;
            }
            return span_to_locator(doc, node, step.index, cfi.char_offset.unwrap_or(0));
        }
    }
    // Element terminus: the element's content start.
    locator_offset_of(doc, node)
}

/// The text node containing `offset` and the offset within it; an offset
/// at or past the end of the text clamps to the last text node's end.
fn text_node_at(doc: &Document, offset: u32) -> Option<(NodeId, u32)> {
    let html = doc.document_element()?;
    let mut count = 0u32;
    let mut last: Option<(NodeId, u32)> = None;
    let mut found: Option<(NodeId, u32)> = None;
    walk_text(doc, html, offset, &mut count, &mut last, &mut found);
    found.or(last)
}

fn walk_text(
    doc: &Document,
    id: NodeId,
    target: u32,
    count: &mut u32,
    last: &mut Option<(NodeId, u32)>,
    found: &mut Option<(NodeId, u32)>,
) {
    if found.is_some() || excluded(doc, id) {
        return;
    }
    match &doc.node(id).data {
        NodeData::Text(text) => {
            let len = text.chars().count() as u32;
            if target < *count + len {
                *found = Some((id, target - *count));
            } else if len > 0 {
                *last = Some((id, len));
            }
            *count += len;
        }
        _ => {
            for child in &doc.node(id).children {
                walk_text(doc, *child, target, count, last, found);
            }
        }
    }
}

/// The `want`-th (1-based, from an even step index) element child — unless
/// the step's ID assertion names a different sibling, which wins (the
/// assertion exists to correct stale indices across editions).
fn element_child(doc: &Document, parent: NodeId, step: &CfiStep) -> Option<NodeId> {
    let want = step.index / 2;
    let mut nth = 0u32;
    let mut by_index = None;
    let mut by_id = None;
    for &child in &doc.node(parent).children {
        if let NodeData::Element(el) = &doc.node(child).data {
            nth += 1;
            if nth == want {
                by_index = Some(child);
            }
            if let (Some(id), Some(assertion)) = (&el.id, &step.assertion) {
                if &**id == assertion.as_str() && by_id.is_none() {
                    by_id = Some(child);
                }
            }
        }
    }
    by_id.or(by_index)
}

/// Map (character-data span `index` under `parent`, `offset` within it)
/// back to a locator-text char offset.
fn span_to_locator(doc: &Document, parent: NodeId, index: u32, offset: u32) -> Option<u32> {
    let skip_elements = (index - 1) / 2;
    let offsets = locator_offsets(doc);
    let mut elements = 0u32;
    let mut in_span = skip_elements == 0;
    let mut remaining = offset;
    let mut last_in_span: Option<(NodeId, u32)> = None;
    for &child in &doc.node(parent).children {
        match &doc.node(child).data {
            NodeData::Element(_) => {
                if in_span {
                    // Span ended before the offset was consumed (or the
                    // span holds no text): clamp to the span end, else to
                    // the terminating element's content start.
                    return match last_in_span {
                        Some((node, len)) => offsets.get(&node).map(|s| s + len),
                        None => offsets.get(&child).copied(),
                    };
                }
                elements += 1;
                in_span = elements == skip_elements;
            }
            NodeData::Text(text) if in_span => {
                let len = text.chars().count() as u32;
                if remaining < len {
                    return offsets.get(&child).map(|s| s + remaining);
                }
                remaining -= len;
                if len > 0 {
                    last_in_span = Some((child, len));
                }
            }
            _ => {}
        }
    }
    last_in_span.and_then(|(node, len)| offsets.get(&node).map(|s| s + len))
}
