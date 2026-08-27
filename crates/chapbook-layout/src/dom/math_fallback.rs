//! The EPUB 3 fallback for MathML content — chapbook does not lay out math.
//!
//! EPUB 3 permits reading systems without MathML support to fall back to a
//! `<math>` element's `altimg` attribute (a publisher-shipped image of the
//! equation) and `alttext` (a plain-text reading). Applied as a post-parse
//! tree rewrite so everything downstream — cascade, box tree, locator text,
//! links — sees ordinary content and needs no math awareness:
//!
//! 1. `altimg` → the `<math>` element becomes an XHTML `<img>` whose `src`
//!    is the altimg href (resolved and decoded exactly like an authored
//!    image); `alttext` rides along as `alt`. The math children are dropped.
//!    Original attributes (`id`, `class`, `style`) are kept, so anchors and
//!    book CSS keep working.
//! 2. `alttext` without `altimg` → the children are replaced by one text
//!    node holding the alttext, rendered as inline text.
//! 3. Neither → last resort: the token text flattens into the line as
//!    before, but `annotation`/`annotation-xml` subtrees are dropped so
//!    alternate encodings (LaTeX source, Content MathML) don't leak into
//!    the page.
//!
//! Because this changes the post-parse tree, it changes what `locator_text`
//! yields for math-bearing documents; it is part of the versioned extraction
//! definition in `chapbook_core::locator` (version 2).

use markup5ever::{local_name, ns, LocalName, QualName};

use super::tree::{Document, ElementData, Node, NodeData, NodeId};

/// Rewrite every MathML `<math>` subtree into its EPUB fallback form.
/// Runs once per document, after tree building and before `seal`.
pub(super) fn apply_mathml_fallback(doc: &mut Document) {
    let mut math_roots = Vec::new();
    collect_math_roots(doc, doc.root(), &mut math_roots);
    for id in math_roots {
        rewrite(doc, id);
    }
}

/// Outermost `<math>` elements in document order. Does not descend into a
/// found subtree: nested math (via `annotation-xml`) is covered by the
/// outermost rewrite.
fn collect_math_roots(doc: &Document, id: NodeId, out: &mut Vec<NodeId>) {
    if is_mathml(doc, id, &local_name!("math")) {
        out.push(id);
        return;
    }
    for child in doc.node(id).children.clone() {
        collect_math_roots(doc, child, out);
    }
}

fn is_mathml(doc: &Document, id: NodeId, name: &LocalName) -> bool {
    match &doc.node(id).data {
        NodeData::Element(e) => e.name.ns == ns!(mathml) && e.name.local == *name,
        _ => false,
    }
}

fn rewrite(doc: &mut Document, id: NodeId) {
    let NodeData::Element(el) = &doc.node(id).data else {
        return;
    };
    // `altimg`/`alttext` are not in the static atom set, so no `local_name!`.
    let non_empty = |name: &str| {
        el.attr(&LocalName::from(name))
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string)
    };
    let altimg = non_empty("altimg");
    let alttext = non_empty("alttext");

    if let Some(src) = altimg {
        // Keep the original attributes (id/class/style survive; the inert
        // MathML ones are harmless), minus any src/alt that would shadow
        // the ones we synthesize.
        let mut attrs: Vec<(QualName, String)> = el
            .attrs
            .iter()
            .filter(|(n, _)| {
                !(n.ns.is_empty()
                    && (n.local == local_name!("src") || n.local == local_name!("alt")))
            })
            .cloned()
            .collect();
        attrs.push((QualName::new(None, ns!(), local_name!("src")), src));
        if let Some(alt) = alttext {
            attrs.push((QualName::new(None, ns!(), local_name!("alt")), alt));
        }
        doc.nodes[id].data = NodeData::Element(ElementData::new(
            QualName::new(None, ns!(html), local_name!("img")),
            attrs,
        ));
        detach_children(doc, id);
    } else if let Some(text) = alttext {
        detach_children(doc, id);
        let child = doc.nodes.insert(Node::new(Some(id), NodeData::Text(text)));
        doc.nodes[id].children.push(child);
    } else {
        strip_annotations(doc, id);
    }
}

fn detach_children(doc: &mut Document, id: NodeId) {
    let children = std::mem::take(&mut doc.nodes[id].children);
    for child in children {
        doc.nodes[child].parent = None;
    }
}

/// Drop `annotation`/`annotation-xml` subtrees under a fallback-less
/// `<math>`, leaving only the presentation markup to flatten.
fn strip_annotations(doc: &mut Document, id: NodeId) {
    let children = doc.node(id).children.clone();
    for child in children {
        if is_mathml(doc, child, &LocalName::from("annotation"))
            || is_mathml(doc, child, &LocalName::from("annotation-xml"))
        {
            doc.nodes[child].parent = None;
            let idx = doc.nodes[id].children.iter().position(|c| *c == child);
            if let Some(idx) = idx {
                doc.nodes[id].children.remove(idx);
            }
        } else {
            strip_annotations(doc, child);
        }
    }
}
