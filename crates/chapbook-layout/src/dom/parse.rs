//! xml5ever / html5ever tree building into the arena [`Document`].
//!
//! Both parsers drive the same `TreeSink`; its methods take `&self`, so
//! construction goes through interior mutability and the finished
//! `Document` is extracted at `finish()`.

use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell};

use html5ever::tendril::TendrilSink;
use html5ever::{Attribute, ParseOpts, QualName};
use markup5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use markup5ever::local_name;

use chapbook_core::Result;

use super::tree::{Document, ElementData, Node, NodeData, NodeId};

/// Parse the bytes of an XHTML content document.
///
/// `base_path` is the document's own container-root path within its EPUB,
/// kept for resolving relative hrefs later. Bytes are decoded as UTF-8
/// (lossily); non-UTF-8 EPUBs are rare enough to punt on.
///
/// kalam: XML first, HTML as the fallback. EPUB content documents are XML
/// by specification, and mainstream publishers write them that way —
/// `<a id="x"/>`, `<span epub:type="pagebreak"/>` — and the HTML parsing
/// algorithm has no self-closing syntax for those elements: `<a …/>` opens
/// an `<a>` that never closes, the rest of the chapter lands inside it,
/// and every paragraph flattens into one inline run (seen on a Penguin
/// Random House title, chapter-as-one-paragraph). A browser picks its
/// parser by media type and reads `.xhtml` as XML; this does the same,
/// keeping the lenient HTML parse for files that are not well-formed
/// (named entities, unclosed tags), which the XML parse reports as errors.
pub fn parse_xhtml(bytes: &[u8], base_path: &str) -> Result<Document> {
    if let Some(document) = parse_as_xml(bytes, base_path) {
        return finish(document);
    }
    parse_as_html(bytes, base_path)
}

/// The strict pass. `None` when the XML parser reported anything at all —
/// one recovered error means the tree may be shaped by recovery rules
/// (the same failure class the fallback exists for), so the whole
/// document goes to the HTML parser instead of trusting a partial tree.
/// (One report is disregarded; see `Sink::parse_error`.)
/// Also `None` when the result has no XHTML `<html>` root: an entity-only
/// or namespace-less document is better served by the HTML tree builder,
/// which puts every element in the XHTML namespace where the UA sheet and
/// the box tree look for it.
fn parse_as_xml(bytes: &[u8], base_path: &str) -> Option<Document> {
    let sink = Sink {
        doc: RefCell::new(Document::new(base_path.to_string())),
        errors: Cell::new(0),
    };
    let parser = xml5ever::driver::parse_document(sink, xml5ever::driver::XmlParseOpts::default());
    let (document, errors) = parser.from_utf8().one(bytes);
    if errors > 0 {
        return None;
    }
    let root = document.document_element()?;
    document
        .is_html_element(root, &local_name!("html"))
        .then_some(document)
}

/// The lenient pass: the HTML parsing algorithm, which copes with named
/// entities, unclosed tags, and the other HTML-isms of real-world EPUBs.
fn parse_as_html(bytes: &[u8], base_path: &str) -> Result<Document> {
    let sink = Sink {
        doc: RefCell::new(Document::new(base_path.to_string())),
        errors: Cell::new(0),
    };
    let (document, _errors) = html5ever::parse_document(sink, ParseOpts::default())
        .from_utf8()
        .one(bytes);
    finish(document)
}

/// The post-parse passes both parsers share.
fn finish(mut document: Document) -> Result<Document> {
    // MathML gets no layout from stylo; capture each <math> source for the
    // native renderer, then rewrite the subtree into its EPUB altimg/alttext
    // fallback before anything walks the tree — the fallback tree is the
    // locator authority whether or not the native renderer runs.
    super::math_fallback::apply_mathml_fallback(&mut document);
    // Inline <svg> stays in the tree (its text keeps its locator
    // contribution); the serialized source lets layout treat it as a
    // replaced image when a rasterizer is in the build.
    super::foreign::capture_svg_roots(&mut document);
    // Fill node back-pointers/self-ids now that the tree is complete; &Node
    // becomes a self-sufficient handle for the stylo traits.
    document.seal();
    Ok(document)
}

struct Sink {
    doc: RefCell<Document>,
    /// Parse errors reported so far. Only the XML pass reads it: any error
    /// there sends the document to the HTML parser.
    errors: Cell<u32>,
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
    type Output = (Document, u32);
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> (Document, u32) {
        (self.doc.into_inner(), self.errors.get())
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        // kalam: xml5ever 0.39 looks for duplicate attributes by local
        // name alone, so `xml:lang="en" lang="en"` — the pair on the
        // <html> of nearly every EPUB that calibre, InDesign or the spec's
        // own samples produced — is reported as a duplicate, and the bare
        // `lang` is dropped. That report must not demote a well-formed
        // document to the HTML parser (which would re-open the
        // self-closed-anchor bug for most books). A real duplicate loses
        // its later copy, which is what the HTML algorithm does with one
        // too. Fixed in xml5ever 0.40 (servo/html5ever#780); this line
        // becomes dead once the dependency moves.
        if msg == "Duplicate attribute" {
            return;
        }
        self.errors.set(self.errors.get().saturating_add(1));
    }

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
