//! Pinch zoom on image books: the transform, its clamps, and the refusal
//! that routes the gesture to font size on reflowable text.

mod common;

use chapbook_reader::chapbook_paint::DisplayOp;
use chapbook_reader::Session;
use common::{fixture, open_isolated, render_loaded};

fn comic() -> Session {
    let mut s = open_isolated("zoom-comic", &fixture("cbz/minimal.cbz"));
    s.set_metrics(common::metrics());
    render_loaded(&mut s);
    s
}

/// The page image's destination rect in the current frame.
fn image_dest(s: &mut Session) -> chapbook_reader::chapbook_core::Rect {
    let frame = s.frame().expect("a loaded page frames");
    frame
        .list
        .ops
        .iter()
        .find_map(|op| match op {
            DisplayOp::Image { dest, .. } => Some(*dest),
            _ => None,
        })
        .expect("an image book's page is an image")
}

#[test]
fn reflowable_text_refuses_the_gesture() {
    let mut s = open_isolated("zoom-epub", &fixture("epub/minimal.epub"));
    s.set_metrics(common::metrics());
    s.render();
    assert!(
        !s.set_page_zoom(2.0, 100.0, 100.0),
        "pinch on prose is a font-size gesture, the shell's to map"
    );
    assert_eq!(s.page_zoom(), 1.0);
    assert!(!s.pan_page(10.0, 10.0));
}

#[test]
fn zoom_scales_the_page_and_anchors_the_focus() {
    let mut s = comic();
    let fit = image_dest(&mut s);

    // Zoom around the fitted image's own center.
    let (fx, fy) = (
        fit.origin.x + fit.size.w / 2.0,
        fit.origin.y + fit.size.h / 2.0,
    );
    assert!(s.set_page_zoom(2.0, fx, fy));
    assert_eq!(s.page_zoom(), 2.0);
    let zoomed = image_dest(&mut s);
    assert!(
        (zoomed.size.w - fit.size.w * 2.0).abs() < 0.5,
        "the page doubled"
    );

    // The content under the focal point stayed under it: the fit-space
    // focus maps forward to the same panel point.
    let (pan_x, pan_y) = s.page_pan();
    let forward_x = fx * 2.0 + pan_x;
    let forward_y = fy * 2.0 + pan_y;
    assert!(
        (forward_x - fx).abs() < 0.5 && (forward_y - fy).abs() < 0.5,
        "focus drifted: ({forward_x}, {forward_y}) vs ({fx}, {fy})"
    );
}

#[test]
fn pan_moves_and_the_edges_hold() {
    let mut s = comic();
    assert!(s.set_page_zoom(2.0, 200.0, 300.0));
    assert!(s.pan_page(-40.0, -40.0), "a zoomed page pans");
    let (px, py) = s.page_pan();

    // Dragging a mile past the corner stops at the corner: the pan never
    // opens a gap between page edge and panel edge.
    s.pan_page(-100_000.0, -100_000.0);
    let (min_x, min_y) = {
        let m = common::metrics();
        (m.size.w * (1.0 - 2.0), m.size.h * (1.0 - 2.0))
    };
    assert_eq!(s.page_pan(), (min_x, min_y));
    s.pan_page(100_000.0, 100_000.0);
    assert_eq!(s.page_pan(), (0.0, 0.0));
    assert!((px, py) > (min_x, min_y) && (px, py) < (0.0, 0.0));
}

#[test]
fn fit_is_a_zoom_of_one_and_turns_keep_the_view() {
    let mut s = comic();
    assert!(s.set_page_zoom(3.0, 100.0, 100.0));

    // A turn keeps the magnification — manga readers page through
    // zoomed, and a shell that wants turn-resets asks for one.
    s.next_page();
    assert_eq!(s.page_zoom(), 3.0);

    assert!(
        s.set_page_zoom(1.0, 0.0, 0.0),
        "returning to fit is a change"
    );
    assert_eq!(s.page_zoom(), 1.0);
    assert_eq!(s.page_pan(), (0.0, 0.0));
    assert!(
        !s.set_page_zoom(0.5, 0.0, 0.0),
        "below fit clamps to fit, already there"
    );
}
