//! Foreign-content capture: the XML sources that native MathML and SVG
//! rendering consume, taken from the tree at parse time.
//!
//! The DOM after `parse_xhtml` is the locator-text authority and must not
//! depend on whether a math font or an SVG rasterizer is in the build — so
//! `<math>` still takes its EPUB fallback rewrite and inline `<svg>` keeps
//! its subtree, exactly as before. What changes is that the original markup
//! is serialized first and carried in a sidecar keyed by node: a rendering
//! input, never a locator input. Renderers that are compiled out simply
//! leave the sidecar unread.
//!
//! Serialization writes local names only. formulary matches MathML by local
//! name, and the SVG root gets its `xmlns` declarations re-attached so usvg
//! sees a well-formed document; `xlink:`/`xml:`-namespaced attributes keep
//! their prefixes (html5ever has already adjusted them per the foreign-
//! content rules).

use markup5ever::{local_name, ns, LocalName};

use super::tree::{Document, NodeData, NodeId};

/// The original markup of one outermost `<math>` element, captured before
/// the altimg/alttext fallback rewrite destroys it.
#[derive(Debug, Clone)]
pub struct MathSource {
    /// The `<math>` subtree as XML (local names, no namespace decls).
    pub xml: String,
    /// `display="block"` on the `<math>` element: the publisher marked this
    /// as display math. Inline math always renders via the fallback — a
    /// replaced block mid-sentence would read worse than flattened tokens.
    pub block: bool,
    /// `alttext`, kept for the line fragment's diagnostic text.
    pub alttext: Option<String>,
    /// The publisher shipped a usable fallback (altimg or alttext), so a
    /// degraded native parse should defer to it.
    pub has_fallback: bool,
}

/// Serialize the outermost SVG-namespace `<svg>` subtrees into the
/// document's sidecar. Runs after the MathML rewrite (an `<svg>` inside a
/// stripped `annotation-xml` is already gone) and before `seal`.
pub(super) fn capture_svg_roots(doc: &mut Document) {
    let mut roots = Vec::new();
    collect_svg_roots(doc, doc.root(), &mut roots);
    for id in roots {
        let mut xml = String::new();
        serialize(doc, id, &mut xml, true);
        doc.inner.svg.insert(id, xml);
    }
}

/// Outermost `<svg>` elements in document order; nested `<svg>` serializes
/// as part of its outer root.
fn collect_svg_roots(doc: &Document, id: NodeId, out: &mut Vec<NodeId>) {
    if is_svg_root(doc, id) {
        out.push(id);
        return;
    }
    for child in &doc.node(id).children {
        collect_svg_roots(doc, *child, out);
    }
}

pub(crate) fn is_svg_root(doc: &Document, id: NodeId) -> bool {
    match &doc.node(id).data {
        NodeData::Element(e) => e.name.ns == ns!(svg) && e.name.local == local_name!("svg"),
        _ => false,
    }
}

/// Serialize a subtree as XML. `declare_ns` re-attaches the SVG namespace
/// declarations on the root element.
pub(super) fn serialize(doc: &Document, id: NodeId, out: &mut String, declare_ns: bool) {
    match &doc.node(id).data {
        NodeData::Element(el) => {
            out.push('<');
            out.push_str(&el.name.local);
            if declare_ns {
                out.push_str(r#" xmlns="http://www.w3.org/2000/svg""#);
                out.push_str(r#" xmlns:xlink="http://www.w3.org/1999/xlink""#);
            }
            for (name, value) in &el.attrs {
                // The declared root writes its own namespace declarations;
                // drop the author's (the parser either left `xmlns` as a
                // plain attribute or adjusted `xmlns:*` into the xmlns
                // namespace).
                let is_ns_decl = name.ns == ns!(xmlns) || name.local == local_name!("xmlns");
                if declare_ns && is_ns_decl {
                    continue;
                }
                out.push(' ');
                if let Some(prefix) = attr_prefix(name) {
                    out.push_str(prefix);
                    out.push(':');
                }
                out.push_str(&name.local);
                out.push_str("=\"");
                escape_into(value, out, true);
                out.push('"');
            }
            if doc.node(id).children.is_empty() {
                out.push_str("/>");
                return;
            }
            out.push('>');
            for child in &doc.node(id).children {
                serialize(doc, *child, out, false);
            }
            out.push_str("</");
            out.push_str(&el.name.local);
            out.push('>');
        }
        NodeData::Text(text) => escape_into(text, out, false),
        _ => {}
    }
}

/// Prefix for a namespaced attribute, per the foreign-content adjustments
/// html5ever applied at parse time (`xmlns:xlink` adjusts to local
/// `xlink` in the xmlns namespace; bare `xmlns` keeps its local name).
fn attr_prefix(name: &markup5ever::QualName) -> Option<&'static str> {
    if name.ns == ns!(xlink) {
        Some("xlink")
    } else if name.ns == ns!(xml) {
        Some("xml")
    } else if name.ns == ns!(xmlns) && name.local != local_name!("xmlns") {
        Some("xmlns")
    } else {
        None
    }
}

fn escape_into(text: &str, out: &mut String, in_attr: bool) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if in_attr => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// Capture one `<math>` element's source (called by the fallback rewrite,
/// before it touches the subtree).
pub(super) fn capture_math(doc: &mut Document, id: NodeId) {
    let NodeData::Element(el) = &doc.node(id).data else {
        return;
    };
    let non_empty = |name: &str| {
        el.attr(&LocalName::from(name))
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string)
    };
    let block = el
        .attr(&LocalName::from("display"))
        .is_some_and(|v| v.trim() == "block");
    let alttext = non_empty("alttext");
    let has_fallback = alttext.is_some() || non_empty("altimg").is_some();
    let mut xml = String::new();
    serialize(doc, id, &mut xml, false);
    doc.inner.math.insert(
        id,
        MathSource {
            xml,
            block,
            alttext,
            has_fallback,
        },
    );
}
