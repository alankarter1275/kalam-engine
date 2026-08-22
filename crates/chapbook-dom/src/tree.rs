use markup5ever::{local_name, ns, LocalName, QualName};
use slotmap::{new_key_type, SlotMap};

new_key_type! {
    /// Arena key for a [`Node`] within its [`Document`].
    pub struct NodeId;
}

/// An XHTML content document as an arena of nodes.
///
/// Static after parse: the tree shape never changes, which is what lets the
/// stylo binding skip snapshots and incremental restyle machinery.
pub struct Document {
    pub(crate) nodes: SlotMap<NodeId, Node>,
    pub(crate) root: NodeId,
    /// Container-root path of this document within its EPUB, for resolving
    /// relative hrefs (stylesheets, images).
    pub base_path: String,
}

pub struct Node {
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub data: NodeData,
}

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
    /// Cached `id` attribute (selector matching hot path).
    pub id: Option<String>,
    /// Cached, whitespace-split `class` attribute (selector matching hot path).
    pub classes: Vec<String>,
}

impl ElementData {
    pub fn new(name: QualName, attrs: Vec<(QualName, String)>) -> Self {
        let mut id = None;
        let mut classes = Vec::new();
        for (attr_name, value) in &attrs {
            if attr_name.ns.is_empty() {
                if attr_name.local == local_name!("id") {
                    id = Some(value.clone());
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
        let root = nodes.insert(Node {
            parent: None,
            children: Vec::new(),
            data: NodeData::Document,
        });
        Document {
            nodes,
            root,
            base_path,
        }
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
        self.descendants(self.root).find(
            |n| matches!(&self.nodes[*n].data, NodeData::Element(e) if e.id.as_deref() == Some(id)),
        )
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
