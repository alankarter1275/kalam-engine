//! Headless session behavior over the fixture books — the point of the
//! session extraction: reading logic testable without a window.

use std::path::PathBuf;

use chapbook_core::{BookKind, EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::Session;

fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// Each test gets its own library dir: tests run in parallel threads and
/// the env var is process-global, so serialize env mutation behind a lock
/// and only touch the var while holding it.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Open a session against a per-test library dir. The env var is
/// process-global and tests run in parallel, so the set-and-open pair
/// holds a lock.
fn open_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, true)
}

/// Reopen against the same per-test library (position-persistence tests).
fn reopen_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, false)
}

fn open_library(name: &str, source: &str, fresh: bool) -> Session {
    let guard = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "chapbook-session-test-{}-{name}",
        std::process::id()
    ));
    if fresh {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);
    let session = Session::open(source).unwrap();
    drop(guard);
    session
}

/// Drive the async load path to completion: render (queues the load),
/// then poll until the unit lands. Panics after ~5s.
fn render_loaded(s: &mut Session) -> chapbook_reader::tiny_skia::Pixmap {
    for _ in 0..200 {
        s.render().expect("render");
        if !s.has_pending_loads() {
            // One more poll+render in case the last result just arrived.
            s.poll_loaded();
            return s.render().expect("render");
        }
        s.poll_loaded();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("unit never finished loading");
}

/// Anchor a selection on the first hit-testable text on the current page.
/// The placed image rect depends on the page's natural size and the
/// margins, so sweep for a hit rather than hardcoding coordinates.
fn sweep_for_text(s: &mut Session) -> (f32, f32) {
    for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            if s.selection_begin(x as f32, y as f32) {
                return (x as f32, y as f32);
            }
        }
    }
    panic!("no hit-testable text on this page");
}

/// Find a point on the current page that sits inside a hyperlink. Where
/// the link lands depends on the fixture font and page size, so sweep.
fn sweep_for_link(s: &mut Session) -> Option<(f32, f32, String)> {
    for y in (40..760).step_by(4) {
        for x in (40..560).step_by(4) {
            if let Some(href) = s.link_at(x as f32, y as f32) {
                return Some((x as f32, y as f32, href));
            }
        }
    }
    None
}

fn metrics() -> PageMetrics {
    PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
        rotation: Rotation::None,
    }
}

#[test]
fn epub_session_renders_navigates_and_selects() {
    let mut s = open_isolated("epub-nav", &fixture("epub/illustrated.epub"));
    assert_eq!(s.kind(), BookKind::Epub);
    s.set_metrics(metrics());
    let pixmap = s.render().expect("page renders");
    assert_eq!((pixmap.width(), pixmap.height()), (600, 800));
    assert!(s.page_count() >= 2);

    // Selection: press near the top text line, drag right and down a line.
    assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
    s.selection_drag(400.0, 140.0);
    let (start, end) = s.selected_range().expect("non-empty selection");
    assert!(end > start);
    // The selected text comes back ready to paste: the locator space is
    // the raw source text, so its line breaks and indentation collapse.
    let text = s.selected_text().expect("selection carries text");
    assert!(
        text.starts_with("e Illustrated Chapter"),
        "selection starts mid-heading: {text:?}"
    );
    assert!(
        text.contains("Text before the picture"),
        "selection runs into the first paragraph: {text:?}"
    );
    assert!(!text.contains('\n'), "pasteable text has no line breaks");
    // The selected page renders with the highlight without panicking.
    s.render().unwrap();
    // Page navigation clears the selection.
    s.next_page();
    assert_eq!(s.selected_range(), None);
    assert_eq!(s.page(), 1);
    s.prev_page();
    assert_eq!(s.page(), 0);

    s.save_position();
}

#[test]
fn cbz_session_pages_through_images() {
    let mut s = open_isolated("cbz-pages", &fixture("cbz/minimal.cbz"));
    assert_eq!(s.kind(), BookKind::Comic);
    assert_eq!(s.spine_len(), 3);
    s.set_metrics(metrics());
    assert_eq!(s.page_count(), 1, "one page per comic unit");

    // Page 1 is solid red (196,64,48): sample the center.
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(
        px.red() > 150 && px.blue() < 90,
        "expected red page: {px:?}"
    );

    // Selection never engages on image pages, so there is nothing to copy.
    assert!(!s.selection_begin(300.0, 400.0));
    assert_eq!(s.selected_text(), None);

    s.next_page();
    assert_eq!(s.spine(), 1, "page turn advances the spine for comics");
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(
        px.green() > 90 && px.red() < 90,
        "expected green page: {px:?}"
    );
    s.next_page();
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(
        px.blue() > 120 && px.red() < 90,
        "expected blue page 10 last: {px:?}"
    );
    // End of book: stays put.
    s.next_page();
    assert_eq!(s.spine(), 2);

    s.save_position();
}

#[test]
fn cbz_position_persists_across_sessions() {
    let source = fixture("cbz/minimal.cbz");
    {
        let mut s = open_isolated("cbz-persist", &source);
        s.set_metrics(metrics());
        s.next_page();
        s.next_page();
        assert_eq!(s.spine(), 2);
        s.save_position();
    }
    let mut s = reopen_isolated("cbz-persist", &source);
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert_eq!(s.spine(), 2, "comic position restores by page progression");
}

#[test]
fn pdf_session_reads_like_an_image_book() {
    let mut s = open_isolated("pdf-read", &fixture("pdf/minimal.pdf"));
    assert_eq!(s.kind(), BookKind::Pdf);
    assert_eq!(s.spine_len(), 3);
    s.set_metrics(metrics());
    // Red first page, blue second; the rect pages carry no text.
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(px.red() > 150 && px.blue() < 100, "red PDF page: {px:?}");
    assert!(!s.selection_begin(300.0, 400.0));
    s.next_page();
    assert_eq!(s.spine(), 1);
    let px = render_loaded(&mut s).pixel(300, 400).unwrap();
    assert!(px.blue() > 120 && px.red() < 100, "blue PDF page: {px:?}");
    s.save_position();
}

#[test]
fn pdf_text_selection_highlights() {
    let mut s = open_isolated("pdf-select", &fixture("pdf/minimal.pdf"));
    s.set_metrics(metrics());
    // Page 3 carries the Helvetica text lines.
    s.next_page();
    s.next_page();
    assert_eq!(s.spine(), 2);
    let before = render_loaded(&mut s);

    let (ax, ay) = sweep_for_text(&mut s);
    s.selection_drag(ax + 150.0, ay);
    let (start, end) = s.selected_range().expect("selection over PDF text");
    assert!(end > start);
    let text = s.selected_text().expect("PDF selection carries text");
    assert!(
        "Hello selection".starts_with(&text),
        "expected a prefix of the first text line, got {text:?}"
    );

    // The highlight visibly changes the render.
    let after = render_loaded(&mut s);
    assert_ne!(before.data(), after.data(), "selection must paint");
    s.selection_clear();
}

#[test]
fn epub_highlight_persists_across_sessions() {
    let source = fixture("epub/illustrated.epub");
    let (start, end, text) = {
        let mut s = open_isolated("epub-highlight", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");
        assert!(s.selection_begin(100.0, 70.0));
        s.selection_drag(400.0, 140.0);
        let (start, end) = s.selected_range().expect("non-empty selection");
        let text = s.selected_text().expect("selection carries text");
        let id = s.add_highlight().expect("highlight is stored");
        assert!(id > 0);
        assert_eq!(s.highlights(0).len(), 1, "visible without a reload");
        (start, end, text)
    };

    let mut s = reopen_isolated("epub-highlight", &source);
    s.set_metrics(metrics());
    let stored = s.highlights(0).to_vec();
    assert_eq!(stored.len(), 1, "highlight survives the session");
    assert_eq!(
        (stored[0].start, stored[0].end),
        (start, end),
        "same edition resolves the exact offsets"
    );
    assert_eq!(stored[0].text.as_deref(), Some(text.as_str()));

    let painted = s.render().expect("page renders");
    // Locator space doesn't move under relayout, so the highlight is
    // unchanged at a different font size.
    s.adjust_font(4.0);
    assert_eq!(s.highlights(0), &stored[..]);
    s.adjust_font(-4.0);

    s.remove_annotation(stored[0].id);
    assert!(s.highlights(0).is_empty(), "delete clears the cache too");
    let plain = s.render().expect("page renders");
    assert_ne!(painted.data(), plain.data(), "the highlight was painted");

    let mut s = reopen_isolated("epub-highlight", &source);
    s.set_metrics(metrics());
    assert!(s.highlights(0).is_empty(), "delete survives the session");
}

#[test]
fn pdf_highlight_resolves_against_the_page_text_layer() {
    let source = fixture("pdf/minimal.pdf");
    let (start, end) = {
        let mut s = open_isolated("pdf-highlight", &source);
        s.set_metrics(metrics());
        // Page 3 carries the text lines.
        s.next_page();
        s.next_page();
        render_loaded(&mut s);
        let (ax, ay) = sweep_for_text(&mut s);
        s.selection_drag(ax + 150.0, ay);
        let range = s.selected_range().expect("selection over PDF text");
        assert!(s.add_highlight().expect("PDF highlight is stored") > 0);
        range
    };

    let mut s = reopen_isolated("pdf-highlight", &source);
    s.set_metrics(metrics());
    s.next_page();
    s.next_page();
    assert_eq!(s.spine(), 2);
    // The page's text layer is what the endpoints resolve against, so
    // nothing resolves until the page has loaded.
    assert!(
        s.highlights(2).is_empty(),
        "unresolved before the page loads"
    );
    render_loaded(&mut s);
    let stored = s.highlights(2).to_vec();
    assert_eq!(stored.len(), 1);
    assert_eq!((stored[0].start, stored[0].end), (start, end));
}

#[test]
fn comic_pages_take_no_highlights() {
    let mut s = open_isolated("cbz-highlight", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert_eq!(s.add_highlight(), None, "no text layer to anchor to");
    assert!(s.highlights(0).is_empty());
}

#[test]
fn display_list_is_the_backend_contract() {
    use chapbook_reader::chapbook_paint::DisplayOp;

    let mut s = open_isolated("epub-displaylist", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    let dl = s.frame().expect("page has a frame").list;

    assert_eq!((dl.size.w, dl.size.h), (600.0, 800.0), "page-space size");
    // The page ground is op 0 so a theme is a color choice, not a
    // renderer concern.
    match dl.ops.first().expect("at least the background") {
        DisplayOp::FillRect { rect, color } => {
            assert_eq!((rect.size.w, rect.size.h), (600.0, 800.0));
            assert_eq!(*color, chapbook_core::Theme::Light.background());
        }
        op => panic!("expected the page ground first, got {op:?}"),
    }
    assert!(
        dl.ops
            .iter()
            .any(|op| matches!(op, DisplayOp::GlyphRun { .. })),
        "a text page paints glyphs"
    );

    // The seam is complete: a shell can reproduce render() from the
    // public API alone — ops, fonts, images.
    let mut pixmap = chapbook_reader::tiny_skia::Pixmap::new(600, 800).expect("pixmap");
    let mut renderer = chapbook_reader::chapbook_render_tinyskia::Renderer::new();
    let (fonts, images) = s.paint_resources();
    renderer.render(&dl, fonts, images, 1.0, &mut pixmap);
    let theirs = s.render().expect("render");
    assert_eq!(pixmap.data(), theirs.data(), "same ops, same pixels");
}

#[test]
fn image_units_key_their_ops_into_the_store() {
    use chapbook_reader::chapbook_paint::DisplayOp;

    let mut s = open_isolated("cbz-displaylist", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    let dl = s.frame().expect("comic page has a frame").list;
    let resource = dl
        .ops
        .iter()
        .find_map(|op| match op {
            DisplayOp::Image { resource, .. } => Some(*resource),
            _ => None,
        })
        .expect("a comic page is an image op");
    assert!(
        s.image_store().get(resource).is_some(),
        "the op's key resolves in the store the session hands out"
    );
}

#[test]
fn frames_report_what_changed() {
    use chapbook_reader::chapbook_paint::FrameIntent;

    let mut s = open_isolated("epub-intent", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());

    // Setting metrics is a reflow, and taking the frame consumes it.
    let frame = s.frame().expect("frame");
    assert_eq!(frame.intent, FrameIntent::Relayout);
    assert_eq!(frame.damage, None, "a reflow disturbs the whole page");
    assert_eq!(s.frame().unwrap().intent, FrameIntent::Repaint);

    s.next_page();
    assert_eq!(s.frame().unwrap().intent, FrameIntent::PageTurn);
    // illustrated.epub is one spine item, so this is a no-op and nothing
    // is reported — the mark follows the move, not the call.
    s.next_unit();
    assert_eq!(s.frame().unwrap().intent, FrameIntent::Repaint);
    s.cycle_theme();
    assert_eq!(s.frame().unwrap().intent, FrameIntent::Relayout);

    // A book with somewhere to go reports the unit change.
    let mut m = open_isolated("epub-intent-units", &fixture("epub/minimal.epub"));
    m.set_metrics(metrics());
    m.frame().expect("frame");
    m.next_unit();
    assert_eq!(m.frame().unwrap().intent, FrameIntent::UnitChange);

    // The strongest change since the last frame is the one reported.
    s.selection_begin(100.0, 70.0);
    s.next_page();
    assert_eq!(
        s.frame().unwrap().intent,
        FrameIntent::PageTurn,
        "a page turn outranks the selection it cleared"
    );
}

#[test]
fn a_selection_change_damages_only_the_lines_it_touches() {
    use chapbook_reader::chapbook_paint::FrameIntent;

    let mut s = open_isolated("epub-damage", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.frame().expect("frame");

    assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
    s.selection_drag(400.0, 70.0);
    let frame = s.frame().expect("frame");
    assert_eq!(frame.intent, FrameIntent::Selection);
    let damage = frame.damage.expect("a selection states its damage");
    assert!(
        damage.size.h < 600.0 && damage.size.w <= 600.0,
        "a one-line selection is not the whole page: {damage:?}"
    );

    // Clearing damages what the selection used to cover, so the backend
    // knows to repaint those lines back to plain text.
    s.selection_clear();
    let cleared = s.frame().expect("frame");
    assert_eq!(cleared.intent, FrameIntent::Selection);
    let cleared = cleared.damage.expect("clearing states its damage too");
    assert!(
        (cleared.size.w - damage.size.w).abs() < 1.0
            && (cleared.size.h - damage.size.h).abs() < 1.0,
        "the same lines: {cleared:?} vs {damage:?}"
    );
}

#[test]
fn a_highlight_damages_only_its_own_lines() {
    use chapbook_reader::chapbook_paint::FrameIntent;

    let mut s = open_isolated("epub-damage-highlight", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.frame().expect("frame");

    assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
    s.selection_drag(400.0, 70.0);
    let selection = s
        .frame()
        .expect("frame")
        .damage
        .expect("a selection states its damage");

    // Adding a highlight outranks the selection on the intent ordering.
    // Damage must not be lost to that: the marked lines are the only ones
    // that changed, and on a panel the difference is a partial refresh
    // versus a full-page flash.
    s.add_highlight().expect("highlight the selection");
    let frame = s.frame().expect("frame");
    assert_eq!(frame.intent, FrameIntent::Annotation);
    let damage = frame
        .damage
        .expect("an annotation states its damage, even though it outranks Selection");
    assert!(
        damage.size.h < 600.0,
        "a one-line highlight is not the whole page: {damage:?}"
    );
    assert!(
        damage.size.h >= selection.size.h - 1.0,
        "the highlight covers at least the lines the selection did: \
         {damage:?} vs {selection:?}"
    );
}

#[test]
fn a_change_that_cannot_name_its_region_repaints_everything() {
    use chapbook_reader::chapbook_paint::FrameIntent;

    let mut s = open_isolated("epub-damage-unstated", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.frame().expect("frame");

    // A live selection knows its lines; a reflow does not, and the
    // pessimistic answer has to win rather than the stated one.
    assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
    s.selection_drag(400.0, 70.0);
    s.set_metrics(metrics().with_rotation(Rotation::Half));
    let frame = s.frame().expect("frame");
    assert_eq!(frame.intent, FrameIntent::Relayout);
    assert_eq!(
        frame.damage, None,
        "an unstated change must not inherit the selection's region"
    );
}

#[test]
fn a_landed_page_load_is_its_own_intent() {
    use chapbook_reader::chapbook_paint::FrameIntent;

    let mut s = open_isolated("cbz-intent", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    // Drive one more unit's load and catch the frame it produces.
    s.next_unit();
    s.frame();
    for _ in 0..200 {
        if s.poll_loaded() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(s.frame().unwrap().intent, FrameIntent::ContentArrived);
}

#[test]
fn a_grey_panel_gets_grey_pages() {
    use chapbook_core::PixelFormat;

    let mut s = open_isolated("epub-grey", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    let color = s.render().expect("page renders");

    s.set_pixel_format(PixelFormat::Grey {
        levels: 2,
        dither: true,
    });
    let grey = s.render().expect("page renders");
    assert_ne!(color.data(), grey.data(), "the panel format reached render");
    for px in grey.data().as_chunks::<4>().0 {
        assert!(px[0] == 0 || px[0] == 255, "not 1-bit: {}", px[0]);
        assert_eq!((px[1], px[2]), (px[0], px[0]), "grey");
    }

    // Ops are unchanged by the conversion, so the frame record isn't
    // disturbed and a partial refresh stays valid.
    assert_eq!(
        s.frame().unwrap().intent,
        chapbook_reader::chapbook_paint::FrameIntent::Repaint
    );
}

#[test]
fn a_turned_panel_gets_turned_pixels_and_untwisted_input() {
    let mut s = open_isolated("epub-rotation", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    let upright = s.render().expect("page renders");
    assert_eq!((upright.width(), upright.height()), (600, 800));

    // A selection over the heading, in page space.
    assert!(s.selection_begin(100.0, 70.0));
    s.selection_drag(400.0, 70.0);
    let expected = s.selected_range().expect("selection over the heading");
    s.selection_clear();

    // Same page, quarter-turned onto the panel. Layout is untouched, so
    // only the buffer changes shape.
    s.set_metrics(metrics().with_rotation(Rotation::Quarter));
    let turned = s.render().expect("page renders");
    assert_eq!((turned.width(), turned.height()), (800, 600));
    assert_eq!(
        s.page_count(),
        {
            let mut plain = open_isolated("epub-rotation-plain", &fixture("epub/illustrated.epub"));
            plain.set_metrics(metrics());
            plain.page_count()
        },
        "a turn does not reflow"
    );

    // Input arrives in panel coordinates: the page point (x, y) sits at
    // (h - y, x) after a clockwise quarter turn.
    assert!(s.selection_begin(800.0 - 70.0, 100.0));
    s.selection_drag(800.0 - 70.0, 400.0);
    assert_eq!(
        s.selected_range(),
        Some(expected),
        "the same words, reached through the turned panel"
    );
}

#[test]
fn links_and_the_toc_both_navigate_and_the_trail_comes_back() {
    let mut s = open_isolated("epub-nav", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);

    let (lx, ly, href) = sweep_for_link(&mut s).expect("chapter one links to chapter two");
    assert_eq!(href, "chapter2.xhtml");
    // A tap in the margin beside the link is not a tap on the link.
    assert_eq!(s.link_at(2.0, ly), None);
    assert!(s.link_at(lx, ly).is_some());

    assert!(s.follow_link(&href));
    assert_eq!(s.spine(), 1, "the link crossed to chapter two");
    assert!(s.can_go_back());
    assert!(s.back());
    assert_eq!(s.spine(), 0, "and back again");
    assert!(!s.can_go_back(), "the trail is spent");

    // The TOC's nested entry carries a fragment.
    let entry = s.toc()[1].children[0].clone();
    assert_eq!(entry.fragment.as_deref(), Some("part2"));
    assert!(s.goto_toc(&entry));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 1);
    let anchor_page = s.page();

    // A fragment the unit doesn't have lands at its start rather than
    // failing outright.
    assert!(s.goto_anchor(1, "not-in-this-chapter"));
    s.render().expect("page renders");
    assert_eq!(s.page(), 0);
    assert!(anchor_page >= s.page());
}

#[test]
fn external_links_are_not_a_reading_position() {
    let mut s = open_isolated("epub-nav-external", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert!(!s.follow_link("https://example.com/"));
    assert!(!s.follow_link("mailto:nobody@example.com"));
    assert!(!s.follow_link("chapter9.xhtml"), "no such unit");
    assert_eq!(s.spine(), 0);
    assert!(!s.can_go_back(), "a refused link leaves no trail");
}

#[test]
fn a_jump_remembers_the_offset_it_left() {
    let mut s = open_isolated("epub-nav-offset", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    // Read into chapter two, past its forced page break, so the position
    // being remembered is an offset and not just a unit.
    assert!(s.goto(chapbook_core::Locator::chapter_start(1)));
    s.render().expect("page renders");
    s.next_page();
    s.render().expect("page renders");
    let before = s.locator();
    assert_eq!(before.spine_index, 1);
    assert!(before.char_offset > 0, "reading past the first page");

    assert!(s.goto(chapbook_core::Locator::chapter_start(0)));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);

    assert!(s.back());
    s.render().expect("page renders");
    assert_eq!(s.locator().spine_index, before.spine_index);
    assert_eq!(
        s.locator().char_offset,
        before.char_offset,
        "back lands on the offset it left, not the chapter start"
    );
}

#[test]
fn search_finds_hits_that_navigate_and_select() {
    let mut s = open_isolated("epub-search", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let hits = s.search("universally acknowledged", 10);
    assert_eq!(hits.len(), 1, "one hit in chapter one: {hits:?}");
    let hit = hits[0].clone();
    assert_eq!(hit.locator.spine_index, 0);
    assert_eq!(hit.end - hit.locator.char_offset, 24);

    // The context is list-ready: the raw locator text\'s newlines and
    // XHTML indentation are collapsed out of it.
    assert!(!hit.context.contains('\n'));
    assert!(!hit.context.contains("  "));
    let matched: String = hit
        .context
        .chars()
        .skip(hit.match_range.0 as usize)
        .take((hit.match_range.1 - hit.match_range.0) as usize)
        .collect();
    assert_eq!(matched, "universally acknowledged", "{:?}", hit.context);

    // A hit is a locator: it navigates, and selecting its range on the
    // page it lands on reads back the words that were searched for.
    assert!(s.goto(hit.locator));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);
    s.select_range(hit.locator.char_offset, hit.end);
    assert_eq!(
        s.selected_text().as_deref(),
        Some("universally acknowledged")
    );
}

#[test]
fn search_is_case_insensitive_and_spans_the_spine() {
    let mut s = open_isolated("epub-search-case", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());

    let hits = s.search("CHAPTER", 50);
    assert!(hits.len() >= 2, "chapters one and two both match: {hits:?}");
    assert!(
        hits.iter().any(|h| h.locator.spine_index == 0)
            && hits.iter().any(|h| h.locator.spine_index == 1),
        "hits come from both units"
    );
    // Hits arrive in reading order.
    let mut ordered = hits.clone();
    ordered.sort_by_key(|h| (h.locator.spine_index, h.locator.char_offset));
    assert_eq!(hits, ordered);

    assert_eq!(s.search("chapter", 2).len(), 2, "the limit is honored");
    assert!(
        s.search("", 10).is_empty(),
        "an empty query matches nothing"
    );
    assert!(s.search("no such phrase anywhere", 10).is_empty());
}

#[test]
fn comics_have_nothing_to_search() {
    let mut s = open_isolated("cbz-search", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert!(s.search("anything", 10).is_empty());
}

#[test]
fn a_pdf_page_becomes_searchable_once_it_loads() {
    let mut s = open_isolated("pdf-search", &fixture("pdf/minimal.pdf"));
    s.set_metrics(metrics());
    // Page three carries the text; nothing is searchable before it loads.
    assert!(s.search_unit(2, "Hello").is_empty());

    s.next_unit();
    s.next_unit();
    render_loaded(&mut s);
    let hits = s.search_unit(2, "hello");
    assert_eq!(hits.len(), 1, "the text layer is searchable: {hits:?}");
    assert_eq!(hits[0].locator.spine_index, 2);
    assert!(hits[0].context.starts_with("Hello"));
}

#[test]
fn settings_survive_a_restart_and_can_be_overridden_per_book() {
    use chapbook_reader::SettingsScope;

    let source = fixture("epub/minimal.epub");
    {
        let mut s = open_isolated("epub-settings", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");
        assert_eq!(s.settings().base_font_px, 18.0, "the built-in default");

        s.adjust_font(4.0);
        s.cycle_theme();
        assert_eq!(s.settings().base_font_px, 22.0);
        assert_eq!(s.settings().theme, chapbook_core::Theme::Sepia);
    }

    // Font size used not to survive a restart. It does now.
    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert_eq!(s.settings().base_font_px, 22.0);
    assert_eq!(s.settings().theme, chapbook_core::Theme::Sepia);

    // The fields no shell could reach before.
    let mut wide = s.settings().clone();
    wide.justify = true;
    wide.line_height = 1.9;
    wide.publisher_styles = false;
    s.set_settings(wide, SettingsScope::ThisBook);
    assert!(s.settings().justify);
    drop(s);

    // A per-book override outlives a later change to the default.
    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert!(s.settings().justify, "the override loaded");
    assert_eq!(s.settings().line_height, 1.9);
    assert!(!s.settings().publisher_styles);

    let mut plain = s.settings().clone();
    plain.base_font_px = 12.0;
    plain.justify = false;
    s.set_settings(plain, SettingsScope::Global);
    drop(s);

    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert!(
        s.settings().justify,
        "the book keeps its override when the default moves"
    );
    assert_eq!(s.settings().base_font_px, 22.0);

    // Clearing the override hands the book back to the default.
    s.clear_book_settings();
    assert_eq!(s.settings().base_font_px, 12.0);
    assert!(!s.settings().justify);
    drop(s);

    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert_eq!(s.settings().base_font_px, 12.0, "and it stays cleared");
}

#[test]
fn a_settings_change_relayouts_and_keeps_the_place() {
    use chapbook_reader::SettingsScope;

    let mut s = open_isolated("epub-settings-relayout", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");
    let before = s.render().expect("page renders").data().to_vec();

    let mut bigger = s.settings().clone();
    bigger.base_font_px += 6.0;
    s.set_settings(bigger, SettingsScope::Global);
    assert_eq!(
        s.frame().unwrap().intent,
        chapbook_reader::chapbook_paint::FrameIntent::Relayout
    );
    assert_ne!(
        s.render().expect("page renders").data(),
        &before[..],
        "the page reflowed"
    );
    assert_eq!(s.spine(), 0, "and the reader stayed put");
}

#[test]
fn taps_find_highlights_recolor_them_and_list_every_mark() {
    use chapbook_library::AnnotationKind;

    let source = fixture("epub/illustrated.epub");
    let (id, point) = {
        let mut s = open_isolated("epub-annotations", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");

        assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
        s.selection_drag(400.0, 70.0);
        let id = s.add_highlight().expect("highlight is stored");
        s.selection_clear();

        // A tap inside the marked words finds it; the margin beside them
        // does not.
        assert_eq!(s.highlight_at(110.0, 70.0), Some(id));
        assert_eq!(s.highlight_at(2.0, 70.0), None);
        (id, (110.0, 70.0))
    };

    let mut s = reopen_isolated("epub-annotations", &source);
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert_eq!(
        s.highlight_at(point.0, point.1),
        Some(id),
        "found after a reload"
    );

    // Recoloring paints differently and survives the session.
    let themed = s.render().expect("page renders").data().to_vec();
    s.set_highlight_color(id, Some("#ff0000"));
    assert_ne!(
        s.render().expect("page renders").data(),
        &themed[..],
        "the stored color reached the page"
    );
    drop(s);

    let mut s = reopen_isolated("epub-annotations", &source);
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert_eq!(
        s.highlights(0)[0].color.as_deref(),
        Some("#ff0000"),
        "the color persisted"
    );

    // Notes and bookmarks are marks too, and all three list together.
    assert!(s.selection_begin(100.0, 70.0));
    s.selection_drag(300.0, 70.0);
    let note = s.add_note("worth revisiting").expect("note is stored");
    s.selection_clear();
    let bookmark = s.add_bookmark().expect("bookmark is stored");

    let all = s.annotations();
    assert_eq!(all.len(), 3, "{all:?}");
    assert_eq!(
        all.iter()
            .filter(|a| a.kind == AnnotationKind::Bookmark)
            .count(),
        1
    );
    let note_summary = all.iter().find(|a| a.id == note).expect("note listed");
    assert_eq!(note_summary.kind, AnnotationKind::Note);
    assert_eq!(note_summary.text.as_deref(), Some("worth revisiting"));
    assert!(all.windows(2).all(|w| w[0].progression <= w[1].progression));

    // A bookmark is a point: it paints nothing, while the ranged marks do.
    assert_eq!(s.highlights(0).len(), 2, "the bookmark isn't painted");

    // Every mark can be jumped to and taken away again.
    assert!(s.goto_annotation(bookmark));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);
    s.remove_annotation(note);
    assert_eq!(s.annotations().len(), 2);
    assert_eq!(s.highlights(0).len(), 1);
}
