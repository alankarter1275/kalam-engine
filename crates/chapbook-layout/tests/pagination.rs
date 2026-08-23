//! Pagination behavior: forced breaks, break-inside avoidance,
//! widows/orphans, and the layout invariants (containment, text order,
//! conservation).

use std::path::PathBuf;

use chapbook_core::{EdgeSizes, PageMetrics, ReadingSettings, Size};
use chapbook_dom::Document;
use chapbook_layout::ChapterLayout;
use chapbook_paint::FragmentKind;

fn fonts() -> cosmic_text::FontSystem {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    chapbook_layout::fixture_font_system(&dir, "Crimson Text")
}

/// Page with room for exactly `lines` default lines (18px font, 1.5 line
/// height = 27px each).
fn page_for_lines(lines: u32) -> PageMetrics {
    PageMetrics {
        size: Size::new(600.0, lines as f32 * 27.0 + 80.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
    }
}

fn layout_html(html: &str, css: &str, page: &PageMetrics) -> (ChapterLayout, Document) {
    let mut doc = chapbook_dom::parse_xhtml(html.as_bytes(), "test.xhtml").unwrap();
    let css_sources = vec![css.to_string()];
    let mut engine = chapbook_style::StyleEngine::new(page, &ReadingSettings::default());
    engine.set_author_sheets(&css_sources);
    engine.style_document(&mut doc);
    let mut fonts = fonts();
    let layout = chapbook_layout::paginate(&doc, &css_sources, page, &mut fonts);
    (layout, doc)
}

fn line_texts_in_order(layout: &ChapterLayout) -> Vec<String> {
    layout
        .pages
        .iter()
        .flat_map(|p| p.fragments.iter())
        .filter_map(|f| match &f.kind {
            FragmentKind::Line(l) => Some(l.text.clone()),
            _ => None,
        })
        .collect()
}

fn lines_per_page(layout: &ChapterLayout) -> Vec<usize> {
    layout.pages.iter().map(|p| p.fragments.len()).collect()
}

/// N hard lines via <br>: line counts independent of font wrap behavior.
fn para_of_lines(n: usize, word: &str) -> String {
    let lines: Vec<String> = (0..n).map(|i| format!("{word} line {i}")).collect();
    format!("<p>{}</p>", lines.join("<br/>"))
}

#[test]
fn forced_break_before_starts_new_page() {
    let html = format!(
        "<html><body>{}{}</body></html>",
        para_of_lines(2, "first"),
        r#"<p class="pb">after the break</p>"#
    );
    let (layout, _) = layout_html(
        &html,
        ".pb { page-break-before: always; }",
        &page_for_lines(10),
    );
    assert_eq!(layout.pages.len(), 2);
    let texts = line_texts_in_order(&layout);
    assert!(texts.last().unwrap().contains("after the break"));
    assert_eq!(layout.pages[1].fragments.len(), 1);
}

#[test]
fn break_after_forces_page_for_next_block() {
    let html = "<html><body><h1>Title</h1><p>body text</p></body></html>";
    let (layout, _) = layout_html(html, "h1 { break-after: page; }", &page_for_lines(10));
    assert_eq!(layout.pages.len(), 2);
    assert!(matches!(&layout.pages[0].fragments[0].kind,
        FragmentKind::Line(l) if l.text == "Title"));
    assert!(matches!(&layout.pages[1].fragments[0].kind,
        FragmentKind::Line(l) if l.text == "body text"));
}

#[test]
fn long_paragraph_fills_and_breaks() {
    let html = format!("<html><body>{}</body></html>", para_of_lines(10, "w"));
    let (layout, _) = layout_html(&html, "p { margin: 0; }", &page_for_lines(4));
    assert_eq!(lines_per_page(&layout), vec![4, 4, 2]);
}

#[test]
fn orphans_move_paragraph_start_to_next_page() {
    // Page holds 4 lines. First paragraph takes 3, leaving room for 1 line
    // of the second — fewer than orphans(2), so it moves entirely.
    let html = format!(
        "<html><body>{}{}</body></html>",
        para_of_lines(3, "first"),
        para_of_lines(3, "second")
    );
    let (layout, _) = layout_html(&html, "p { margin: 0; }", &page_for_lines(4));
    assert_eq!(lines_per_page(&layout), vec![3, 3]);
}

#[test]
fn widows_pull_extra_line_to_next_page() {
    // Page holds 4 lines; 5-line paragraph would leave 1 widow — the break
    // moves up so the next page gets 2 lines.
    let html = format!("<html><body>{}</body></html>", para_of_lines(5, "w"));
    let (layout, _) = layout_html(&html, "p { margin: 0; }", &page_for_lines(4));
    assert_eq!(lines_per_page(&layout), vec![3, 2]);
}

#[test]
fn widows_orphans_custom_values_respected() {
    let html = format!("<html><body>{}</body></html>", para_of_lines(6, "w"));
    let (layout, _) = layout_html(
        &html,
        "p { margin: 0; widows: 3; orphans: 3; }",
        &page_for_lines(4),
    );
    // 6 lines, page of 4: plain fill would be 4+2, but widows:3 forces 3+3.
    assert_eq!(lines_per_page(&layout), vec![3, 3]);
}

#[test]
fn break_inside_avoid_moves_whole_block() {
    // First paragraph takes 2 of 4 lines; the second (3 lines,
    // break-inside: avoid) doesn't fit in the remaining 2 → whole block
    // moves to page 2 instead of splitting.
    let keep_para = format!(
        "<p class=\"keep\">{}</p>",
        (0..3)
            .map(|i| format!("keep line {i}"))
            .collect::<Vec<_>>()
            .join("<br/>")
    );
    let html = format!(
        "<html><body>{}{keep_para}</body></html>",
        para_of_lines(2, "first"),
    );
    let (layout, _) = layout_html(
        &html,
        "p { margin: 0; } .keep { break-inside: avoid; }",
        &page_for_lines(4),
    );
    assert_eq!(lines_per_page(&layout), vec![2, 3]);
}

#[test]
fn margins_discarded_at_page_top() {
    // Two paragraphs with big margins; the second starts a fresh page — its
    // top margin must not push it down the fresh page.
    let html = format!(
        "<html><body>{}{}</body></html>",
        para_of_lines(4, "first"),
        para_of_lines(2, "second")
    );
    let (layout, _) = layout_html(&html, "p { margin: 30px 0; }", &page_for_lines(4));
    assert_eq!(layout.pages.len(), 2);
    let first_on_page2 = &layout.pages[1].fragments[0];
    assert!(
        (first_on_page2.rect.origin.y - 40.0).abs() < 0.5,
        "expected content at page top, got y={}",
        first_on_page2.rect.origin.y
    );
}

// ---- Invariants over the fixture book ----

fn fixture_book_layout(spine: usize) -> (ChapterLayout, Document, String) {
    use chapbook_core::Publication;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/epub/minimal.epub");
    let book = chapbook_epub::Book::open(&path).unwrap();
    let href = book.spine_item(spine).unwrap().href.clone();
    let bytes = book.unit_bytes(spine).unwrap();
    let mut doc = chapbook_dom::parse_xhtml(&bytes, &href).unwrap();
    let css: Vec<String> = doc
        .stylesheet_sources()
        .iter()
        .filter_map(|s| match s {
            chapbook_dom::StylesheetSource::Inline(t) => Some(t.clone()),
            chapbook_dom::StylesheetSource::External(rel) => book
                .resource(&href, rel)
                .ok()
                .map(|r| String::from_utf8_lossy(&r.data).into_owned()),
        })
        .collect();
    let page = PageMetrics::default();
    let mut engine = chapbook_style::StyleEngine::new(&page, &ReadingSettings::default());
    engine.set_author_sheets(&css);
    engine.style_document(&mut doc);
    let mut fonts = fonts();
    let layout = chapbook_layout::paginate(&doc, &css, &page, &mut fonts);
    let display_text = chapbook_dom::extract_text(&doc);
    (layout, doc, display_text)
}

fn strip_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    needle.chars().all(|n| hay.by_ref().any(|h| h == n))
}

#[test]
fn invariants_on_fixture_chapters() {
    for spine in 0..2 {
        let (layout, _doc, display_text) = fixture_book_layout(spine);

        // 1. Every fragment inside the page content box.
        for (i, page) in layout.pages.iter().enumerate() {
            for frag in &page.fragments {
                assert!(
                    page.content.contains_rect(&frag.rect),
                    "spine {spine} page {i}: fragment {:?} outside content {:?}",
                    frag.rect,
                    page.content
                );
            }
        }

        // 2. Locator offsets never decrease across the fragment stream, and
        //    the char_map is monotonic.
        let mut last = 0u32;
        for page in &layout.pages {
            for frag in &page.fragments {
                if let FragmentKind::Line(l) = &frag.kind {
                    assert!(l.locator_start >= last, "locator order violated");
                    last = l.locator_start;
                }
            }
        }
        assert!(layout.char_map.windows(2).all(|w| w[0] <= w[1]));

        // 3. Conservation: every visible char of the extracted display text
        //    appears, in order, in the laid-out lines (which may add list
        //    markers but never drop or reorder text).
        let laid_out: String = layout
            .pages
            .iter()
            .flat_map(|p| &p.fragments)
            .filter_map(|f| match &f.kind {
                FragmentKind::Line(l) => Some(l.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            is_subsequence(&strip_ws(&display_text), &strip_ws(&laid_out)),
            "spine {spine}: text lost or reordered in layout"
        );
    }
}
