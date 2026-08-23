//! Box-tree construction: styled DOM → block boxes and inline formatting
//! contexts, per CSS 2.1 §9.2 (anonymous block boxes wrap inline runs that
//! have block siblings).
//!
//! v1 degradations, per ARCHITECTURE.md: table display types and list items
//! become plain blocks (list items get a text marker), floats/positioning
//! are ignored, deeply-inline-nested images are skipped.
//! `::before`/`::after` emit literal string content (counters/attr()/images
//! in `content` are skipped).

use std::collections::HashMap;

use style::properties::ComputedValues;
use style::servo_arc::Arc as ServoArc;

use chapbook_dom::{Document, NodeData, NodeId};
use chapbook_paint::ImageStore;

use crate::fragmentation::FragStyle;

/// A block-level box. Its content is either more blocks or one inline
/// formatting context (the classic simplification: mixed content is
/// normalized with anonymous blocks).
pub struct BlockBox {
    /// The generating element; anonymous boxes borrow their parent's node.
    pub node: NodeId,
    pub style: ServoArc<ComputedValues>,
    pub frag: FragStyle,
    /// True for generated anonymous boxes (no own margins/padding/breaks).
    pub anonymous: bool,
    pub kind: BlockKind,
}

pub enum BlockKind {
    Container(Vec<BlockBox>),
    Inline(InlineContent),
    /// A replaced image block (intrinsic size in CSS px).
    Image {
        width: u32,
        height: u32,
    },
    /// A horizontal rule.
    Rule,
    /// A real table grid (see `crate::table`).
    Table(Box<crate::table::TableBox>),
}

/// One inline formatting context: styled text runs in document order, with
/// whitespace already collapsed and per-char locator offsets retained.
#[derive(Default)]
pub struct InlineContent {
    pub runs: Vec<InlineRun>,
}

#[derive(Clone)]
pub struct InlineRun {
    pub text: String,
    /// Locator-text char offset for each char of `text` (same length in
    /// chars). Markers and other generated chars repeat their element's
    /// offset.
    pub offsets: Vec<u32>,
    pub style: ServoArc<ComputedValues>,
}

impl InlineContent {
    fn is_empty_or_space(&self) -> bool {
        self.runs.iter().all(|r| r.text.chars().all(|c| c == ' '))
    }

    fn ends_with_space(&self) -> bool {
        self.runs
            .last()
            .and_then(|r| r.text.chars().next_back())
            .is_none_or(|c| c == ' ')
    }

    /// Drop leading collapsible spaces (used when re-shaping the remainder
    /// of a split IFC: the line break consumed them).
    pub(crate) fn trim_leading_space(&mut self) {
        while let Some(run) = self.runs.first_mut() {
            let trimmed = run.text.trim_start_matches(' ').len();
            let removed_chars =
                run.text.chars().count() - run.text[run.text.len() - trimmed..].chars().count();
            if removed_chars > 0 {
                run.text = run.text[run.text.len() - trimmed..].to_string();
                run.offsets.drain(..removed_chars);
            }
            if run.text.is_empty() {
                self.runs.remove(0);
            } else {
                break;
            }
        }
    }

    fn trim_trailing_space(&mut self) {
        while let Some(run) = self.runs.last_mut() {
            if run.text.ends_with(' ') {
                run.text.pop();
                run.offsets.pop();
            }
            if run.text.is_empty() {
                self.runs.pop();
            } else if !run.text.ends_with(' ') {
                break;
            }
        }
    }
}

pub struct BoxTreeInput<'a> {
    pub doc: &'a Document,
    pub frag: &'a HashMap<NodeId, FragStyle>,
    /// Node → locator-text offset (from `chapbook_dom::locator_offsets`).
    pub locator: &'a HashMap<NodeId, u32>,
    /// Decoded images keyed by node tag (from `crate::collect_images`).
    pub images: &'a ImageStore,
}

/// Build the box tree from the `<body>` element. Returns `None` when there
/// is no body or everything is `display: none`.
pub fn build_box_tree(input: &BoxTreeInput) -> Option<BlockBox> {
    let doc = input.doc;
    let html = doc.document_element()?;
    let body = doc
        .node(html)
        .children
        .iter()
        .copied()
        .find(|id| doc.is_html_element(*id, &markup5ever::local_name!("body")))?;
    let style = doc.primary_styles(body)?;
    if display_of(&style) == DisplayClass::None {
        return None;
    }
    Some(build_block(input, body, style))
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum DisplayClass {
    None,
    /// block, list-item, and the block-degraded display types (tables).
    Block,
    ListItem,
    /// inline, inline-block (v1: treated as inline text context).
    Inline,
}

pub fn display_of(style: &ComputedValues) -> DisplayClass {
    use style::values::specified::box_::DisplayOutside;
    let display = style.get_box().display;
    if display.is_none() {
        return DisplayClass::None;
    }
    match display.outside() {
        DisplayOutside::Inline => DisplayClass::Inline,
        _ => {
            if display.is_list_item() {
                DisplayClass::ListItem
            } else {
                DisplayClass::Block
            }
        }
    }
}

fn build_block(input: &BoxTreeInput, node: NodeId, style: ServoArc<ComputedValues>) -> BlockBox {
    let doc = input.doc;
    let frag = input.frag.get(&node).copied().unwrap_or_default();

    // Table elements get the real grid treatment.
    if style.get_box().display.inside() == style::values::specified::box_::DisplayInside::Table {
        if let Some(table) = crate::table::build_table(input, node) {
            return BlockBox {
                node,
                style,
                frag,
                anonymous: false,
                kind: BlockKind::Table(Box::new(table)),
            };
        }
        // Cell-less table: fall through to ordinary block handling.
    }

    // Determine content model: any block-level element child → container.
    let has_block_child = doc.node(node).children.iter().any(|child| {
        matches!(&doc.node(*child).data, NodeData::Element(_))
            && doc.primary_styles(*child).is_some_and(|s| {
                !matches!(display_of(&s), DisplayClass::Inline | DisplayClass::None)
            })
    });

    let kind = if has_block_child {
        let mut children = Vec::new();
        let mut pending_inline = InlineContent::default();
        collect_container(
            input,
            node,
            &style,
            &mut children,
            &mut pending_inline,
            node,
        );
        flush_anonymous(&mut children, &mut pending_inline, node, &style);
        BlockKind::Container(children)
    } else {
        let mut inline = InlineContent::default();
        if display_of(&style) == DisplayClass::ListItem {
            push_marker(&mut inline, input, node, &style);
        }
        push_generated(
            &mut inline,
            input,
            node,
            chapbook_dom::PseudoElement::Before,
        );
        collect_inline(input, node, &style, &mut inline);
        push_generated(&mut inline, input, node, chapbook_dom::PseudoElement::After);
        inline.trim_trailing_space();
        BlockKind::Inline(inline)
    };

    BlockBox {
        node,
        style,
        frag,
        anonymous: false,
        kind,
    }
}

/// Walk `node`'s children building block children, wrapping inline runs
/// between them into anonymous blocks.
fn collect_container(
    input: &BoxTreeInput,
    node: NodeId,
    inherited_style: &ServoArc<ComputedValues>,
    children: &mut Vec<BlockBox>,
    pending_inline: &mut InlineContent,
    anon_node: NodeId,
) {
    let doc = input.doc;
    for child in &doc.node(node).children {
        match &doc.node(*child).data {
            NodeData::Text(text) => {
                append_collapsed(
                    pending_inline,
                    text,
                    input.locator.get(child).copied().unwrap_or(0),
                    inherited_style.clone(),
                );
            }
            NodeData::Element(_) => {
                let Some(style) = doc.primary_styles(*child) else {
                    continue;
                };
                if display_of(&style) != DisplayClass::None {
                    if let Some(replaced) = replaced_block(input, *child, &style) {
                        flush_anonymous(children, pending_inline, anon_node, inherited_style);
                        children.push(replaced);
                        continue;
                    }
                }
                match display_of(&style) {
                    DisplayClass::None => {}
                    DisplayClass::Inline => {
                        collect_inline_element(input, *child, style, pending_inline);
                    }
                    DisplayClass::Block | DisplayClass::ListItem => {
                        flush_anonymous(children, pending_inline, anon_node, inherited_style);
                        children.push(build_block(input, *child, style));
                    }
                }
            }
            _ => {}
        }
    }
}

fn flush_anonymous(
    children: &mut Vec<BlockBox>,
    pending: &mut InlineContent,
    node: NodeId,
    style: &ServoArc<ComputedValues>,
) {
    let mut inline = std::mem::take(pending);
    inline.trim_trailing_space();
    if inline.is_empty_or_space() {
        return;
    }
    children.push(BlockBox {
        node,
        style: style.clone(),
        frag: FragStyle::default(),
        anonymous: true,
        kind: BlockKind::Inline(inline),
    });
}

/// Collect the inline content of `node` (its children, not itself).
fn collect_inline(
    input: &BoxTreeInput,
    node: NodeId,
    style: &ServoArc<ComputedValues>,
    out: &mut InlineContent,
) {
    let doc = input.doc;
    for child in &doc.node(node).children {
        match &doc.node(*child).data {
            NodeData::Text(text) => {
                append_collapsed(
                    out,
                    text,
                    input.locator.get(child).copied().unwrap_or(0),
                    style.clone(),
                );
            }
            NodeData::Element(_) => {
                let Some(child_style) = doc.primary_styles(*child) else {
                    continue;
                };
                match display_of(&child_style) {
                    DisplayClass::None => {}
                    // Block-in-inline is rare in books; v1 flattens it into
                    // the surrounding inline flow.
                    _ => collect_inline_element(input, *child, child_style, out),
                }
            }
            _ => {}
        }
    }
}

fn collect_inline_element(
    input: &BoxTreeInput,
    node: NodeId,
    style: ServoArc<ComputedValues>,
    out: &mut InlineContent,
) {
    if input
        .doc
        .is_html_element(node, &markup5ever::local_name!("br"))
    {
        // Forced line break: encoded as '\n', which cosmic-text treats as a
        // hard line boundary within the buffer.
        let offset = input.locator.get(&node).copied().unwrap_or(0);
        out.runs.push(InlineRun {
            text: "\n".to_string(),
            offsets: vec![offset],
            style,
        });
        return;
    }
    push_generated(out, input, node, chapbook_dom::PseudoElement::Before);
    collect_inline(input, node, &style, out);
    push_generated(out, input, node, chapbook_dom::PseudoElement::After);
}

/// `::before`/`::after` generated content: literal string items only
/// (counters, attr(), and images are unsupported and skipped). Generated
/// chars carry their element's locator offset, like list markers — they are
/// presentation, not source text.
fn push_generated(
    out: &mut InlineContent,
    input: &BoxTreeInput,
    node: NodeId,
    pseudo: chapbook_dom::PseudoElement,
) {
    let Some(style) = input.doc.pseudo_styles(node, pseudo) else {
        return;
    };
    use style::values::generics::counters::{Content, ContentItem};
    let text: String = match &style.get_counters().content {
        Content::Items(items) => items
            .items
            .iter()
            .filter_map(|item| match item {
                ContentItem::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect(),
        _ => return,
    };
    if text.is_empty() {
        return;
    }
    let offset = input.locator.get(&node).copied().unwrap_or(0);
    let offsets = vec![offset; text.chars().count()];
    out.runs.push(InlineRun {
        text,
        offsets,
        style,
    });
}

/// Append a text node's content with CSS `white-space: normal` collapsing
/// (the collapsible set: space, tab, CR, LF, FF — never NBSP), tracking the
/// locator offset of every kept char.
/// Crate-visible alias for the table builder, which lives in a sibling
/// module but shares the collapsing rules.
pub(crate) fn append_collapsed_pub(
    out: &mut InlineContent,
    text: &str,
    locator_start: u32,
    style: ServoArc<ComputedValues>,
) {
    append_collapsed(out, text, locator_start, style)
}

fn append_collapsed(
    out: &mut InlineContent,
    text: &str,
    locator_start: u32,
    style: ServoArc<ComputedValues>,
) {
    let mut run_text = String::new();
    let mut run_offsets = Vec::new();
    let mut last_space = out.ends_with_space();
    for (i, ch) in text.chars().enumerate() {
        let offset = locator_start + i as u32;
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0C') {
            if !last_space {
                run_text.push(' ');
                run_offsets.push(offset);
                last_space = true;
            }
        } else {
            run_text.push(ch);
            run_offsets.push(offset);
            last_space = false;
        }
    }
    if run_text.is_empty() {
        return;
    }
    out.runs.push(InlineRun {
        text: run_text,
        offsets: run_offsets,
        style,
    });
}

/// List markers as leading text runs — the v1 stand-in for real marker
/// boxes. Numbering: position among list-item siblings for `ol`, bullet
/// otherwise.
fn push_marker(
    inline: &mut InlineContent,
    input: &BoxTreeInput,
    node: NodeId,
    style: &ServoArc<ComputedValues>,
) {
    let doc = input.doc;
    let parent = doc.node(node).parent;
    let ordered = parent.is_some_and(|p| doc.is_html_element(p, &markup5ever::local_name!("ol")));
    let marker = if ordered {
        let index = parent
            .map(|p| {
                doc.node(p)
                    .children
                    .iter()
                    .filter(|c| {
                        matches!(&doc.node(**c).data, NodeData::Element(_))
                            && doc
                                .primary_styles(**c)
                                .is_some_and(|s| display_of(&s) == DisplayClass::ListItem)
                    })
                    .position(|c| *c == node)
                    .map(|i| i + 1)
                    .unwrap_or(1)
            })
            .unwrap_or(1);
        format!("{index}. ")
    } else {
        "\u{2022} ".to_string()
    };
    let offset = input.locator.get(&node).copied().unwrap_or(0);
    let offsets = vec![offset; marker.chars().count()];
    inline.runs.push(InlineRun {
        text: marker,
        offsets,
        style: style.clone(),
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplacedKind {
    Image,
    Rule,
}

fn replaced_kind(doc: &Document, id: NodeId) -> Option<ReplacedKind> {
    if doc.is_html_element(id, &markup5ever::local_name!("img")) {
        Some(ReplacedKind::Image)
    } else if doc.is_html_element(id, &markup5ever::local_name!("hr")) {
        Some(ReplacedKind::Rule)
    } else {
        None
    }
}

/// Build a replaced block for `img`/`hr`. An image missing from the store
/// (unresolvable src, undecodable data) degrades to nothing.
fn replaced_block(
    input: &BoxTreeInput,
    node: NodeId,
    style: &ServoArc<ComputedValues>,
) -> Option<BlockBox> {
    let kind = match replaced_kind(input.doc, node)? {
        ReplacedKind::Rule => BlockKind::Rule,
        ReplacedKind::Image => {
            let (width, height) = input.images.dims(chapbook_dom::node_tag(node))?;
            BlockKind::Image { width, height }
        }
    };
    Some(BlockBox {
        node,
        style: style.clone(),
        frag: input.frag.get(&node).copied().unwrap_or_default(),
        anonymous: false,
        kind,
    })
}
