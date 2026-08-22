use chapbook_dom::{extract_text, parse_xhtml, NodeData};

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
        chapbook_dom::StylesheetSource::Inline(css) => assert!(css.contains("color: red")),
        _ => panic!("first source should be the <style> element"),
    }
    match &sources[1] {
        chapbook_dom::StylesheetSource::External(href) => {
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
