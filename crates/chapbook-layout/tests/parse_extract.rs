use chapbook_layout::dom::{extract_text, parse_xhtml, NodeData};

const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>Ignored Title</title>
  <style>p { color: red; }</style>
  <link rel="stylesheet" href="../styles/main.css"/>
</head>
<body>
  <h1>Heading</h1>
  <p id="intro" class="first  lead">Text with
     a source line-wrap and <em>inline emphasis</em>, plus an
     entity: caf&#233;.</p>
  <p>Second paragraph.<br/>After a forced break.</p>
</body>
</html>"#;

#[test]
fn text_extraction_collapses_whitespace_and_keeps_blocks() {
    let doc = parse_xhtml(DOC.as_bytes(), "OEBPS/text/ch.xhtml").unwrap();
    let text = extract_text(&doc);
    assert_eq!(
        text,
        "Heading\n\
         Text with a source line-wrap and inline emphasis, plus an entity: café.\n\
         Second paragraph.\nAfter a forced break.\n"
    );
}

#[test]
fn head_content_is_excluded() {
    let doc = parse_xhtml(DOC.as_bytes(), "OEBPS/text/ch.xhtml").unwrap();
    let text = extract_text(&doc);
    assert!(!text.contains("Ignored Title"));
    assert!(!text.contains("color: red"));
}

#[test]
fn id_and_class_caches() {
    let doc = parse_xhtml(DOC.as_bytes(), "OEBPS/text/ch.xhtml").unwrap();
    let p = doc.element_by_id("intro").expect("intro paragraph present");
    let NodeData::Element(el) = &doc.node(p).data else {
        panic!("expected element");
    };
    assert_eq!(el.classes, vec!["first".to_string(), "lead".to_string()]);
}

#[test]
fn stylesheet_sources_in_document_order() {
    let doc = parse_xhtml(DOC.as_bytes(), "OEBPS/text/ch.xhtml").unwrap();
    let sources = doc.stylesheet_sources();
    assert_eq!(sources.len(), 2);
    match &sources[0] {
        chapbook_layout::dom::StylesheetSource::Inline(css) => assert!(css.contains("color: red")),
        _ => panic!("first source should be the <style> element"),
    }
    match &sources[1] {
        chapbook_layout::dom::StylesheetSource::External(href) => {
            assert_eq!(href, "../styles/main.css");
        }
        _ => panic!("second source should be the <link>"),
    }
}

#[test]
fn lenient_parsing_of_html_isms() {
    // Unclosed <p>, named entity, bare ampersand — all invalid XML, all
    // common in real EPUBs. The lenient parser must cope.
    let sloppy = "<html><body><p>One&nbsp;bit<p>Two &amp; three & four</body></html>";
    let doc = parse_xhtml(sloppy.as_bytes(), "ch.xhtml").unwrap();
    let text = extract_text(&doc);
    assert!(text.contains("One\u{a0}bit"));
    assert!(text.contains("Two & three & four"));
}

#[test]
fn arbitrary_bytes_do_not_panic() {
    let garbage: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let _ = parse_xhtml(&garbage, "junk.bin");
}

// kalam: the XML-first parse. The shapes below are lifted from a Penguin
// Random House EPUB whose every chapter rendered as one paragraph.

/// Element children of `id`, by local name, in order.
fn child_tags(
    doc: &chapbook_layout::dom::Document,
    id: chapbook_layout::dom::NodeId,
) -> Vec<String> {
    doc.node(id)
        .children
        .iter()
        .filter_map(|c| match &doc.node(*c).data {
            NodeData::Element(el) => Some(el.local_name().to_string()),
            _ => None,
        })
        .collect()
}

fn body_of(doc: &chapbook_layout::dom::Document) -> chapbook_layout::dom::NodeId {
    let html = doc.document_element().expect("html element");
    doc.node(html)
        .children
        .iter()
        .copied()
        .find(|id| matches!(&doc.node(*id).data, NodeData::Element(el) if &**el.local_name() == "body"))
        .expect("body element")
}

#[test]
fn self_closed_anchor_does_not_swallow_the_chapter() {
    // `<a id="…"/>` is an empty element in XML. The HTML algorithm has no
    // self-closing syntax for `<a>`, so it would open an anchor that never
    // closes and every paragraph would nest inside it as inline content.
    let xhtml = concat!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">"#,
        "<head><title>Nyxia</title></head>",
        r#"<body><a id="d1-d2s3d3s3"/><div class="page_top_padding">"#,
        r#"<span epub:type="pagebreak" id="page3" title="3"/>"#,
        r#"<p class="para-cda-alt-chap-pg sans">DAY 1, 8:47 A.M.</p>"#,
        r#"<p class="para-cda1 sans">Aboard <em class="char-i-alt">Genesis 11</em></p>"#,
        r#"<p class="para-pf">“You all know why you’re here.”</p>"#,
        r#"<p class="para-p">There are ten of us at the table.</p>"#,
        "</div></body></html>",
    );
    let doc = parse_xhtml(xhtml.as_bytes(), "OEBPS/c001.xhtml").unwrap();
    let body = body_of(&doc);
    assert_eq!(child_tags(&doc, body), vec!["a", "div"]);
    let anchor = doc.node(body).children[0];
    assert!(doc.node(anchor).children.is_empty(), "the anchor is empty");
    let div = doc.node(body).children[1];
    assert_eq!(child_tags(&doc, div), vec!["span", "p", "p", "p", "p"]);
    // Elements land in the XHTML namespace, where the UA sheet and the box
    // tree look for them.
    assert!(doc.is_html_element(div, &markup5ever::local_name!("div")));
    assert!(doc.is_html_element(doc.node(div).children[1], &markup5ever::local_name!("p")));
    // And the text still reads as four paragraphs.
    assert_eq!(
        extract_text(&doc),
        "DAY 1, 8:47 A.M.\nAboard Genesis 11\n“You all know why you’re here.”\nThere are ten of us at the table.\n"
    );
}

#[test]
fn xml_lang_beside_lang_does_not_demote_a_document_to_html() {
    // `xml:lang="en" lang="en"` on <html> is what the EPUB samples, calibre
    // and InDesign write. xml5ever 0.39 compares attribute names by local
    // name only and reports the pair as a duplicate; if that report sent
    // the document to the HTML parser, the self-closed anchor would
    // swallow the paragraph again — and it would for most books.
    let xhtml = concat!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en" lang="en">"#,
        r#"<body><a id="top"/><p>after the anchor</p></body></html>"#,
    );
    let doc = parse_xhtml(xhtml.as_bytes(), "ch.xhtml").unwrap();
    let body = body_of(&doc);
    assert_eq!(child_tags(&doc, body), vec!["a", "p"]);
    assert_eq!(extract_text(&doc), "after the anchor\n");
}

#[test]
fn xml_parse_keeps_ids_classes_and_stylesheets() {
    let doc = parse_xhtml(DOC.as_bytes(), "OEBPS/text/ch.xhtml").unwrap();
    let p = doc.element_by_id("intro").expect("intro paragraph present");
    let NodeData::Element(el) = &doc.node(p).data else {
        panic!("expected element");
    };
    assert_eq!(el.classes, vec!["first".to_string(), "lead".to_string()]);
    assert_eq!(doc.stylesheet_sources().len(), 2);
}

#[test]
fn malformed_xml_falls_back_to_the_html_parser() {
    // A named entity XML does not define, an unclosed <p>, a bare ampersand:
    // not well-formed, so the HTML algorithm takes over — and it still
    // reads as paragraphs.
    let sloppy = concat!(
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>"#,
        "<p>One&nbsp;bit<p>Two &amp; three & four</body></html>",
    );
    let doc = parse_xhtml(sloppy.as_bytes(), "ch.xhtml").unwrap();
    let body = body_of(&doc);
    assert_eq!(child_tags(&doc, body), vec!["p", "p"]);
    assert_eq!(extract_text(&doc), "One\u{a0}bit\nTwo & three & four\n");
}

#[test]
fn namespace_less_document_falls_back_to_the_html_parser() {
    // Well-formed XML, but no XHTML namespace: as XML its elements would
    // be in no namespace and match nothing. The HTML parser puts them
    // where the stylesheets expect them.
    let doc = parse_xhtml(b"<html><body><p>hi</p></body></html>", "ch.xhtml").unwrap();
    let body = body_of(&doc);
    assert!(doc.is_html_element(body, &markup5ever::local_name!("body")));
    assert_eq!(child_tags(&doc, body), vec!["p"]);
}

#[test]
fn xml_declaration_doctype_and_cdata_are_handled() {
    let xhtml = concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        "\n<!DOCTYPE html>\n",
        r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title>"#,
        "<style><![CDATA[p { color: red; }]]></style></head>",
        "<body><p>caf&#233; &amp; more</p><!-- note --></body></html>",
    );
    let doc = parse_xhtml(xhtml.as_bytes(), "ch.xhtml").unwrap();
    assert_eq!(extract_text(&doc), "café & more\n");
    match &doc.stylesheet_sources()[0] {
        chapbook_layout::dom::StylesheetSource::Inline(css) => {
            assert!(css.contains("color: red"), "css was {css:?}")
        }
        _ => panic!("expected the <style> element"),
    }
}
