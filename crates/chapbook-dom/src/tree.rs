use std::cell::Cell;
use std::sync::atomic::AtomicBool;

use markup5ever::{local_name, ns, LocalName, QualName};
use selectors::matching::ElementSelectorFlags;
use slotmap::{new_key_type, SlotMap};
use style::properties::PropertyDeclarationBlock;
use style::servo_arc::Arc as ServoArc;
use style::shared_lock::{Locked, SharedRwLock};
use style::stylesheets::UrlExtraData;
use style::Atom;

use crate::stylo_data::StyloData;

new_key_type! {
    /// Arena key for a [`Node`] within its [`Document`].
    pub struct NodeId;
}

/// An XHTML content document as an arena of nodes.
///
/// Static after parse: the tree shape never changes, which is what lets the
/// stylo binding skip snapshots and incremental restyle machinery.
///
/// The real state lives in a heap-boxed [`DocumentInner`] (reachable through
/// `Deref`) so its address is stable across moves of `Document`: stylo's
/// style sharing cache requires the element handle to be pointer-sized, so
/// the handle is `&Node` and every node carries a back-pointer to the inner
/// document (blitz-dom does the same with its `*mut NodeTree`).
pub struct Document {
    pub(crate) inner: Box<DocumentInner>,
}

pub struct DocumentInner {
    pub(crate) nodes: SlotMap<NodeId, Node>,
    pub(crate) root: NodeId,
    /// Container-root path of this document within its EPUB, for resolving
    /// relative hrefs (stylesheets, images).
    pub base_path: String,
    /// Lock guarding stylo `Locked<T>` values owned by this document (style
    /// attributes). Replaced by the style engine's lock in
    /// [`DocumentInner::attach_style_context`] so every `Locked` in a style
    /// run belongs to one lock.
    pub(crate) guard: SharedRwLock,
    /// Base URL for CSS parsing within this document (relative url() etc.).
    pub(crate) url_data: UrlExtraData,
}

impl std::ops::Deref for Document {
    type Target = DocumentInner;
    fn deref(&self) -> &DocumentInner {
        &self.inner
    }
}

impl std::ops::DerefMut for Document {
    fn deref_mut(&mut self) -> &mut DocumentInner {
        &mut self.inner
    }
}

pub struct Node {
    /// Back-pointer to the owning [`DocumentInner`], filled in by
    /// [`DocumentInner::seal`]. What makes `&Node` a self-sufficient,
    /// pointer-sized stylo handle.
    pub(crate) doc: *const DocumentInner,
    /// This node's own arena key, filled in by [`DocumentInner::seal`].
    pub(crate) id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
    /// Per-node stylo hooks (style data, selector flags); interior-mutable
    /// because stylo's traversal works through `&self` handles.
    pub(crate) stylo: StyloState,
}

impl Node {
    pub(crate) fn new(parent: Option<NodeId>, data: NodeData) -> Self {
        Node {
            doc: std::ptr::null(),
            id: NodeId::default(),
            parent,
            children: Vec::new(),
            data,
            stylo: StyloState::default(),
        }
    }

    /// The owning document. Only valid after [`DocumentInner::seal`].
    pub(crate) fn document(&self) -> &DocumentInner {
        debug_assert!(!self.doc.is_null(), "node accessed before Document::seal");
        // SAFETY: `doc` points at the Box<DocumentInner> that owns this node;
        // the box's address is stable and outlives any `&Node` borrow, which
        // necessarily borrows from the same document.
        unsafe { &*self.doc }
    }
}

/// The per-node state stylo needs during selector matching and the restyle
/// traversal.
pub(crate) struct StyloState {
    pub(crate) data: StyloData,
    pub(crate) selector_flags: Cell<ElementSelectorFlags>,
    pub(crate) snapshot_handled: AtomicBool,
    pub(crate) dirty_descendants: Cell<bool>,
}

impl Default for StyloState {
    fn default() -> Self {
        StyloState {
            data: StyloData::default(),
            selector_flags: Cell::new(ElementSelectorFlags::empty()),
            snapshot_handled: AtomicBool::new(false),
            dirty_descendants: Cell::new(false),
        }
    }
}

// SAFETY: the `Cell`/`UnsafeCell` state above is only mutated either through
// `&mut Document` or from within stylo's style traversal, which guarantees
// exclusive access to each node it visits. Chapbook runs that traversal
// sequentially (no rayon pool is passed to `traverse_dom`). This mirrors
// blitz-dom's Node, which carries the same impls for the same reason.
unsafe impl Send for Node {}
unsafe impl Sync for Node {}

pub enum NodeData {
    /// The document root (not an element).
    Document,
    Element(ElementData),
    Text(String),
    /// Comments and processing instructions are kept as inert placeholders so
    /// sibling indices stay meaningful, but carry no content.
    Comment,
}

pub struct ElementData {
    pub name: QualName,
    pub attrs: Vec<(QualName, String)>,
    /// Cached, interned `id` attribute (selector matching hot path; stylo's
    /// `TElement::id` wants an `&Atom`). Derefs to `str` for plain use.
    pub id: Option<Atom>,
    /// Cached, whitespace-split `class` attribute (selector matching hot path).
    pub classes: Vec<String>,
    /// Parsed `style="..."` attribute, populated by
    /// [`Document::attach_style_context`] before a style run.
    pub(crate) style_attribute: Option<ServoArc<Locked<PropertyDeclarationBlock>>>,
}

impl ElementData {
    pub fn new(name: QualName, attrs: Vec<(QualName, String)>) -> Self {
        let mut id = None;
        let mut classes = Vec::new();
        for (attr_name, value) in &attrs {
            if attr_name.ns.is_empty() {
                if attr_name.local == local_name!("id") {
                    id = Some(Atom::from(value.as_str()));
                } else if attr_name.local == local_name!("class") {
                    classes = value.split_ascii_whitespace().map(str::to_string).collect();
                }
            }
        }
        ElementData {
            name,
            attrs,
            id,
            classes,
            style_attribute: None,
        }
    }

    /// Value of a no-namespace attribute.
    pub fn attr(&self, name: &LocalName) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n.ns.is_empty() && n.local == *name)
            .map(|(_, v)| v.as_str())
    }

    pub fn local_name(&self) -> &LocalName {
        &self.name.local
    }
}

/// A stylesheet referenced or embedded by a content document, in document order.
pub enum StylesheetSource {
    /// `<link rel="stylesheet" href="...">` — href as written, unresolved.
    External(String),
    /// `<style>` element contents.
    Inline(String),
}

impl Document {
    pub(crate) fn new(base_path: String) -> Self {
        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(Node::new(None, NodeData::Document));
        let url = url::Url::parse(&format!("epub:///{base_path}"))
            .unwrap_or_else(|_| url::Url::parse("epub:///content.xhtml").unwrap());
        let mut doc = Document {
            inner: Box::new(DocumentInner {
                nodes,
                root,
                base_path,
                guard: SharedRwLock::new(),
                url_data: UrlExtraData(ServoArc::new(url)),
            }),
        };
        doc.inner.seal();
        doc
    }
}

impl DocumentInner {
    /// Fill every node's back-pointer and self-id. Called after parse (and
    /// defensively before styling); idempotent. Nodes never move within the
    /// arena, so a seal stays valid for the document's lifetime.
    pub(crate) fn seal(&mut self) {
        let ptr: *const DocumentInner = self;
        let ids: Vec<NodeId> = self.nodes.keys().collect();
        for id in ids {
            let node = &mut self.nodes[id];
            node.doc = ptr;
            node.id = id;
        }
    }

    /// Adopt the style engine's shared lock and parse every `style="..."`
    /// attribute under it. Must run before this document participates in a
    /// style traversal so that all `Locked<T>` values (stylesheets, style
    /// attributes) belong to the one lock the traversal's guards come from.
    pub fn attach_style_context(&mut self, lock: SharedRwLock) {
        self.seal();
        self.guard = lock;
        let ids: Vec<NodeId> = self.descendants(self.root).collect();
        for id in ids {
            let style_text = match &self.nodes[id].data {
                NodeData::Element(el) => el
                    .attr(&local_name!("style"))
                    .map(str::to_string)
                    .filter(|s| !s.trim().is_empty()),
                _ => None,
            };
            let Some(style_text) = style_text else {
                continue;
            };
            let block = style::properties::parse_style_attribute(
                &style_text,
                &self.url_data,
                None,
                selectors::matching::QuirksMode::NoQuirks,
                style::stylesheets::CssRuleType::Style,
            );
            let locked = ServoArc::new(self.guard.wrap(block));
            if let NodeData::Element(el) = &mut self.nodes[id].data {
                el.style_attribute = Some(locked);
            }
        }
    }

    /// The lock guarding this document's stylo `Locked<T>` values.
    pub fn shared_lock(&self) -> &SharedRwLock {
        &self.guard
    }

    pub(crate) fn index_in_parent(&self, id: NodeId) -> Option<(NodeId, usize)> {
        let parent = self.nodes[id].parent?;
        let idx = self.nodes[parent].children.iter().position(|c| *c == id)?;
        Some((parent, idx))
    }

    pub(crate) fn sibling(&self, id: NodeId, offset: isize) -> Option<NodeId> {
        let (parent, idx) = self.index_in_parent(id)?;
        let target = idx.checked_add_signed(offset)?;
        self.nodes[parent].children.get(target).copied()
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// The `<html>` element, if the document has one.
    pub fn document_element(&self) -> Option<NodeId> {
        self.nodes[self.root]
            .children
            .iter()
            .copied()
            .find(|id| matches!(self.nodes[*id].data, NodeData::Element(_)))
    }

    /// Depth-first pre-order traversal from `start`.
    pub fn descendants(&self, start: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut stack = vec![start];
        std::iter::from_fn(move || {
            let id = stack.pop()?;
            stack.extend(self.nodes[id].children.iter().rev().copied());
            Some(id)
        })
    }

    /// Find an element anywhere in the document by its `id` attribute.
    pub fn element_by_id(&self, id: &str) -> Option<NodeId> {
        self.descendants(self.root).find(|n| {
            matches!(&self.nodes[*n].data, NodeData::Element(e)
                if e.id.as_deref() == Some(id))
        })
    }

    /// Stylesheets referenced by the document head, in document order:
    /// `<link rel="stylesheet">` hrefs and `<style>` contents.
    pub fn stylesheet_sources(&self) -> Vec<StylesheetSource> {
        let mut out = Vec::new();
        for id in self.descendants(self.root) {
            let NodeData::Element(el) = &self.nodes[id].data else {
                continue;
            };
            match el.local_name() {
                l if *l == local_name!("link") => {
                    let is_stylesheet = el.attr(&local_name!("rel")).is_some_and(|rel| {
                        rel.split_ascii_whitespace()
                            .any(|r| r.eq_ignore_ascii_case("stylesheet"))
                    });
                    if is_stylesheet {
                        if let Some(href) = el.attr(&local_name!("href")) {
                            out.push(StylesheetSource::External(href.to_string()));
                        }
                    }
                }
                l if *l == local_name!("style") => {
                    let css: String = self.nodes[id]
                        .children
                        .iter()
                        .filter_map(|c| match &self.nodes[*c].data {
                            NodeData::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect();
                    out.push(StylesheetSource::Inline(css));
                }
                _ => {}
            }
        }
        out
    }

    /// True if this element is in the XHTML namespace with the given tag name.
    pub fn is_html_element(&self, id: NodeId, name: &LocalName) -> bool {
        match &self.nodes[id].data {
            NodeData::Element(e) => e.name.ns == ns!(html) && e.name.local == *name,
            _ => false,
        }
    }
}
