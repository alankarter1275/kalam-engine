//! The frame contract: display lists, intent and damage, dither
//! regions, pixel formats, and the turned panel.

mod common;
use chapbook_core::Rotation;
use common::*;

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
#[cfg(feature = "cbz")]
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
    // versus a full-page flash. (kalam: the host stores the mark and
    // shows it; the frame contract is the same as upstream's.)
    let (start, end) = s.selected_range().expect("a selection to mark");
    let row = chapbook_reader::HostHighlight {
        id: 1,
        start: s.layered_locator_at(start).expect("start captures"),
        end: s.layered_locator_at(end).expect("end captures"),
        color: None,
        text: s.selected_text(),
    };
    s.show_host_highlight(row);
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
#[cfg(feature = "cbz")]
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
#[cfg(feature = "cbz")]
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
