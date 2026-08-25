//! Deterministic computed-style dump — the M2 golden-test surface and the
//! `chapbook styles` CLI output. Format changes here invalidate snapshots.

use style::properties::{LonghandId, PropertyDeclarationId};

use crate::dom::{Document, NodeData, NodeId};

/// Properties shown per element: the set that drives book layout.
const DUMPED: &[(&str, LonghandId)] = &[
    ("display", LonghandId::Display),
    ("font-size", LonghandId::FontSize),
    ("font-weight", LonghandId::FontWeight),
    ("font-style", LonghandId::FontStyle),
    ("font-family", LonghandId::FontFamily),
    ("line-height", LonghandId::LineHeight),
    ("color", LonghandId::Color),
    ("margin-top", LonghandId::MarginTop),
    ("margin-right", LonghandId::MarginRight),
    ("margin-bottom", LonghandId::MarginBottom),
    ("margin-left", LonghandId::MarginLeft),
    ("text-align", LonghandId::TextAlign),
    ("text-indent", LonghandId::TextIndent),
    // NOTE: no break-before/after/inside, widows, or orphans — servo-mode
    // stylo does not implement the fragmentation properties (they are
    // Gecko-only). M3's pagination gets them from a sidecar cascade in
    // chapbook-layout that parses just those declarations and matches their
    // selectors via our own `selectors::Element` impl.
];

/// Walk the styled document and render one line per element with its
/// computed values for the properties in [`DUMPED`]. The `<head>` subtree is
/// skipped (computed `display: none` per the UA sheet; nothing to lay out).
pub fn dump_computed_styles(doc: &Document) -> String {
    let mut out = String::new();
    if let Some(root) = doc.document_element() {
        walk(doc, root, 0, &mut out);
    }
    out
}

fn walk(doc: &Document, id: NodeId, depth: usize, out: &mut String) {
    let node = doc.node(id);
    let NodeData::Element(el) = &node.data else {
        return;
    };
    if *el.local_name() == markup5ever::local_name!("head") {
        return;
    }

    let mut label = format!("<{}>", el.name.local);
    if let Some(id_attr) = &el.id {
        label.push('#');
        label.push_str(id_attr);
    }
    for class in &el.classes {
        label.push('.');
        label.push_str(class);
    }

    out.push_str(&"  ".repeat(depth));
    out.push_str(&label);

    match doc.primary_styles(id) {
        Some(style) => {
            out.push_str(" {");
            for (i, (name, longhand)) in DUMPED.iter().enumerate() {
                if i > 0 {
                    out.push_str("; ");
                }
                let value =
                    style.computed_value_to_string(PropertyDeclarationId::Longhand(*longhand));
                out.push_str(name);
                out.push_str(": ");
                out.push_str(&value);
            }
            out.push_str("}\n");
        }
        None => out.push_str(" {unstyled}\n"),
    }

    for child in &node.children {
        walk(doc, *child, depth + 1, out);
    }
}
