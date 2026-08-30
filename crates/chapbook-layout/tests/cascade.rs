//! Cascade correctness: origins, specificity, inheritance, style attributes,
//! reader settings. Golden dumps over the fixture EPUB live in the CLI crate.

use chapbook_core::{PageMetrics, ReadingSettings};
use chapbook_layout::cascade::StyleEngine;
use chapbook_layout::dom::parse_xhtml;

fn styled(html: &str, author_css: &[&str]) -> (chapbook_layout::dom::Document, StyleEngine) {
    let mut doc = parse_xhtml(html.as_bytes(), "test.xhtml").unwrap();
    let mut engine = StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
    let css: Vec<String> = author_css.iter().map(|s| s.to_string()).collect();
    engine.set_author_sheets(&css);
    engine.style_document(&mut doc);
    (doc, engine)
}

fn dump_of(doc: &chapbook_layout::dom::Document) -> String {
    chapbook_layout::cascade::dump_computed_styles(doc)
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

// ---- The reader's chosen typeface ----

/// A helper that styles one document against settings of the caller's
/// choosing, so the font-family tests can vary the one field.
fn styled_with(
    html: &str,
    author_css: &[&str],
    settings: &ReadingSettings,
) -> chapbook_layout::dom::Document {
    let mut doc = parse_xhtml(html.as_bytes(), "test.xhtml").unwrap();
    let mut engine = StyleEngine::new(&PageMetrics::default(), settings);
    let css: Vec<String> = author_css.iter().map(|s| s.to_string()).collect();
    engine.set_author_sheets(&css);
    engine.style_document(&mut doc);
    doc
}

fn line_for<'a>(dump: &'a str, tag: &str) -> &'a str {
    dump.lines()
        .find(|l| l.trim_start().starts_with(tag))
        .unwrap_or_else(|| panic!("no {tag} in dump:\n{dump}"))
}

/// The whole point of the setting. Nearly every real EPUB sets
/// `body { font-family }`, so a polite user-origin rule would be ignored
/// on nearly every book and the reader's choice would do nothing.
#[test]
fn a_chosen_font_family_beats_the_publishers() {
    let settings = ReadingSettings {
        font_family: Some("Chosen Serif".into()),
        ..Default::default()
    };
    let doc = styled_with(
        "<html><body><p>hi</p></body></html>",
        &["body { font-family: 'Publisher Sans'; }"],
        &settings,
    );
    let dump = dump_of(&doc);
    let p = line_for(&dump, "<p>");
    assert!(p.contains("Chosen Serif"), "{p}");
    assert!(!p.contains("Publisher Sans"), "{p}");
}

/// A code listing reflowed into the reader's serif is a bug people report,
/// not a preference they expressed. The UA sheet already gives these the
/// monospace generic and the choice must not take it away.
#[test]
fn monospace_elements_keep_their_font() {
    let settings = ReadingSettings {
        font_family: Some("Chosen Serif".into()),
        ..Default::default()
    };
    let doc = styled_with(
        "<html><body><p>hi</p><pre>x</pre><code>y</code></body></html>",
        &[],
        &settings,
    );
    let dump = dump_of(&doc);
    for tag in ["<pre>", "<code>"] {
        let line = line_for(&dump, tag);
        assert!(
            !line.contains("Chosen Serif"),
            "{tag} took the reader's body font: {line}"
        );
    }
    assert!(line_for(&dump, "<p>").contains("Chosen Serif"));
}

/// `None` is the publisher's font, and it has to be genuinely inert —
/// this is the default every existing book and every pre-v5 settings row
/// carries.
#[test]
fn no_chosen_family_leaves_the_publisher_alone() {
    let doc = styled_with(
        "<html><body><p>hi</p></body></html>",
        &["body { font-family: 'Publisher Sans'; }"],
        &ReadingSettings::default(),
    );
    let dump = dump_of(&doc);
    assert!(line_for(&dump, "<p>").contains("Publisher Sans"));
}

/// A family name is data, and it reaches a CSS parser. A name carrying a
/// quote or a brace would otherwise close the rule and open whatever came
/// after it — so it is quoted and escaped, and the worst case is a family
/// nothing matches rather than a stylesheet the reader wrote by accident.
///
/// The payload sets `color`, because that is a property this rule has no
/// business touching: if it lands, the escaping failed. Asserting on the
/// family text itself would not work — the payload *is* family text, so it
/// appears in the computed value either way.
#[test]
fn a_family_name_cannot_escape_its_own_rule() {
    let settings = ReadingSettings {
        font_family: Some(r#"Evil"; } * { color: rgb(9, 9, 9); } x { font-family: "z"#.into()),
        ..Default::default()
    };
    let doc = styled_with("<html><body><p>hi</p></body></html>", &[], &settings);
    let dump = dump_of(&doc);
    let p = line_for(&dump, "<p>");
    assert!(
        p.contains("color: rgb(0, 0, 0)"),
        "the family name injected a declaration: {p}"
    );
}

/// The `:root`-versus-`*` trap, kept honest by a fixture that styles the
/// element publishers actually style. A rule on `:root` passes a test
/// whose author sheet targets `html` and loses on every real book.
#[test]
fn a_chosen_family_reaches_elements_the_publisher_styled_directly() {
    let settings = ReadingSettings {
        font_family: Some("Chosen Serif".into()),
        ..Default::default()
    };
    let doc = styled_with(
        "<html><body><div><p>hi</p></div></body></html>",
        &[
            "body { font-family: 'Publisher Sans'; }",
            "div { font-family: 'Publisher Display'; }",
            "p { font-family: 'Publisher Text'; }",
        ],
        &settings,
    );
    let dump = dump_of(&doc);
    for tag in ["<div>", "<p>"] {
        let line = line_for(&dump, tag);
        assert!(
            line.contains("Chosen Serif"),
            "{tag} kept a publisher font: {line}"
        );
    }
}

/// A `<span>` inside a `<pre>` is still code. Without the descendant half
/// of the monospace exemption, `*` would claim it and the listing would
/// come apart mid-line.
#[test]
fn markup_inside_a_code_block_stays_monospace() {
    let settings = ReadingSettings {
        font_family: Some("Chosen Serif".into()),
        ..Default::default()
    };
    let doc = styled_with(
        "<html><body><pre><span>x</span></pre></body></html>",
        &[],
        &settings,
    );
    let dump = dump_of(&doc);
    let span = line_for(&dump, "<span>");
    assert!(!span.contains("Chosen Serif"), "{span}");
}

/// An all-whitespace name is not a choice. Treated as `None` rather than
/// emitted, so it cannot produce a rule naming nothing.
#[test]
fn a_blank_family_name_is_not_a_choice() {
    let settings = ReadingSettings {
        font_family: Some("   ".into()),
        ..Default::default()
    };
    let doc = styled_with(
        "<html><body><p>hi</p></body></html>",
        &["body { font-family: 'Publisher Sans'; }"],
        &settings,
    );
    let dump = dump_of(&doc);
    assert!(line_for(&dump, "<p>").contains("Publisher Sans"));
}
