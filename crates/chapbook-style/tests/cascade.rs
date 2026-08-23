//! Cascade correctness: origins, specificity, inheritance, style attributes,
//! reader settings. Golden dumps over the fixture EPUB live in the CLI crate.

use chapbook_core::{PageMetrics, ReadingSettings};
use chapbook_dom::parse_xhtml;
use chapbook_style::StyleEngine;

fn styled(html: &str, author_css: &[&str]) -> (chapbook_dom::Document, StyleEngine) {
    let mut doc = parse_xhtml(html.as_bytes(), "test.xhtml").unwrap();
    let mut engine = StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
    let css: Vec<String> = author_css.iter().map(|s| s.to_string()).collect();
    engine.set_author_sheets(&css);
    engine.style_document(&mut doc);
    (doc, engine)
}

fn dump_of(doc: &chapbook_dom::Document) -> String {
    chapbook_style::dump_computed_styles(doc)
}

#[test]
fn style_attribute_beats_author_sheet() {
    let (doc, _) = styled(
        r#"<html><body><p id="x" style="color: rgb(1, 2, 3)">hi</p></body></html>"#,
        &["p { color: rgb(9, 9, 9); }"],
    );
    let dump = dump_of(&doc);
    let p_line = dump.lines().find(|l| l.contains("<p>#x")).unwrap();
    assert!(p_line.contains("color: rgb(1, 2, 3)"), "{p_line}");
}

#[test]
fn id_beats_class_beats_element() {
    let (doc, _) = styled(
        r#"<html><body><p id="x" class="c">hi</p></body></html>"#,
        &[
            "p { color: rgb(1, 1, 1); }",
            ".c { color: rgb(2, 2, 2); }",
            "#x { color: rgb(3, 3, 3); }",
        ],
    );
    let dump = dump_of(&doc);
    let p_line = dump.lines().find(|l| l.contains("<p>#x.c")).unwrap();
    assert!(p_line.contains("color: rgb(3, 3, 3)"), "{p_line}");
}

#[test]
fn descendant_and_child_combinators() {
    let (doc, _) = styled(
        r#"<html><body><div class="a"><section><p>deep</p></section><p id="direct">child</p></div></body></html>"#,
        &[
            ".a p { color: rgb(1, 1, 1); }",
            ".a > p { color: rgb(2, 2, 2); }",
        ],
    );
    let dump = dump_of(&doc);
    let deep = dump
        .lines()
        .find(|l| l.trim_start().starts_with("<p> "))
        .unwrap();
    let direct = dump.lines().find(|l| l.contains("<p>#direct")).unwrap();
    assert!(deep.contains("rgb(1, 1, 1)"), "{deep}");
    // Equal specificity: the later child-combinator rule wins on #direct.
    assert!(direct.contains("rgb(2, 2, 2)"), "{direct}");
}

#[test]
fn em_resolution_against_reader_base_size() {
    let mut doc = parse_xhtml(b"<html><body><p>hi</p></body></html>", "test.xhtml").unwrap();
    let settings = ReadingSettings {
        base_font_px: 20.0,
        line_height: 2.0,
        ..Default::default()
    };
    let mut engine = StyleEngine::new(&PageMetrics::default(), &settings);
    engine.set_author_sheets(&["p { margin-top: 1.5em; }".to_string()]);
    engine.style_document(&mut doc);
    let dump = dump_of(&doc);
    let p_line = dump
        .lines()
        .find(|l| l.trim_start().starts_with("<p>"))
        .unwrap();
    assert!(p_line.contains("font-size: 20px"), "{p_line}");
    assert!(p_line.contains("line-height: 40px"), "{p_line}");
    assert!(p_line.contains("margin-top: 30px"), "{p_line}");
}

#[test]
fn user_sheet_important_beats_author() {
    let mut doc = parse_xhtml(b"<html><body><p>hi</p></body></html>", "test.xhtml").unwrap();
    let mut engine = StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
    engine.add_user_sheet("p { color: rgb(5, 5, 5) !important; }");
    engine.set_author_sheets(&["p { color: rgb(9, 9, 9); }".to_string()]);
    engine.style_document(&mut doc);
    let dump = dump_of(&doc);
    let p_line = dump
        .lines()
        .find(|l| l.trim_start().starts_with("<p>"))
        .unwrap();
    assert!(p_line.contains("rgb(5, 5, 5)"), "{p_line}");
}

#[test]
fn hidden_attribute_computes_display_none() {
    let (doc, _) = styled(
        r#"<html><body><p hidden id="h">gone</p></body></html>"#,
        &[],
    );
    let dump = dump_of(&doc);
    let line = dump.lines().find(|l| l.contains("<p>#h")).unwrap();
    assert!(line.contains("display: none"), "{line}");
}

#[test]
fn one_engine_styles_many_documents() {
    let mut engine = StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
    for i in 0..3 {
        let html = format!("<html><body><p class=\"c{i}\">doc {i}</p></body></html>");
        let mut doc = parse_xhtml(html.as_bytes(), "t.xhtml").unwrap();
        let css = format!(".c{i} {{ color: rgb({i}, {i}, {i}); }}");
        engine.set_author_sheets(&[css]);
        engine.style_document(&mut doc);
        let dump = dump_of(&doc);
        let p_line = dump
            .lines()
            .find(|l| l.contains(&format!("<p>.c{i}")))
            .unwrap();
        assert!(
            p_line.contains(&format!("rgb({i}, {i}, {i})")),
            "doc {i}: {p_line}"
        );
    }
}
