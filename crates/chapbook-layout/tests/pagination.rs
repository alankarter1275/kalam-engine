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
    let layout = chapbook_layout::paginate(
        &doc,
        &css_sources,
        page,
        &mut fonts,
        &chapbook_paint::ImageStore::default(),
    );
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
    let layout = chapbook_layout::paginate(
        &doc,
        &css,
        &page,
        &mut fonts,
        &chapbook_paint::ImageStore::default(),
    );
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

// ---- Typography fidelity sweep ----

#[test]
fn text_indent_shifts_first_line_only() {
    let html = "<html><body><p>one two three four five six seven eight nine ten \
                eleven twelve thirteen fourteen fifteen sixteen seventeen</p></body></html>";
    let (layout, _) = layout_html(
        html,
        "p { margin: 0; text-indent: 36px; }",
        &page_for_lines(10),
    );
    let lines: Vec<&chapbook_paint::Fragment> = layout.pages[0].fragments.iter().collect();
    assert!(lines.len() >= 2, "paragraph must wrap");
    let first_x = lines[0].rect.origin.x;
    let second_x = lines[1].rect.origin.x;
    assert!(
        (first_x - (second_x + 36.0)).abs() < 0.5,
        "first line indented by 36px: first={first_x} second={second_x}"
    );
}

#[test]
fn generated_content_wraps_element_text() {
    let html = r#"<html><body><p class="q">quoted</p></body></html>"#;
    let (layout, _) = layout_html(
        html,
        r#".q::before { content: "« "; } .q::after { content: " »"; }"#,
        &page_for_lines(10),
    );
    let texts = line_texts_in_order(&layout);
    assert_eq!(texts, vec!["« quoted »"]);
}

#[test]
fn letter_spacing_widens_lines() {
    let html = "<html><body><p>letter spacing sample</p></body></html>";
    let (plain, _) = layout_html(html, "p { margin: 0; }", &page_for_lines(10));
    let (spaced, _) = layout_html(
        html,
        "p { margin: 0; letter-spacing: 2px; }",
        &page_for_lines(10),
    );
    let width = |l: &ChapterLayout| l.pages[0].fragments[0].rect.size.w;
    assert!(
        width(&spaced) > width(&plain) + 10.0,
        "tracking must widen the line: plain={} spaced={}",
        width(&plain),
        width(&spaced)
    );
}

#[test]
fn box_decoration_slices_across_pages() {
    // A bordered block tall enough to span two pages: one Box fragment per
    // page, top edge only on the first slice, bottom only on the last.
    let html = format!(
        "<html><body><div class=\"framed\">{}</div></body></html>",
        para_of_lines(6, "boxed")
    );
    let (layout, _) = layout_html(
        &html,
        ".framed { border: 2px solid #000; background-color: #eee; } p { margin: 0; }",
        &page_for_lines(4),
    );
    assert_eq!(layout.pages.len(), 2);
    let slices: Vec<&chapbook_paint::BoxDecoration> = layout
        .pages
        .iter()
        .flat_map(|p| &p.fragments)
        .filter_map(|f| match &f.kind {
            chapbook_paint::FragmentKind::Box(b) => Some(b),
            _ => None,
        })
        .collect();
    assert_eq!(slices.len(), 2, "one slice per page");
    assert!(slices[0].first_slice && !slices[0].last_slice);
    assert!(!slices[1].first_slice && slices[1].last_slice);
    assert_eq!(
        slices[0].background,
        Some(chapbook_core::Rgba::new(0xee, 0xee, 0xee, 255))
    );
    assert_eq!(slices[0].border_widths.top, 2.0);

    // The background paints under the page's text: Box fragment comes first.
    assert!(matches!(
        layout.pages[0].fragments[0].kind,
        chapbook_paint::FragmentKind::Box(_)
    ));
    assert!(matches!(
        layout.pages[1].fragments[0].kind,
        chapbook_paint::FragmentKind::Box(_)
    ));
}

#[test]
fn undecorated_blocks_emit_no_box_fragments() {
    let html = format!("<html><body>{}</body></html>", para_of_lines(2, "plain"));
    let (layout, _) = layout_html(&html, "p { margin: 0; }", &page_for_lines(4));
    assert!(layout
        .pages
        .iter()
        .flat_map(|p| &p.fragments)
        .all(|f| !matches!(f.kind, chapbook_paint::FragmentKind::Box(_))));
}

// ---- Tables ----

fn cell_lines(layout: &ChapterLayout) -> Vec<(f32, f32, String)> {
    layout
        .pages
        .iter()
        .flat_map(|p| &p.fragments)
        .filter_map(|f| match &f.kind {
            FragmentKind::Line(l) => Some((f.rect.origin.x, f.rect.origin.y, l.text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn table_columns_lay_out_side_by_side() {
    let html = r#"<html><body><table>
        <tr><td>a</td><td>considerably longer cell content here</td></tr>
        <tr><td>b</td><td>short</td></tr>
    </table></body></html>"#;
    let (layout, _) = layout_html(html, "td { padding: 2px; }", &page_for_lines(10));
    let lines = cell_lines(&layout);
    let a = lines.iter().find(|(_, _, t)| t == "a").unwrap();
    let long = lines
        .iter()
        .find(|(_, _, t)| t.starts_with("considerably"))
        .unwrap();
    let b = lines.iter().find(|(_, _, t)| t == "b").unwrap();
    // Same row: same y, different x; column B right of A.
    assert!((a.1 - long.1).abs() < 0.5, "row cells align vertically");
    assert!(long.0 > a.0 + 5.0, "second column right of first");
    // Column x stable across rows.
    assert!((a.0 - b.0).abs() < 0.5, "first column x consistent");
    // Long content did not wrap: column got its max-content width.
    assert_eq!(
        lines
            .iter()
            .filter(|(_, _, t)| t.contains("longer cell"))
            .count(),
        1
    );
}

#[test]
fn table_narrow_page_wraps_long_column() {
    let html = r#"<html><body><table>
        <tr><td>key</td><td>a rather long value that cannot fit on one narrow line at all</td></tr>
    </table></body></html>"#;
    let mut page = page_for_lines(10);
    page.size.w = 300.0;
    let (layout, _) = layout_html(html, "", &page);
    let lines = cell_lines(&layout);
    let value_lines = lines.iter().filter(|(_, _, t)| t != "key").count();
    assert!(
        value_lines >= 2,
        "long column must wrap when space is short"
    );
    // Everything stays inside the content box.
    for page in &layout.pages {
        for frag in &page.fragments {
            assert!(page.content.contains_rect(&frag.rect), "{:?}", frag.rect);
        }
    }
}

#[test]
fn table_colspan_spans_columns() {
    let html = r#"<html><body><table>
        <tr><th colspan="2">Spanning Header</th></tr>
        <tr><td>left cell text</td><td>right cell text</td></tr>
    </table></body></html>"#;
    let (layout, _) = layout_html(
        html,
        "th, td { border: 1px solid #000; padding: 2px; }",
        &page_for_lines(10),
    );
    // Box fragments per cell: 3 (one spanning + two normal).
    let boxes: Vec<&chapbook_paint::Fragment> = layout
        .pages
        .iter()
        .flat_map(|p| &p.fragments)
        .filter(|f| matches!(f.kind, FragmentKind::Box(_)))
        .collect();
    assert_eq!(boxes.len(), 3);
    let spanning = boxes[0];
    let left = boxes[1];
    let right = boxes[2];
    let spanned = right.rect.max_x() - left.rect.origin.x;
    assert!(
        (spanning.rect.size.w - spanned).abs() < 1.0,
        "header spans both columns: header={} cells={spanned}",
        spanning.rect.size.w
    );
}

#[test]
fn table_rows_break_atomically_across_pages() {
    let mut rows = String::new();
    for i in 0..8 {
        rows.push_str(&format!("<tr><td>row {i} cell</td></tr>"));
    }
    let html = format!("<html><body><table>{rows}</table></body></html>");
    let (layout, _) = layout_html(&html, "td { padding: 0; }", &page_for_lines(4));
    assert!(layout.pages.len() >= 2, "table must paginate");
    // No row's text is split across pages: each "row N cell" line appears
    // exactly once, and y positions restart near the top on later pages.
    let lines = cell_lines(&layout);
    assert_eq!(lines.len(), 8);
    let first_on_page2 = &layout.pages[1].fragments[0];
    assert!(first_on_page2.rect.origin.y < 40.0 + 30.0 + 5.0);
}

#[test]
fn table_caption_and_text_order_preserved() {
    let html = r#"<html><body>
      <table>
        <caption>Table One</caption>
        <tr><td>alpha</td><td>beta</td></tr>
        <tr><td>gamma</td><td>delta</td></tr>
      </table></body></html>"#;
    let (layout, _) = layout_html(html, "", &page_for_lines(10));
    let texts: Vec<String> = cell_lines(&layout).into_iter().map(|(_, _, t)| t).collect();
    assert_eq!(texts[0], "Table One", "caption first");
    for word in ["alpha", "beta", "gamma", "delta"] {
        assert!(texts.iter().any(|t| t == word), "missing {word}");
    }
    // Locator monotonicity across the whole fragment stream still holds.
    let mut last = 0u32;
    for page in &layout.pages {
        for frag in &page.fragments {
            if let FragmentKind::Line(l) = &frag.kind {
                assert!(l.locator_start >= last);
                last = l.locator_start;
            }
        }
    }
}

// ---- Layout batch 2: keeps, @media, table polish, justified indents ----

#[test]
fn keep_with_next_migrates_heading_to_new_page() {
    // Filler leaves exactly one line of room; the heading lands there, and
    // its paragraph (2+ lines) can't follow — both must move to page 2.
    let html = format!(
        "<html><body>{}<h3>Kept Heading</h3>{}</body></html>",
        para_of_lines(3, "filler"),
        para_of_lines(3, "body")
    );
    let (layout, _) = layout_html(
        &html,
        "p { margin: 0; } h3 { margin: 0; font-size: 18px; break-after: avoid; }",
        &page_for_lines(4),
    );
    assert_eq!(layout.pages.len(), 2);
    let page2_texts: Vec<String> = layout.pages[1]
        .fragments
        .iter()
        .filter_map(|f| match &f.kind {
            FragmentKind::Line(l) => Some(l.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        page2_texts.first().map(String::as_str),
        Some("Kept Heading"),
        "heading must migrate with its paragraph: page2 = {page2_texts:?}"
    );
    // And nothing of the heading remains on page 1.
    assert!(layout.pages[0]
        .fragments
        .iter()
        .all(|f| !matches!(&f.kind, FragmentKind::Line(l) if l.text == "Kept Heading")));
}

#[test]
fn break_before_avoid_is_equivalent_keep() {
    let html = format!(
        "<html><body>{}<h3>Kept Too</h3>{}</body></html>",
        para_of_lines(3, "filler"),
        para_of_lines(3, "body")
    );
    let (layout, _) = layout_html(
        &html,
        "p { margin: 0; } h3 { margin: 0; font-size: 18px; } p + h3 + p { break-before: avoid; }",
        &page_for_lines(4),
    );
    // The paragraph after the heading declares break-before: avoid.
    let page2_first = layout.pages.get(1).and_then(|p| {
        p.fragments.iter().find_map(|f| match &f.kind {
            FragmentKind::Line(l) => Some(l.text.clone()),
            _ => None,
        })
    });
    assert_eq!(page2_first.as_deref(), Some("Kept Too"));
}

#[test]
fn media_screen_break_rules_apply_print_ones_do_not() {
    let html = format!(
        "<html><body>{}{}</body></html>",
        para_of_lines(1, "first"),
        r#"<p class="scr">screen-broken</p><p class="prn">print-broken</p>"#
    );
    let css = r#"
        p { margin: 0; }
        @media screen { .scr { page-break-before: always; } }
        @media print { .prn { page-break-before: always; } }
    "#;
    let (layout, _) = layout_html(&html, css, &page_for_lines(10));
    // screen rule honored → page break before .scr; print rule ignored →
    // .prn flows right after on the same page.
    assert_eq!(layout.pages.len(), 2);
    let page2: Vec<String> = layout.pages[1]
        .fragments
        .iter()
        .filter_map(|f| match &f.kind {
            FragmentKind::Line(l) => Some(l.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(page2, vec!["screen-broken", "print-broken"]);
}

#[test]
fn table_cell_vertical_align_middle() {
    let html = r#"<html><body><table><tr>
        <td class="mid">short</td>
        <td>a much longer cell<br/>with a second line<br/>and a third line</td>
    </tr></table></body></html>"#;
    let (layout, _) = layout_html(
        html,
        ".mid { vertical-align: middle; } td { padding: 0; }",
        &page_for_lines(10),
    );
    let lines = cell_lines(&layout);
    let short = lines.iter().find(|(_, _, t)| t == "short").unwrap();
    let first_long = lines
        .iter()
        .find(|(_, _, t)| t.starts_with("a much longer"))
        .unwrap();
    assert!(
        short.1 > first_long.1 + 10.0,
        "middle-aligned cell sits below the top: short_y={} long_y={}",
        short.1,
        first_long.1
    );
}

#[test]
fn table_header_repeats_on_continuation_pages() {
    let mut rows = String::from("<tr><th>Col A</th><th>Col B</th></tr>");
    for i in 0..8 {
        rows.push_str(&format!("<tr><td>row {i}</td><td>value {i}</td></tr>"));
    }
    let html = format!("<html><body><table>{rows}</table></body></html>");
    let (layout, _) = layout_html(&html, "td, th { padding: 0; }", &page_for_lines(5));
    assert!(layout.pages.len() >= 2);
    for (i, page) in layout.pages.iter().enumerate() {
        let first_text = page.fragments.iter().find_map(|f| match &f.kind {
            FragmentKind::Line(l) => Some(l.text.clone()),
            _ => None,
        });
        assert_eq!(
            first_text.as_deref(),
            Some("Col A"),
            "page {i} must open with the repeated header"
        );
    }
}

#[test]
fn indented_justified_first_line_fills_measure() {
    let html = "<html><body><p>words that repeat words that repeat words that \
                repeat words that repeat words that repeat words that repeat \
                words that repeat words that repeat</p></body></html>";
    let (layout, _) = layout_html(
        html,
        "p { margin: 0; text-indent: 40px; text-align: justify; }",
        &page_for_lines(10),
    );
    let lines: Vec<&chapbook_paint::Fragment> = layout.pages[0].fragments.iter().collect();
    assert!(lines.len() >= 3, "needs several lines");
    // Content width is 520; first line starts at 40 + 40 indent and must
    // reach the right edge like the justified middle lines do.
    let first = &lines[0];
    let middle = &lines[1];
    let first_right = first.rect.origin.x + first.rect.size.w;
    let middle_right = middle.rect.origin.x + middle.rect.size.w;
    assert!(
        (first_right - middle_right).abs() < 1.0,
        "justified first line must fill the measure: first_right={first_right} middle_right={middle_right}"
    );
}
