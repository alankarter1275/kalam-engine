//! html5ever tree building into the arena [`Document`].
//!
//! html5ever's `TreeSink` methods take `&self`, so construction goes through
//! interior mutability; the finished `Document` is extracted at `finish()`.

use std::borrow::Cow;
use std::cell::{Ref, RefCell};

use html5ever::tendril::TendrilSink;
use html5ever::{parse_document, Attribute, ParseOpts, QualName};
use markup5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::local_name;

use chapbook_core::Result;

use super::tree::{Document, ElementData, Node, NodeData, NodeId};

/// Parse the bytes of an XHTML content document.
///
/// `base_path` is the document's own container-root path within its EPUB,
/// kept for resolving relative hrefs later. Parsing is lenient (HTML parsing
/// algorithm): real-world EPUBs contain named entities, unclosed tags, and
/// other HTML-isms that a strict XML parse would reject. Bytes are decoded as
/// UTF-8 (lossily); non-UTF-8 EPUBs are rare enough to punt on.
pub fn parse_xhtml(bytes: &[u8], base_path: &str) -> Result<Document> {
    let sink = Sink {
        doc: RefCell::new(Document::new(base_path.to_string())),
    };
    let mut document = parse_document(sink, ParseOpts::default())
        .from_utf8()
        .one(bytes);
    // Fill node back-pointers/self-ids now that the tree is complete; &Node
    // becomes a self-sufficient handle for the stylo traits.
    document.seal();
    Ok(document)
}

struct Sink {
    doc: RefCell<Document>,
}

impl Sink {
    fn detached_node(&self, data: NodeData) -> NodeId {
        self.doc.borrow_mut().nodes.insert(Node::new(None, data))
    }

    fn append_child(&self, parent: NodeId, child: NodeId) {
        let mut doc = self.doc.borrow_mut();
        if let Some(old_parent) = doc.nodes[child].parent {
            let idx = doc.nodes[old_parent]
                .children
                .iter()
                .position(|c| *c == child);
            if let Some(idx) = idx {
                doc.nodes[old_parent].children.remove(idx);
            }
        }
        doc.nodes[child].parent = Some(parent);
        doc.nodes[parent].children.push(child);
    }

    fn append_text(&self, parent: NodeId, text: &str) {
        let mut doc = self.doc.borrow_mut();
        // Merge with a trailing text node so entity-split runs come out whole.
        if let Some(&last) = doc.nodes[parent].children.last() {
            if let NodeData::Text(existing) = &mut doc.nodes[last].data {
                existing.push_str(text);
                return;
            }
        }
        let id = doc
            .nodes
            .insert(Node::new(Some(parent), NodeData::Text(text.to_string())));
        doc.nodes[parent].children.push(id);
    }
}

impl TreeSink for Sink {
    type Handle = NodeId;
    type Output = Document;
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Document {
        self.doc.into_inner()
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> NodeId {
        self.doc.borrow().root
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> Ref<'a, QualName> {
        Ref::map(self.doc.borrow(), |doc| match &doc.nodes[*target].data {
            NodeData::Element(e) => &e.name,
            _ => panic!("elem_name called on non-element node"),
        })
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> NodeId {
        let attrs = attrs
            .into_iter()
            .map(|a| (a.name, a.value.to_string()))
            .collect();
        self.detached_node(NodeData::Element(ElementData::new(name, attrs)))
    }

    fn create_comment(&self, _text: html5ever::tendril::StrTendril) -> NodeId {
        self.detached_node(NodeData::Comment)
    }

    fn create_pi(
        &self,
        _target: html5ever::tendril::StrTendril,
        _data: html5ever::tendril::StrTendril,
    ) -> NodeId {
        self.detached_node(NodeData::Comment)
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        match child {
            NodeOrText::AppendNode(node) => self.append_child(*parent, node),
            NodeOrText::AppendText(text) => self.append_text(*parent, &text),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.doc.borrow().nodes[*element].parent.is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: html5ever::tendril::StrTendril,
        _public_id: html5ever::tendril::StrTendril,
        _system_id: html5ever::tendril::StrTendril,
    ) {
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        // No <template> support in EPUB content; treat contents as children.
        *target
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let (parent, idx) = {
            let doc = self.doc.borrow();
            let parent = doc.nodes[*sibling]
                .parent
                .expect("append_before_sibling on parentless node");
            let idx = doc.nodes[parent]
                .children
                .iter()
                .position(|c| c == sibling)
                .expect("sibling not found in parent");
            (parent, idx)
        };
        match new_node {
            NodeOrText::AppendNode(node) => {
                let mut doc = self.doc.borrow_mut();
                if let Some(old_parent) = doc.nodes[node].parent {
                    let old_idx = doc.nodes[old_parent]
                        .children
                        .iter()
                        .position(|c| *c == node);
                    if let Some(old_idx) = old_idx {
                        doc.nodes[old_parent].children.remove(old_idx);
                    }
                }
                doc.nodes[node].parent = Some(parent);
                doc.nodes[parent].children.insert(idx, node);
            }
            NodeOrText::AppendText(text) => {
                let mut doc = self.doc.borrow_mut();
                // Merge into the preceding text node when there is one.
                if idx > 0 {
                    let prev = doc.nodes[parent].children[idx - 1];
                    if let NodeData::Text(existing) = &mut doc.nodes[prev].data {
                        existing.push_str(&text);
                        return;
                    }
                }
                let id = doc
                    .nodes
                    .insert(Node::new(Some(parent), NodeData::Text(text.to_string())));
                doc.nodes[parent].children.insert(idx, id);
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        let mut doc = self.doc.borrow_mut();
        let NodeData::Element(el) = &mut doc.nodes[*target].data else {
            return;
        };
        for attr in attrs {
            if !el.attrs.iter().any(|(n, _)| *n == attr.name) {
                let value = attr.value.to_string();
                if attr.name.ns.is_empty() {
                    if attr.name.local == local_name!("id") && el.id.is_none() {
                        el.id = Some(style::Atom::from(value.as_str()));
                    } else if attr.name.local == local_name!("class") && el.classes.is_empty() {
                        el.classes = value.split_ascii_whitespace().map(str::to_string).collect();
                    }
                }
                el.attrs.push((attr.name, value));
            }
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        let mut doc = self.doc.borrow_mut();
        if let Some(parent) = doc.nodes[*target].parent.take() {
            let idx = doc.nodes[parent].children.iter().position(|c| c == target);
            if let Some(idx) = idx {
                doc.nodes[parent].children.remove(idx);
            }
        }
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let children = {
            let mut doc = self.doc.borrow_mut();
            std::mem::take(&mut doc.nodes[*node].children)
        };
        for child in children {
            let mut doc = self.doc.borrow_mut();
            doc.nodes[child].parent = Some(*new_parent);
            doc.nodes[*new_parent].children.push(child);
        }
    }
}
