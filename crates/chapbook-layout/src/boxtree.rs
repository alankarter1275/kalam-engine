//! Box-tree construction: styled DOM → block boxes and inline formatting
//! contexts, per CSS 2.1 §9.2 (anonymous block boxes wrap inline runs that
//! have block siblings).
//!
//! v1 degradations, per ARCHITECTURE.md: list items become plain blocks
//! with a text marker; positioning is ignored; floats lay out for real
//! (see `paginate`) except shrink-to-fit non-replaced floats, which stay
//! in flow; images nested deeper than one level inside inline content are
//! skipped. `::before`/`::after` emit string, `attr()`, and quote content
//! (`counter()`/`counters()` and images in `content` are skipped).

use std::collections::HashMap;

use style::properties::ComputedValues;
use style::selector_parser::PseudoElement;
use style::servo_arc::Arc as ServoArc;

use crate::dom::{Document, NodeData, NodeId};
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
    /// On an anonymous box: it is the container's first in-flow content,
    /// so the container's `text-indent` applies to its first line (floats
    /// are out of flow and do not consume the indent).
    pub indent_first: bool,
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
    /// A natively laid-out block `<math>` (see `crate::mathml`).
    #[cfg(feature = "mathml")]
    Math(Box<crate::mathml::PreparedMath>),
}

/// One inline formatting context: styled text runs in document order, with
/// whitespace already collapsed and per-char locator offsets retained.
#[derive(Default, Clone)]
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
    /// Node → locator-text offset (from `crate::dom::locator_offsets`).
    pub locator: &'a HashMap<NodeId, u32>,
    /// Decoded images keyed by node tag (from `crate::collect_images`).
    pub images: &'a ImageStore,
    /// Formulas laid out by formulary (from `crate::mathml::prepare`).
    #[cfg(feature = "mathml")]
    pub math: &'a crate::mathml::MathStore,
    /// CSS quote nesting depth for `open-quote`/`close-quote` content,
    /// advanced in document order as the box tree is walked.
    pub quote_depth: std::cell::Cell<usize>,
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
                indent_first: false,
                kind: BlockKind::Table(Box::new(table)),
            };
        }
        // Cell-less table: fall through to ordinary block handling.
    }

    // Determine content model: any block-level element child → container.
    // Replaced children (img/hr) count too — they always hoist to their own
    // block, so `<p><img/>text</p>` becomes a container instead of dropping
    // the image (and a floated image can wrap its paragraph's text).
    let has_block_child = doc.node(node).children.iter().any(|child| {
        matches!(&doc.node(*child).data, NodeData::Element(_))
            && (replaced_kind(input, *child).is_some()
                || doc.primary_styles(*child).is_some_and(|s| {
                    !matches!(display_of(&s), DisplayClass::Inline | DisplayClass::None)
                }))
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
        flush_anonymous(input, &mut children, &mut pending_inline, node, &style);
        // The container's text-indent belongs to its first in-flow line
        // box: mark the first non-floated child if it is an anonymous
        // inline (a hoisted floated image does not consume the indent).
        for child in &mut children {
            if !child.anonymous && child.style.get_box().float.is_floating() {
                continue;
            }
            if child.anonymous {
                child.indent_first = true;
            }
            break;
        }
        BlockKind::Container(children)
    } else {
        let mut inline = InlineContent::default();
        if display_of(&style) == DisplayClass::ListItem {
            push_marker(&mut inline, input, node, &style);
        }
        push_generated(&mut inline, input, node, PseudoElement::Before);
        collect_inline(input, node, &style, &mut inline);
        push_generated(&mut inline, input, node, PseudoElement::After);
        inline.trim_trailing_space();
        if frag.hyphens_auto {
            crate::hyphenate::apply(&mut inline);
        }
        BlockKind::Inline(inline)
    };

    BlockBox {
        node,
        style,
        frag,
        anonymous: false,
        indent_first: false,
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
                        flush_anonymous(
                            input,
                            children,
                            pending_inline,
                            anon_node,
                            inherited_style,
                        );
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
                        flush_anonymous(
                            input,
                            children,
                            pending_inline,
                            anon_node,
                            inherited_style,
                        );
                        children.push(build_block(input, *child, style));
                    }
                }
            }
            _ => {}
        }
    }
}

fn flush_anonymous(
    input: &BoxTreeInput,
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
    // Anonymous boxes carry no fragmentation style of their own, but the
    // inherited hyphenation setting of the wrapping element still applies.
    if input.frag.get(&node).is_some_and(|f| f.hyphens_auto) {
        crate::hyphenate::apply(&mut inline);
    }
    children.push(BlockBox {
        node,
        style: style.clone(),
        frag: FragStyle::default(),
        anonymous: true,
        indent_first: false,
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
    push_generated(out, input, node, PseudoElement::Before);
    collect_inline(input, node, &style, out);
    push_generated(out, input, node, PseudoElement::After);
}

/// `::before`/`::after` generated content: string, `attr()`, and quote
/// items (with proper nesting depth, tracked in document order across the
/// box-tree walk). `counter()`/`counters()` need document counter scopes
/// and `url()` images are not text — both are skipped (documented).
/// Generated chars carry their element's locator offset, like list
/// markers — they are presentation, not source text.
fn push_generated(
    out: &mut InlineContent,
    input: &BoxTreeInput,
    node: NodeId,
    pseudo: PseudoElement,
) {
    let Some(style) = input.doc.pseudo_styles(node, pseudo) else {
        return;
    };
    use style::values::generics::counters::{Content, ContentItem};
    let Content::Items(items) = &style.get_counters().content else {
        return;
    };
    let mut text = String::new();
    for item in items.items.iter() {
        match item {
            ContentItem::String(s) => text.push_str(s),
            ContentItem::Attr(attr) => {
                if let NodeData::Element(el) = &input.doc.node(node).data {
                    let name: &str = &attr.attribute;
                    match el.attrs.iter().find(|(q, _)| &*q.local == name) {
                        Some((_, value)) => text.push_str(value),
                        None => text.push_str(&attr.fallback),
                    }
                }
            }
            ContentItem::OpenQuote => {
                let depth = input.quote_depth.get();
                text.push_str(&quote_mark(&style, depth, true));
                input.quote_depth.set(depth + 1);
            }
            ContentItem::CloseQuote => {
                let depth = input.quote_depth.get().saturating_sub(1);
                input.quote_depth.set(depth);
                text.push_str(&quote_mark(&style, depth, false));
            }
            ContentItem::NoOpenQuote => input.quote_depth.set(input.quote_depth.get() + 1),
            ContentItem::NoCloseQuote => input
                .quote_depth
                .set(input.quote_depth.get().saturating_sub(1)),
            _ => {}
        }
    }
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

/// The quote mark for `open-quote`/`close-quote` at `depth`: from the
/// computed `quotes` list, or English typographic quotes for `auto`.
/// Deeper nesting than the list provides repeats the last pair.
fn quote_mark(style: &ComputedValues, depth: usize, open: bool) -> String {
    use style::values::specified::list::Quotes;
    match &style.get_list().quotes {
        Quotes::QuoteList(list) => match list.0.iter().count() {
            0 => String::new(),
            n => {
                let pair = &list.0[depth.min(n - 1)];
                if open {
                    pair.opening.to_string()
                } else {
                    pair.closing.to_string()
                }
            }
        },
        Quotes::Auto => {
            let pairs = [("\u{201C}", "\u{201D}"), ("\u{2018}", "\u{2019}")];
            let (o, c) = pairs[depth.min(1)];
            (if open { o } else { c }).to_string()
        }
    }
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
    #[cfg(feature = "mathml")]
    Math,
}

fn replaced_kind(input: &BoxTreeInput, id: NodeId) -> Option<ReplacedKind> {
    let doc = input.doc;
    // A prepared formula wins over the fallback form its node was rewritten
    // into (an `<img>` for altimg, a flattening `<math>` otherwise).
    #[cfg(feature = "mathml")]
    if input.math.contains(crate::dom::node_tag(id)) {
        return Some(ReplacedKind::Math);
    }
    if doc.is_html_element(id, &markup5ever::local_name!("img")) {
        Some(ReplacedKind::Image)
    } else if doc.is_html_element(id, &markup5ever::local_name!("hr")) {
        Some(ReplacedKind::Rule)
    } else if crate::dom::is_svg_root(doc, id)
        && input.images.dims(crate::dom::node_tag(id)).is_some()
    {
        // An inline `<svg>` is replaced only once rasterized; without an
        // entry in the store it keeps flattening to its text as before.
        Some(ReplacedKind::Image)
    } else {
        None
    }
}

/// Build a replaced block for `img`/`hr`/`svg`/`math`. An image missing
/// from the store (unresolvable src, undecodable data) degrades to nothing.
fn replaced_block(
    input: &BoxTreeInput,
    node: NodeId,
    style: &ServoArc<ComputedValues>,
) -> Option<BlockBox> {
    let kind = match replaced_kind(input, node)? {
        ReplacedKind::Rule => BlockKind::Rule,
        ReplacedKind::Image => {
            let (width, height) = input.images.dims(crate::dom::node_tag(node))?;
            BlockKind::Image { width, height }
        }
        #[cfg(feature = "mathml")]
        ReplacedKind::Math => BlockKind::Math(Box::new(
            input.math.get(crate::dom::node_tag(node))?.clone(),
        )),
    };
    Some(BlockBox {
        node,
        style: style.clone(),
        frag: input.frag.get(&node).copied().unwrap_or_default(),
        anonymous: false,
        indent_first: false,
        kind,
    })
}
