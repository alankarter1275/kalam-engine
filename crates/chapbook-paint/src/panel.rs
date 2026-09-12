//! Panel policy: what a rasterized page has to become before it reaches a
//! particular screen.
//!
//! Orientation is a property of the target, not of the rasterizer that drew
//! the page — a shell on an e-ink device gets the same answer whether its
//! pages come from the CPU backend or a GPU one, and two shells on the same
//! device agree with each other. So the turn lives here, above both
//! backends.
//!
//! Reduction — greyscale, levels, where the dithering goes — used to live
//! here too and is now [`mezzotint::encode`], because it is arithmetic
//! about panels and has nothing to do with pages. What stays is the pair
//! that does: [`rotate`] turns the pixels, and [`panel_rect`] turns a rect
//! the same way, so a damage region and the pixels under it agree.
//!
//! [`rotate`] operates on rows of RGBA8, top-left origin, as every backend
//! produces. Pages paint over an opaque background, so premultiplied and
//! straight values coincide here and it does not have to care which it was
//! handed.

use std::borrow::Cow;

use chapbook_core::{PanelRect, Rect, Rotation, Size};

/// Map a page-coordinate rect (CSS px, page orientation) onto the panel:
/// scale to device pixels, turn by `rotation`, round outward, clamp to the
/// panel.
///
/// `page` is the page size in CSS px — the same `Size` the display list
/// carries — so the caller never has to work out the rotated panel bounds
/// itself.
///
/// This is [`rotate`]'s forward turn applied to a span rather than to a
/// pixel, and it lives beside it for that reason: the two have to agree, and
/// a rect that disagrees with its pixels states damage for a region the
/// panel did not repaint. `quarter_turn_matches_the_pixel_rotation` is
/// where that agreement is pinned down.
///
/// Rounding is outward, always. A region trimmed by half a pixel leaves a
/// stale sliver on screen, and on e-ink a stale sliver stays there until
/// something else disturbs it.
pub fn panel_rect(rect: Rect, page: Size, scale: f32, rotation: Rotation) -> PanelRect {
    let (pw, ph) = (page.w * scale, page.h * scale);
    let (x0, y0) = (rect.min_x() * scale, rect.min_y() * scale);
    let (x1, y1) = (rect.max_x() * scale, rect.max_y() * scale);

    // The two corners swap roles on the axes the rotation reverses.
    let (a0, b0, a1, b1) = match rotation {
        Rotation::None => (x0, y0, x1, y1),
        Rotation::Quarter => (ph - y1, x0, ph - y0, x1),
        Rotation::Half => (pw - x1, ph - y1, pw - x0, ph - y0),
        Rotation::ThreeQuarter => (y0, pw - x1, y1, pw - x0),
    };
    let (bound_w, bound_h) = if rotation.swaps_axes() {
        (ph, pw)
    } else {
        (pw, ph)
    };

    let left = a0.floor().clamp(0.0, bound_w) as u32;
    let top = b0.floor().clamp(0.0, bound_h) as u32;
    let right = a1.ceil().clamp(0.0, bound_w) as u32;
    let bottom = b1.ceil().clamp(0.0, bound_h) as u32;
    PanelRect::new(
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

/// Turn a rasterized page for a panel mounted in a different orientation,
/// clockwise. Quarter turns swap the page's dimensions, so the returned
/// rows are `height` wide.
///
/// [`Rotation::None`] borrows: an unrotated shell is the common case — a
/// device is mounted the way it is mounted — and it was paying a full-page
/// allocation and copy every frame to be handed back what it already had.
/// Callers that genuinely need to own the pixels say `into_owned()`, which
/// costs nothing on the turned paths because those already allocated.
///
/// The inverse for input is [`chapbook_core::PageMetrics::panel_to_page`],
/// so a shell that turns its output here reads pointer coordinates back
/// through that.
pub fn rotate(rgba: &[u8], width: u32, height: u32, rotation: Rotation) -> Cow<'_, [u8]> {
    if rotation == Rotation::None {
        return Cow::Borrowed(rgba);
    }
    let (w, h) = (width as usize, height as usize);
    let dw = if rotation.swaps_axes() { h } else { w };
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = match rotation {
                Rotation::None => (x, y),
                Rotation::Quarter => (h - 1 - y, x),
                Rotation::Half => (w - 1 - x, h - 1 - y),
                Rotation::ThreeQuarter => (y, w - 1 - x),
            };
            let (from, to) = ((y * w + x) * 4, (dy * dw + dx) * 4);
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-pixel page: red then blue.
    fn pair() -> Vec<u8> {
        vec![255, 0, 0, 255, 0, 0, 255, 255]
    }

    fn at(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    // ---- panel_rect ----

    #[test]
    fn unrotated_page_rect_scales_to_device_pixels() {
        let r = panel_rect(
            Rect::new(10.0, 20.0, 30.0, 40.0),
            Size::new(100.0, 200.0),
            2.0,
            Rotation::None,
        );
        assert_eq!(r, PanelRect::new(20, 40, 60, 80));
    }

    #[test]
    fn fractional_edges_round_outward() {
        // A region trimmed by rounding leaves a stale sliver that e-ink
        // holds until something else disturbs it, so both edges must grow.
        let r = panel_rect(
            Rect::new(10.4, 20.6, 5.3, 5.1),
            Size::new(100.0, 200.0),
            1.0,
            Rotation::None,
        );
        assert_eq!(r.x, 10);
        assert_eq!(r.y, 20);
        assert!(r.max_x() >= 16, "right edge {} lost a pixel", r.max_x());
        assert!(r.max_y() >= 26, "bottom edge {} lost a pixel", r.max_y());
    }

    #[test]
    fn quarter_turn_matches_the_pixel_rotation() {
        // The span mapping has to agree with `rotate`, which sends page
        // (x, y) to panel (h - 1 - y, x). A rect in the page's top-left
        // lands in the panel's top-right.
        let page = Size::new(100.0, 200.0);
        let r = panel_rect(
            Rect::new(0.0, 0.0, 10.0, 20.0),
            page,
            1.0,
            Rotation::Quarter,
        );
        assert_eq!(r, PanelRect::new(180, 0, 20, 10));
    }

    #[test]
    fn every_turn_stays_inside_the_panel() {
        let page = Size::new(100.0, 200.0);
        let rect = Rect::new(7.0, 11.0, 33.0, 44.0);
        for rotation in [
            Rotation::None,
            Rotation::Quarter,
            Rotation::Half,
            Rotation::ThreeQuarter,
        ] {
            let r = panel_rect(rect, page, 1.5, rotation);
            let (pw, ph) = (150, 300);
            let (bw, bh) = if rotation.swaps_axes() {
                (ph, pw)
            } else {
                (pw, ph)
            };
            assert!(r.max_x() <= bw, "{rotation:?} overflows width: {r:?}");
            assert!(r.max_y() <= bh, "{rotation:?} overflows height: {r:?}");
            assert!(!r.is_empty(), "{rotation:?} collapsed: {r:?}");
        }
    }

    #[test]
    fn turns_preserve_area() {
        let page = Size::new(100.0, 200.0);
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        let area = |rotation| {
            let r = panel_rect(rect, page, 1.0, rotation);
            r.w * r.h
        };
        for rotation in [Rotation::Quarter, Rotation::Half, Rotation::ThreeQuarter] {
            assert_eq!(area(rotation), area(Rotation::None), "{rotation:?}");
        }
    }

    // ---- rotate ----

    #[test]
    fn a_quarter_turn_swaps_the_axes() {
        let page = pair();
        let turned = rotate(&page, 2, 1, Rotation::Quarter);
        // Clockwise: the left pixel goes to the top of a 1x2 page.
        assert_eq!(at(&turned, 1, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&turned, 1, 0, 1), [0, 0, 255, 255]);
    }

    #[test]
    fn a_half_turn_keeps_the_shape_and_reverses_it() {
        let page = pair();
        let turned = rotate(&page, 2, 1, Rotation::Half);
        assert_eq!(at(&turned, 2, 0, 0), [0, 0, 255, 255]);
        assert_eq!(at(&turned, 2, 1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn four_quarter_turns_come_back() {
        let original = pair();
        let mut turned = original.clone();
        let (mut w, mut h) = (2u32, 1u32);
        for _ in 0..4 {
            turned = rotate(&turned, w, h, Rotation::Quarter).into_owned();
            std::mem::swap(&mut w, &mut h);
        }
        assert_eq!(turned, original);
    }

    #[test]
    fn none_hands_back_the_same_pixels_without_copying() {
        let original = pair();
        let turned = rotate(&original, 2, 1, Rotation::None);
        assert_eq!(turned.as_ref(), original.as_slice());
        // And it is the caller's own buffer, not a duplicate of it: an
        // unrotated shell allocates nothing per frame here.
        assert!(matches!(turned, Cow::Borrowed(_)));
        assert!(std::ptr::eq(turned.as_ref().as_ptr(), original.as_ptr()));
    }

    #[test]
    fn a_turn_owns_what_it_builds() {
        let original = pair();
        let turned = rotate(&original, 2, 1, Rotation::Quarter);
        assert!(matches!(turned, Cow::Owned(_)));
    }
}
