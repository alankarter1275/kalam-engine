//! Headless session behavior over the fixture books — the point of the
//! session extraction: reading logic testable without a window.

use std::path::PathBuf;

use chapbook_core::{BookKind, EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::{Session, SessionConfig};

fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// Open a session against a per-test library dir.
///
/// This used to set `CHAPBOOK_LIBRARY_DIR` behind a mutex, because the only
/// way to place a library was a process-global variable and these tests run
/// in parallel. `SessionConfig::with_library_dir` is an argument, so the
/// lock is gone and so is the serialization.
fn open_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, true)
}

/// Reopen against the same per-test library (position-persistence tests).
/// The vendored fixture faces, all three axes pinned, so this suite means
/// the same thing on Linux, on a Mac and on a device. Taking the host's
/// fonts is what pinned two of these assertions to one machine's
/// collection.
fn fixture_fonts() -> chapbook_core::FontSource {
    chapbook_core::FontSource::embedded(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts"),
        "Crimson Text",
    )
}

fn reopen_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, false)
}

fn open_library(name: &str, source: &str, fresh: bool) -> Session {
    let dir = library_dir(name);
    if fresh {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Session::open_with(
        source,
        SessionConfig::new(fixture_fonts()).with_library_dir(dir),
    )
    .unwrap()
}

/// A library dir of this test's own, stable across a reopen.
fn library_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "chapbook-session-test-{}-{name}",
        std::process::id()
    ))
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

/// The one locator range where `needle` occurs in the open unit.
///
/// Offsets come from search rather than being written down. `search_unit`
/// and `select_range` share the unit's locator space, and neither has any
/// idea which fonts laid the page out — so a range obtained this way means
/// the same thing on every host, which a hand-written offset does not.
fn only_hit(s: &mut Session, needle: &str) -> (u32, u32) {
    let spine = s.spine();
    let hits = s.search_unit(spine, needle);
    assert_eq!(hits.len(), 1, "{needle:?} should occur exactly once");
    (hits[0].locator.char_offset, hits[0].end)
}

#[test]
fn epub_session_renders_navigates_and_selects() {
    let mut s = open_isolated("epub-nav", &fixture("epub/illustrated.epub"));
    assert_eq!(s.kind(), BookKind::Epub);
    s.set_metrics(metrics());
    let pixmap = s.render().expect("page renders");
    assert_eq!((pixmap.width(), pixmap.height()), (600, 800));
    assert!(s.page_count() >= 2);

    // Selection by hit test: press near the top text line, drag right and
    // down a line. Which character that lands on is a function of the
    // host's fonts, so this half asserts the shape of the result — it runs
    // from the heading into the paragraph below it — and the deterministic
    // half below asserts the exact text.
    assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
    s.selection_drag(400.0, 140.0);
    let (start, end) = s.selected_range().expect("non-empty selection");
    assert!(end > start);
    let text = s.selected_text().expect("selection carries text");
    assert!(
        text.contains("Illustrated Chapter"),
        "selection starts in the heading: {text:?}"
    );
    assert!(
        text.contains("Text before the picture"),
        "selection runs into the first paragraph: {text:?}"
    );
    assert!(!text.contains('\n'), "pasteable text has no line breaks");

    // The selected text comes back ready to paste: the locator space is
    // the raw source text, so its line breaks and indentation collapse.
    // chapter1.xhtml ends a source line after the link and indents the
    // next, so a range spanning the two proves the collapse exactly.
    let (link_start, _) = only_hit(&mut s, "underlined link");
    let (_, after_end) = only_hit(&mut s, "and some");
    s.select_range(link_start, after_end);
    assert_eq!(
        s.selected_text().as_deref(),
        Some("underlined link and some"),
        "the source breaks the line and indents between these two"
    );
    s.selection_clear();
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
    renderer.render(&dl, fonts, images, 1.0, &mut pixmap.as_mut());
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

    // The strongest change since the last frame is the one reported. Walk
    // back to the first page before asking for a turn: how many pages this
    // book has depends on the host's fonts, and under some of them page 1
    // is the last, which would make the turn below a no-op and leave the
    // selection as the strongest change.
    assert!(s.page_count() >= 2);
    while s.prev_page() {}
    s.frame().expect("consume the walk back");
    let (heading_start, heading_end) = only_hit(&mut s, "Illustrated Chapter");
    s.select_range(heading_start, heading_end);
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
fn a_landed_page_load_is_its_own_intent_and_states_its_region() {
    use chapbook_reader::chapbook_paint::FrameIntent;

    let mut s = open_isolated("cbz-intent", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    // The first frame is the placeholder, and asking for it is what
    // queues the unit.
    s.frame().expect("placeholder frame");
    for _ in 0..200 {
        if s.poll_loaded() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let frame = s.frame().expect("frame once the page landed");
    assert_eq!(frame.intent, FrameIntent::ContentArrived);

    // A page image is letterboxed into the content box, so the pixels the
    // placeholder painted around it did not change — and the frame says
    // so instead of claiming the whole page.
    let damage = frame
        .damage
        .expect("a landed image knows exactly where it went");
    let m = metrics();
    let page = chapbook_core::Rect::new(0.0, 0.0, m.size.w, m.size.h);
    assert!(page.contains_rect(&damage), "{damage:?} escaped {page:?}");
    assert!(
        damage.size.w < m.size.w || damage.size.h < m.size.h,
        "{damage:?} is the whole page, which is what stating a region was \
         supposed to avoid"
    );
}

/// The other half: a unit landing that the reader cannot see must not ask
/// for anything at all. Prefetches land constantly, and treating each one
/// as a change cost a full-page panel update — on e-ink, a visible flash
/// of a page that had not moved.
#[test]
fn a_prefetch_that_lands_off_screen_asks_for_nothing() {
    use chapbook_reader::chapbook_paint::FrameIntent;

    let mut s = open_isolated("cbz-prefetch", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    // Move to a unit that is already decoded. Laying it out prefetches the
    // one after it, which is the load this test watches.
    s.next_unit();
    let _ = s.page_count();
    s.frame().expect("frame for the unit change");
    assert!(
        s.has_pending_loads(),
        "expected the next unit to be prefetching"
    );

    let mut reported = false;
    for _ in 0..400 {
        reported |= s.poll_loaded();
        if !s.has_pending_loads() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!s.has_pending_loads(), "the prefetch never landed");
    assert!(
        !reported,
        "poll_loaded asked for a redraw for a unit that is not on screen"
    );
    assert_eq!(
        s.frame().expect("frame").intent,
        FrameIntent::Repaint,
        "an off-screen prefetch left a change record behind"
    );
}

/// Dithering follows the images, not the page. The display list is the
/// only thing that still knows which pixels came from a photograph by the
/// time there are pixels, so it is where the answer has to come from.
#[test]
fn dither_regions_name_the_images_and_nothing_else() {
    let mut s = open_isolated("epub-dither", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    // Find the page that actually carries the image.
    let m = metrics();
    for _ in 0..40 {
        let list = s.frame().expect("frame").list;
        let images: Vec<_> = list
            .ops
            .iter()
            .filter_map(|op| match op {
                chapbook_reader::chapbook_paint::DisplayOp::Image { dest, .. } => Some(*dest),
                _ => None,
            })
            .collect();
        let regions = list.dither_regions(1.0);
        assert_eq!(
            regions.len(),
            images.len(),
            "one region per image op, and no region for anything else"
        );
        if let Some(dest) = images.first() {
            let region = regions[0];
            // Rounded outward, so the region covers the op rather than
            // trimming it — a trimmed edge leaves an undithered sliver.
            assert!(f32::from(region.x as u16) <= dest.min_x() + 1.0);
            assert!(f32::from(region.max_x() as u16) >= dest.max_x() - 1.0);
            // And it is a region, not the page.
            assert!(
                region.w < m.size.w as u32 || region.h < m.size.h as u32,
                "{region:?} is the whole page"
            );
            return;
        }
        if !s.next_page() {
            break;
        }
        render_loaded(&mut s);
    }
    panic!("no image op anywhere in the fixture, so this asserts nothing");
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

/// A session may cross threads but may not be shared across them, which is
/// the shape every binding in `docs/FFI.md` is built on: a host holds one
/// opaque handle, moves it freely, and needs no lock of its own. `Sync`
/// fails today on the loader's receiver and on rusqlite's connection, so
/// only the half we actually rely on is asserted here — if `Send` ever
/// goes, the FFI's threading contract goes with it.
#[test]
fn a_session_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Session>();
}

/// The failure `FontSource` exists to convert into an error.
///
/// A session that finds no faces is not an ereader that looks wrong; it is
/// an ereader that cannot move. Every book paginates to one blank page, so
/// there is nowhere to navigate to, nothing for search to find, and no page
/// for a TOC entry to land on — and the session lays out, renders, paints
/// and *conforms* the whole time. fontdb has no Android, iOS or wasm
/// branch, which makes that the ordinary case on three platforms rather
/// than a corner.
#[test]
fn a_session_with_no_faces_is_refused_at_construction() {
    let empty = chapbook_core::FontSource::embedded("/nonexistent/fonts", "Nothing");
    let result = Session::open_with(
        fixture("epub/minimal.epub"),
        SessionConfig::new(empty).with_library_dir(library_dir("fontless")),
    );

    let err = result.err().expect("a fontless session must not open");
    let message = err.to_string();
    assert!(message.contains("no faces"), "{message}");
}

/// The read-back half: a shell cannot offer a font-family picker over a
/// list it cannot obtain.
#[test]
fn the_session_can_enumerate_the_families_it_was_given() {
    let session = open_isolated("families", &fixture("epub/minimal.epub"));

    let families = session.font_families();
    assert_eq!(families, vec!["Crimson Text".to_string()]);
    // And the source resolved cleanly, which is what a shell would print.
    assert!(
        session.font_report().is_clean(),
        "{}",
        session.font_report()
    );
    assert_eq!(session.font_report().faces, 4);
}

/// The host capabilities a session used to reach for on its own now arrive
/// through `SessionConfig`. This covers the plumbing; the credential retry
/// flow itself lives in `transport.rs`, which can drive it now that the
/// transport is injectable too.
#[test]
fn a_session_takes_its_host_capabilities_explicitly() {
    use chapbook_core::{Credential, CredentialKey, CredentialStore, Freshness, MemoryCredentials};

    let dir = library_dir("config");
    let store = std::sync::Arc::new(MemoryCredentials::new());
    let key = CredentialKey::http_origin("https://cat.example.com/opds/abc123secret/").unwrap();
    store
        .store(&key, &Credential::basic("reader", "pw"))
        .unwrap();

    let config = SessionConfig::new(fixture_fonts())
        .with_credentials(store.clone())
        .with_library_dir(&dir);
    // A store in the config is not a store the local path consults.
    let session = Session::open_with(fixture("epub/minimal.epub"), config).unwrap();
    assert!(session.spine_len() > 0);

    // And the config's Debug is safe to log: no secret in it.
    let shown = format!("{:?}", SessionConfig::new(fixture_fonts()));
    assert!(!shown.contains("pw"), "{shown}");

    assert!(matches!(
        store.get(&key, Freshness::Cached),
        chapbook_core::CredentialLookup::Found(_)
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A host's own buffer is a legitimate render target.
///
/// The point of `render_into` is that every platform already owns the
/// memory it wants the page in, and `render()` allocating a fresh `Pixmap`
/// per page turn meant a copy into that memory on every one. These check
/// the promise rather than the plumbing: same pixels, and the size a caller
/// is told to allocate is the size it gets.
mod render_into {
    use super::*;

    fn metrics() -> chapbook_core::PageMetrics {
        chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(20.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        }
    }

    fn ready(name: &str) -> Session {
        let mut session = open_isolated(name, &fixture("epub/minimal.epub"));
        session.set_metrics(metrics());
        session
    }

    #[test]
    fn render_size_matches_what_render_produces() {
        // `render_size` derives from the metrics; `render` sizes its pixmap
        // from the display list. A caller allocates from the first and the
        // engine rasterizes against the second, so if these ever diverge
        // `render_into` refuses rather than misdrawing — and this is what
        // notices the divergence.
        let mut session = ready("render-size");
        let (w, h) = session.render_size().expect("metrics are set");
        let pixmap = session.render().expect("something to draw");
        assert_eq!((pixmap.width(), pixmap.height()), (w, h));
    }

    #[test]
    fn a_borrowed_buffer_gets_the_same_pixels_as_an_allocated_one() {
        let mut owned = ready("into-owned");
        let expected = owned.render().expect("something to draw");
        let (w, h) = (expected.width(), expected.height());

        let mut borrowed = ready("into-borrowed");
        let mut dst = vec![0u8; (w * h * 4) as usize];
        assert!(borrowed.render_into(&mut dst, w, h, (w * 4) as usize));

        assert_eq!(
            dst,
            expected.data(),
            "byte-exact, or it is not the same page"
        );
    }

    #[test]
    fn a_padded_stride_lands_row_by_row_and_leaves_the_padding_alone() {
        // Android exposes a stride and does not promise it equals the row.
        // The slow path has to be correct even though it is not free.
        let mut owned = ready("stride-owned");
        let expected = owned.render().expect("something to draw");
        let (w, h) = (expected.width(), expected.height());
        let row = (w * 4) as usize;
        let stride = row + 64;

        let mut session = ready("stride-into");
        let mut dst = vec![0xABu8; stride * h as usize];
        assert!(session.render_into(&mut dst, w, h, stride));

        for y in 0..h as usize {
            let at = y * stride;
            assert_eq!(&dst[at..at + row], &expected.data()[y * row..(y + 1) * row]);
            assert!(
                dst[at + row..at + stride].iter().all(|b| *b == 0xAB),
                "row {y}: padding is the host's, not ours"
            );
        }
    }

    #[test]
    fn a_rotated_page_still_arrives_the_right_way_up() {
        let mut session = ready("rotated");
        session.set_metrics(chapbook_core::PageMetrics {
            rotation: chapbook_core::Rotation::Quarter,
            ..metrics()
        });
        let (w, h) = session.render_size().expect("metrics are set");
        // The quarter turn swaps the axes, and `render_size` says so.
        assert_eq!((w, h), (600, 400));

        let expected = session.render().expect("something to draw");
        assert_eq!((expected.width(), expected.height()), (w, h));

        let mut into = ready("rotated-into");
        into.set_metrics(chapbook_core::PageMetrics {
            rotation: chapbook_core::Rotation::Quarter,
            ..metrics()
        });
        let mut dst = vec![0u8; (w * h * 4) as usize];
        assert!(into.render_into(&mut dst, w, h, (w * 4) as usize));
        assert_eq!(dst, expected.data());
    }

    #[test]
    fn a_buffer_that_does_not_fit_is_refused_rather_than_overrun() {
        let mut session = ready("refused");
        let (w, h) = session.render_size().expect("metrics are set");
        let row = (w * 4) as usize;

        // Wrong dimensions.
        let mut dst = vec![0u8; row * h as usize];
        assert!(!session.render_into(&mut dst, w + 1, h, row));
        assert!(!session.render_into(&mut dst, w, h + 1, row));
        // A stride narrower than a row.
        assert!(!session.render_into(&mut dst, w, h, row - 1));
        // Right shape, too few bytes.
        let mut short = vec![0u8; row * h as usize - 1];
        assert!(!session.render_into(&mut short, w, h, row));
    }

    #[test]
    fn without_metrics_there_is_no_size_and_nothing_to_render_into() {
        let mut session = open_isolated("no-metrics", &fixture("epub/minimal.epub"));
        assert!(session.render_size().is_none());
        let mut dst = vec![0u8; 16];
        assert!(!session.render_into(&mut dst, 2, 2, 8));
    }
}

/// The caches had no ceiling: `layouts` and `images` were cleared only
/// wholesale, and for an image book not even then. Measured, reading forty
/// 1600x2400 comic pages forward took a process to 676 MB and no API could
/// give any of it back. These pin the ceiling that fixes it.
///
/// The CBZ fixture decodes to 86,400 bytes a page, which is what the
/// budgets below are counted in.
mod cache_budget {
    use super::*;

    /// One page of the comic fixture, decoded.
    const PAGE: usize = 120 * 180 * 4;

    fn comic(name: &str, budget: usize) -> Session {
        let dir = std::env::temp_dir().join(format!(
            "chapbook-budget-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::open_with(
            fixture("cbz/minimal.cbz"),
            SessionConfig::new(fixture_fonts())
                .with_library_dir(&dir)
                .with_cache_budget(budget),
        )
        .unwrap();
        session.set_metrics(chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(0.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        });
        session
    }

    #[test]
    fn the_default_applies_when_the_host_says_nothing() {
        let session = open_isolated("budget-default", &fixture("epub/minimal.epub"));
        assert_eq!(
            session.cache_budget(),
            chapbook_reader::DEFAULT_CACHE_BUDGET
        );
    }

    #[test]
    fn paging_a_comic_forward_stays_under_the_budget() {
        // Room for two pages, not three.
        let budget = PAGE * 2 + PAGE / 2;
        let mut session = comic("forward", budget);
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            assert!(
                session.cache_bytes() <= budget,
                "cache {} over budget {budget}",
                session.cache_bytes()
            );
            session.next_unit();
        }
    }

    #[test]
    fn the_unit_being_read_is_never_evicted() {
        // A budget nothing can fit. One page over is better than a reader
        // with nothing on screen.
        let mut session = comic("pinned", 1);
        let pixmap = render_loaded(&mut session);
        assert!(session.cache_bytes() > 0, "the current page survived");
        assert!(pixmap.width() > 0);
    }

    #[test]
    fn an_evicted_page_comes_back_the_same() {
        // Eviction is only safe because nothing cached is authoritative: a
        // comic page is re-read from the archive and decoded again. This is
        // that claim, checked.
        let mut session = comic("revisit", PAGE + PAGE / 2);
        let first = render_loaded(&mut session).data().to_vec();

        session.next_unit();
        render_loaded(&mut session);
        session.next_unit();
        render_loaded(&mut session);

        session.goto(chapbook_core::Locator {
            spine_index: 0,
            char_offset: 0,
        });
        let again = render_loaded(&mut session).data().to_vec();
        assert_eq!(first, again, "a re-decoded page is the same page");
    }

    #[test]
    fn lowering_the_budget_evicts_immediately() {
        // What a host does on a memory warning: it does not get to wait
        // for the next page turn.
        let mut session = comic("lowered", PAGE * 8);
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            session.next_unit();
        }
        assert!(session.cache_bytes() > PAGE, "several pages are held");

        session.set_cache_budget(PAGE + PAGE / 2);
        assert!(
            session.cache_bytes() <= PAGE + PAGE / 2,
            "cache {} did not shrink on the spot",
            session.cache_bytes()
        );
    }

    #[test]
    fn laid_out_chapters_count_against_it_too() {
        // Text is the smaller term — 0.3 MB a chapter against 15 MB a comic
        // page — but it accumulates the same way, so one budget covers
        // both rather than leaving a second unbounded cache behind.
        let dir =
            std::env::temp_dir().join(format!("chapbook-budget-test-{}-text", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let budget = 64 * 1024;
        let mut session = Session::open_with(
            fixture("epub/minimal.epub"),
            SessionConfig::new(fixture_fonts())
                .with_library_dir(&dir)
                .with_cache_budget(budget),
        )
        .unwrap();
        session.set_metrics(chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(20.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        });
        for _ in 0..session.spine_len() {
            session.render().expect("render");
            session.next_unit();
        }
        // The pinned unit may exceed it alone; nothing else may accumulate.
        assert!(
            session.cache_bytes() <= budget.max(PAGE),
            "text layouts accumulated to {}",
            session.cache_bytes()
        );
    }
}

/// The two calls a platform makes when it is telling you something:
/// "memory is short" and "you are about to be stopped".
mod lifecycle {
    use super::*;

    const PAGE: usize = 120 * 180 * 4;
    /// A retained unit is its decoded image *and* its laid-out page, so
    /// "one unit" is a little over one page's worth of pixels.
    const ONE_UNIT: usize = PAGE + 8 * 1024;

    fn dir_for(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chapbook-lifecycle-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn comic(name: &str) -> Session {
        let mut session = Session::open_with(
            fixture("cbz/minimal.cbz"),
            SessionConfig::new(fixture_fonts()).with_library_dir(dir_for(name)),
        )
        .unwrap();
        session.set_metrics(chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(0.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        });
        session
    }

    #[test]
    fn release_caches_keeps_the_page_on_screen_and_drops_the_rest() {
        let mut session = comic("release");
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            session.next_unit();
        }
        session.prev_unit();
        render_loaded(&mut session);
        let before = session.cache_bytes();
        assert!(before > PAGE, "more than one page is held: {before}");

        session.release_caches();
        assert!(
            session.cache_bytes() <= ONE_UNIT,
            "only the current unit should survive, held {}",
            session.cache_bytes()
        );
        // And the reader still has something to show, immediately.
        assert!(session.render().is_some());
    }

    #[test]
    fn what_release_drops_comes_back_identical() {
        let mut session = comic("release-refill");
        let first = render_loaded(&mut session).data().to_vec();
        session.next_unit();
        render_loaded(&mut session);

        session.release_caches();
        session.goto(chapbook_core::Locator {
            spine_index: 0,
            char_offset: 0,
        });
        assert_eq!(render_loaded(&mut session).data().to_vec(), first);
    }

    #[test]
    fn suspend_persists_the_position_and_lets_go_of_the_database() {
        let dir = dir_for("suspend");
        let source = fixture("epub/minimal.epub");
        let config = || SessionConfig::new(fixture_fonts()).with_library_dir(&dir);

        let mut session = Session::open_with(source.as_str(), config()).unwrap();
        session.set_metrics(chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(20.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        });
        session.next_unit();
        session.render();
        let where_we_were = session.locator();
        session.suspend();

        // Another session opening the same library sees the position,
        // which is only possible if suspend wrote it *and* released the
        // lock on the way out.
        let reopened = Session::open_with(source.as_str(), config()).unwrap();
        assert_eq!(reopened.locator().spine_index, where_we_were.spine_index);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_suspended_session_keeps_working() {
        // `onStop` is often followed by `onStart` with the process still
        // alive. A session that stopped saving after the first suspend
        // would lose every position from then on, silently.
        let mut session = comic("suspend-resume");
        render_loaded(&mut session);
        session.suspend();

        session.next_unit();
        assert!(render_loaded(&mut session).width() > 0, "still renders");
        session.save_position();
        session.suspend();
        assert!(session.render().is_some(), "and survives a second one");
    }

    #[test]
    fn suspending_gives_the_caches_back_too() {
        let mut session = comic("suspend-caches");
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            session.next_unit();
        }
        session.prev_unit();
        render_loaded(&mut session);
        assert!(session.cache_bytes() > PAGE);

        session.suspend();
        assert!(
            session.cache_bytes() <= ONE_UNIT,
            "a stopped app should not hold decoded pages: {}",
            session.cache_bytes()
        );
    }
}

/// A restored position lands in `frame()`, because an offset cannot become
/// a page until the unit has laid out. Nothing obliges a shell to paint
/// before it navigates, though — a batched turn, or the conformance
/// harness, walks with no frame in between — and the restore was then
/// applied to whatever unit the reader had reached by the time one
/// arrived, resolving one chapter's offset against another chapter's
/// pages and moving the reader without being asked.
///
/// Found by the Android spike, but nothing about it is Android: running
/// `examples/conform` twice against the same library failed the second
/// time, because the first run left a position for the second to restore.
/// "The end of the book stands still" was the check that broke — a
/// `frame()` there moved the reader backwards, and the turn that had just
/// refused then worked.
///
/// Swept over stopping points rather than aimed at one, because whether a
/// misapplied offset is *visible* depends on where it happens to land: an
/// offset from unit 4 resolved against unit 9 sometimes names the page the
/// reader was already on. The bug is the same either way, so the test
/// asks the invariant at every stop instead of picking a lucky one.
#[test]
fn a_restore_does_not_follow_the_reader_into_another_unit() {
    let source = fixture("corpus/accessible_epub_3.epub");
    let saved = {
        let mut s = open_isolated("restore-follows", &source);
        s.set_metrics(metrics());
        for _ in 0..6 {
            s.next_page();
            let _ = s.frame();
        }
        let at = s.position();
        assert!(at.spine > 0 || at.page > 0, "moved off the first page");
        s.save_position();
        at
    };

    // One session per stopping point: the pending restore is consumed by
    // the first frame, so each session affords exactly one observation.
    for turns in 1..40 {
        let mut s = reopen_isolated("restore-follows", &source);
        s.set_metrics(metrics());
        assert_eq!(s.spine(), saved.spine, "reopened in the restored unit");

        // Walk without ever painting, so the restore is still pending.
        for _ in 0..turns {
            if !s.next_page() {
                break;
            }
        }
        let before = s.position();
        if before.spine == saved.spine {
            continue; // still in the restored unit: landing there is correct
        }

        let _ = s.frame();
        assert_eq!(
            s.position(),
            before,
            "after {turns} turns, a pending restore from unit {} moved the \
             reader inside unit {}",
            saved.spine,
            before.spine
        );
    }
}

#[test]
fn actions_route_to_the_verbs_a_shell_would_have_called_by_hand() {
    use chapbook_core::Action;

    let mut s = open_isolated("epub-apply-nav", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    let start = s.locator();
    assert!(s.apply(Action::NextPage), "the book has somewhere to go");
    let after = s.locator();
    assert_ne!(after, start, "and it went there");

    assert!(s.apply(Action::PrevPage));
    assert_eq!(s.locator(), start, "back where it started");

    // The honest `false` a shell's redraw check depends on: nothing before
    // the first page, so nothing moved and nothing needs repainting.
    assert!(!s.apply(Action::PrevPage), "no page before the first");
    assert_eq!(s.locator(), start);

    if s.spine_len() > 1 {
        assert!(s.apply(Action::NextUnit));
        assert_eq!(s.page(), 0, "a unit skip lands on its first page");
        assert!(s.apply(Action::PrevUnit));
    }
    assert!(!s.apply(Action::PrevUnit), "no unit before the first");
}

#[test]
fn font_actions_step_by_the_engines_amount_and_stop_at_the_clamp() {
    use chapbook_core::Action;
    use chapbook_reader::FONT_STEP_PX;

    let mut s = open_isolated("epub-apply-font", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    let start = s.settings().base_font_px;
    assert!(s.apply(Action::FontUp));
    assert_eq!(s.settings().base_font_px, start + FONT_STEP_PX);
    assert!(s.apply(Action::FontDown));
    assert_eq!(s.settings().base_font_px, start, "and back down again");

    // `adjust_font` clamps, so at the stop `apply` has to say nothing
    // moved rather than ask for a redraw of an identical page. Bounded
    // well above the number of steps the 10–40 range can hold.
    let mut steps = 0;
    while s.apply(Action::FontDown) {
        steps += 1;
        assert!(steps < 100, "the clamp never arrived");
    }
    let floor = s.settings().base_font_px;
    assert!(!s.apply(Action::FontDown), "still refused at the floor");
    assert_eq!(s.settings().base_font_px, floor, "and did not drift");
    assert!(s.apply(Action::FontUp), "the other direction still moves");
}

#[test]
fn cycling_the_theme_is_an_action_and_the_menu_is_not_the_engines() {
    use chapbook_core::{Action, Theme};

    let mut s = open_isolated("epub-apply-misc", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    let start = s.settings().theme;
    assert!(s.apply(Action::CycleTheme));
    assert_eq!(s.settings().theme, start.cycle());

    // Three variants, so the cycle comes home and every step redraws.
    assert!(s.apply(Action::CycleTheme));
    assert!(s.apply(Action::CycleTheme));
    assert_eq!(s.settings().theme, start);
    assert_eq!(Theme::default().cycle().cycle().cycle(), Theme::default());

    // The engine has no chrome to toggle. `false` here means "not mine",
    // and the shell is expected to have matched for it first.
    let before = s.locator();
    assert!(!s.apply(Action::ToggleMenu));
    assert_eq!(s.locator(), before, "and it touched nothing on the way");
}
