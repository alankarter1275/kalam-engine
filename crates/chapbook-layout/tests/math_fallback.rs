//! The MathML fallback rewrite: chapbook lays out no math; `<math>`
//! subtrees become their EPUB 3 altimg/alttext fallback at parse time
//! (`chapbook_layout::dom` internals, observable through the parse API).

use chapbook_layout::dom::{extract_text, locator_text, parse_xhtml, Document, NodeData, NodeId};

fn parse(body: &str) -> Document {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>t</title></head>
<body>{body}</body>
</html>"#
    );
    parse_xhtml(doc.as_bytes(), "OEBPS/ch.xhtml").unwrap()
}

/// First element with the given local name, any namespace.
fn find(doc: &Document, name: &str) -> Option<NodeId> {
    doc.descendants(doc.root())
        .find(|id| match &doc.node(*id).data {
            NodeData::Element(el) => &**el.local_name() == name,
            _ => false,
        })
}

#[test]
fn altimg_becomes_an_img() {
    let doc = parse(
        r#"<p>Euler: <math xmlns="http://www.w3.org/1998/Math/MathML"
              altimg="../images/euler.png" alttext="e^(i pi) + 1 = 0" id="eq1">
              <msup><mi>e</mi><mrow><mi>i</mi><mi>&#960;</mi></mrow></msup>
              <mo>+</mo><mn>1</mn><mo>=</mo><mn>0</mn>
            </math> as shipped.</p>"#,
    );
    assert!(
        find(&doc, "math").is_none(),
        "math should be rewritten away"
    );
    let img = find(&doc, "img").expect("altimg should synthesize an img");
    assert!(doc.is_html_element(img, &markup5ever::local_name!("img")));
    let NodeData::Element(el) = &doc.node(img).data else {
        unreachable!()
    };
    assert_eq!(
        el.attr(&markup5ever::local_name!("src")),
        Some("../images/euler.png")
    );
    assert_eq!(
        el.attr(&markup5ever::local_name!("alt")),
        Some("e^(i pi) + 1 = 0")
    );
    // The original id survives, so anchor jumps to the equation work.
    assert_eq!(doc.element_by_id("eq1"), Some(img));
    assert!(doc.node(img).children.is_empty());
    // Neither token soup nor alttext reaches the text spaces — `alt` never
    // counts (chapbook_core::locator).
    let loc = locator_text(&doc);
    assert_eq!(loc.matches("Euler: ").count(), 1);
    assert!(!loc.contains("+1=0"));
    assert!(!loc.contains("e^(i pi)"));
    assert!(loc.contains("Euler:  as shipped."));
}

#[test]
fn alttext_without_altimg_becomes_inline_text() {
    let doc = parse(
        r#"<p>One half is <math xmlns="http://www.w3.org/1998/Math/MathML"
              alttext="1/2"><mfrac><mn>1</mn><mn>2</mn></mfrac>
            </math> exactly.</p>"#,
    );
    // The flattened token text would read "12" — the alttext replaces it in
    // both the locator space and display text.
    let loc = locator_text(&doc);
    assert!(loc.contains("One half is 1/2 exactly."));
    assert!(!loc.contains("12"));
    assert!(extract_text(&doc).contains("One half is 1/2 exactly."));
}

#[test]
fn empty_alttext_falls_through_to_flattened_content() {
    let doc = parse(
        r#"<p>Sum <math xmlns="http://www.w3.org/1998/Math/MathML" alttext="  ">
              <mi>a</mi><mo>+</mo><mi>b</mi></math> end.</p>"#,
    );
    assert!(locator_text(&doc).contains("a+b"));
}

#[test]
fn no_fallback_flattens_but_drops_annotations() {
    let doc = parse(
        r#"<p>Ratio <math xmlns="http://www.w3.org/1998/Math/MathML">
              <semantics>
                <mfrac><mi>a</mi><mi>b</mi></mfrac>
                <annotation encoding="application/x-tex">\frac{a}{b}</annotation>
                <annotation-xml encoding="MathML-Content">
                  <apply><divide/><ci>a</ci><ci>b</ci></apply>
                </annotation-xml>
              </semantics>
            </math> end.</p>"#,
    );
    let loc = locator_text(&doc);
    // Last resort: presentation tokens flatten into the line...
    assert!(loc.contains('a') && loc.contains('b'));
    // ...but alternate encodings must not leak.
    assert!(!loc.contains("frac"), "LaTeX annotation leaked: {loc:?}");
    assert!(find(&doc, "annotation").is_none());
    assert!(find(&doc, "annotation-xml").is_none());
}

#[test]
fn svg_subtrees_are_untouched() {
    let doc = parse(
        r#"<p>Fig <svg xmlns="http://www.w3.org/2000/svg" width="4" height="4">
              <title>dot</title></svg> end.</p>"#,
    );
    assert!(find(&doc, "svg").is_some());
}
