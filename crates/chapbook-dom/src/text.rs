//! Plain-text extraction from a content document, for display, snapshots,
//! and the layout text-order property tests.
//!
//! NOT the offset space for `Locator::char_offset` — that is the raw,
//! versioned locator text in [`crate::locator_text`] (see `docs/LOCATORS.md`).
//! This output collapses whitespace and adds block separators, and is free to
//! evolve without shifting stored positions.

use markup5ever::local_name;

use crate::tree::{Document, NodeData, NodeId};

/// Sentinel between block boxes; becomes a newline. Ordinary `\n` in source
/// text is inline whitespace and collapses like a space.
const BLOCK_BOUNDARY: char = '\u{1}';
/// Sentinel for a forced `<br>` line break within a block.
const LINE_BREAK: char = '\u{2}';

/// Extract the readable text of a document.
///
/// Head content, scripts, and styles are skipped. Whitespace runs — including
/// source line-wrapping newlines — collapse to single spaces (EPUB content is
/// not `white-space: pre` in the common case; pre-handling arrives with
/// layout). Block boundaries yield newlines so paragraphs stay
/// distinguishable in snapshots.
pub fn extract_text(doc: &Document) -> String {
    let mut raw = String::new();
    if let Some(html) = doc.document_element() {
        walk(doc, html, &mut raw);
    }
    normalize(&raw)
}

fn walk(doc: &Document, id: NodeId, out: &mut String) {
    let node = doc.node(id);
    match &node.data {
        NodeData::Text(text) => out.push_str(text),
        NodeData::Element(el) => {
            match *el.local_name() {
                local_name!("head")
                | local_name!("script")
                | local_name!("style")
                | local_name!("template") => return,
                local_name!("br") => {
                    out.push(LINE_BREAK);
                    return;
                }
                _ => {}
            }
            let block = is_block(el.local_name());
            if block {
                out.push(BLOCK_BOUNDARY);
            }
            for child in &node.children {
                walk(doc, *child, out);
            }
            if block {
                out.push(BLOCK_BOUNDARY);
            }
        }
        _ => {
            for child in &node.children {
                walk(doc, *child, out);
            }
        }
    }
}

/// Tags treated as block-level for text-boundary purposes. This is the
/// default-UA-display set relevant to EPUB content, not a full CSS answer —
/// layout will use computed `display` once the cascade exists.
fn is_block(name: &markup5ever::LocalName) -> bool {
    matches!(
        *name,
        local_name!("address")
            | local_name!("article")
            | local_name!("aside")
            | local_name!("blockquote")
            | local_name!("div")
            | local_name!("dd")
            | local_name!("dl")
            | local_name!("dt")
            | local_name!("figcaption")
            | local_name!("figure")
            | local_name!("footer")
            | local_name!("h1")
            | local_name!("h2")
            | local_name!("h3")
            | local_name!("h4")
            | local_name!("h5")
            | local_name!("h6")
            | local_name!("header")
            | local_name!("hr")
            | local_name!("li")
            | local_name!("main")
            | local_name!("nav")
            | local_name!("ol")
            | local_name!("p")
            | local_name!("pre")
            | local_name!("section")
            | local_name!("table")
            | local_name!("td")
            | local_name!("th")
            | local_name!("tr")
            | local_name!("ul")
    )
}

/// The CSS-collapsible whitespace set (space, tab, LF, CR, FF). Notably NOT
/// U+00A0 no-break space — that is content, and it exists to not collapse.
fn is_css_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C')
}

/// Collapse whitespace within blocks; render block boundaries as newlines
/// (runs of empty blocks collapse) and `<br>` as a hard newline in place.
fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for block in raw.split(BLOCK_BOUNDARY) {
        let lines: Vec<String> = block
            .split(LINE_BREAK)
            .map(|seg| {
                seg.split(is_css_space)
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        let block_text = lines.join("\n");
        if block_text.trim_matches(is_css_space).is_empty() {
            continue;
        }
        out.push_str(&block_text);
        out.push('\n');
    }
    out
}
