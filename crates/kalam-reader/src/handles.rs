//! kalam: the two selection handles — the old reader's
//! `.kalam-selection-handle-start` / `-end`, drawn by the widget.
//!
//! A handle is a 2 px bar in the theme's handle colour, as tall as the
//! selection band at that end, standing on the band's outer edge: the
//! left edge of the first band for the start, the right edge of the last
//! band for the end. A 5 px teardrop grip sits at the bar's outer end —
//! above the start bar, below the end bar — pointing at the text. The
//! hit area is 24 px wide and reaches 12 px past either end of the bar,
//! so a finger or a rough pointer catches it; only the bar and the
//! teardrop are painted.
//!
//! The geometry lives here, in widget coordinates, so that the press
//! handler and the painter agree on what a handle is. Both take the end
//! rects the widget already computes for `SelectedText`.

use chapbook_core::{Point, Rect, Rgba};
use chapbook_reader::tiny_skia::{
    self, FillRule, Paint, PathBuilder, Pixmap, Rect as SkRect, Transform,
};

/// Bar width, CSS px.
const BAR_WIDTH: f32 = 2.0;
/// Teardrop grip size, CSS px (its bounding square).
const GRIP: f32 = 5.0;
/// Hit area width, CSS px.
const HIT_WIDTH: f32 = 24.0;
/// How far the hit area reaches past each end of the bar, CSS px.
const HIT_REACH: f32 = 12.0;

/// Which end of the selection a handle moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Start,
    End,
}

/// One handle: where its bar stands, in widget coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Handle {
    pub edge: Edge,
    /// The bar's x — its centre line.
    pub x: f32,
    /// The bar's vertical extent: the band's.
    pub top: f32,
    pub bottom: f32,
}

impl Handle {
    /// The handles for a selection whose first band is `first` and last
    /// band is `last` (the same rect for a one-line selection).
    pub(crate) fn pair(first: Rect, last: Rect) -> [Handle; 2] {
        [
            Handle {
                edge: Edge::Start,
                x: first.min_x(),
                top: first.min_y(),
                bottom: first.max_y(),
            },
            Handle {
                edge: Edge::End,
                x: last.max_x(),
                top: last.min_y(),
                bottom: last.max_y(),
            },
        ]
    }

    /// The area a press lands in to take hold of this handle.
    pub(crate) fn hit_rect(&self) -> Rect {
        Rect::new(
            self.x - HIT_WIDTH / 2.0,
            self.top - HIT_REACH,
            HIT_WIDTH,
            self.bottom - self.top + 2.0 * HIT_REACH,
        )
    }

    pub(crate) fn hit(&self, x: f32, y: f32) -> bool {
        let r = self.hit_rect();
        x >= r.min_x() && x <= r.max_x() && y >= r.min_y() && y <= r.max_y()
    }

    /// The bar's painted rect (before device scaling).
    fn bar(&self) -> Rect {
        Rect::new(
            self.x - BAR_WIDTH / 2.0,
            self.top,
            BAR_WIDTH,
            self.bottom - self.top,
        )
    }

    /// Where the teardrop's tip meets the bar: the outer end.
    fn grip_tip(&self) -> Point {
        match self.edge {
            Edge::Start => Point::new(self.x, self.top),
            Edge::End => Point::new(self.x, self.bottom),
        }
    }
}

/// Which handle, if any, a press at widget (x, y) takes hold of. When the
/// two overlap — a selection one glyph wide — the end handle wins, which
/// is the one a reader who just dragged rightwards is reaching for.
pub(crate) fn handle_at(handles: &[Handle; 2], x: f32, y: f32) -> Option<Edge> {
    if handles[1].hit(x, y) {
        Some(Edge::End)
    } else if handles[0].hit(x, y) {
        Some(Edge::Start)
    } else {
        None
    }
}

/// Paint both handles onto the frame, `scale` device pixels per CSS px.
pub(crate) fn paint(out: &mut Pixmap, handles: &[Handle; 2], color: Rgba, scale: f32) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, color.a);
    let to_device = Transform::from_scale(scale, scale);
    for handle in handles {
        // The bar, on whole device pixels so it is crisp at 2 px.
        let bar = handle.bar();
        let x = (bar.min_x() * scale).round() / scale;
        let w = (bar.size.w * scale).round().max(1.0) / scale;
        paint.anti_alias = false;
        if let Some(rect) = SkRect::from_xywh(x, bar.min_y(), w, bar.size.h) {
            out.fill_rect(rect, &paint, to_device, None);
        }
        // The teardrop: a circle whose outer quadrant is squared off into
        // a point that touches the bar's end — the CSS's
        // `border-radius: 50% 50% 50% 0` rotated 45°, so the corner
        // points along the bar.
        paint.anti_alias = true;
        if let Some(path) = teardrop(handle.grip_tip(), handle.edge) {
            out.fill_path(&path, &paint, FillRule::Winding, to_device, None);
        }
    }
}

/// A teardrop of [`GRIP`] px whose point is at `tip`, hanging away from
/// the text: upward for the start handle, downward for the end.
///
/// A circle of radius `GRIP / 2` whose centre sits `r·√2` from the tip
/// along the bar's axis, so the tip is a corner of the square around it
/// and the two edges from the tip are tangents; the rest is the far
/// three quarters of the circle. The CSS was `border-radius: 50% 50% 50%
/// 0` turned 45°, which is this shape.
fn teardrop(tip: Point, edge: Edge) -> Option<tiny_skia::Path> {
    let r = GRIP / 2.0;
    let dir = match edge {
        Edge::Start => -1.0,
        Edge::End => 1.0,
    };
    let c = Point::new(tip.x, tip.y + dir * r * std::f32::consts::SQRT_2);
    // Tangent points: 45° either side of the axis, on the tip's side.
    let t = r * std::f32::consts::FRAC_1_SQRT_2;
    let left = Point::new(c.x - t, c.y - dir * t);
    let right = Point::new(c.x + t, c.y - dir * t);
    let far = Point::new(c.x, c.y + dir * r);
    let side_l = Point::new(c.x - r, c.y);
    let side_r = Point::new(c.x + r, c.y);
    let mut pb = PathBuilder::new();
    pb.move_to(tip.x, tip.y);
    pb.line_to(left.x, left.y);
    // Round the far side: left → side → far pole → side → right, as
    // arcs of 45°, 90°, 90°, 45°.
    arc_to(&mut pb, c, r, left, side_l);
    arc_to(&mut pb, c, r, side_l, far);
    arc_to(&mut pb, c, r, far, side_r);
    arc_to(&mut pb, c, r, side_r, right);
    pb.close();
    pb.finish()
}

/// One circular arc from `a` to `b`, both on the circle (`c`, `r`), the
/// short way round, as a cubic. The control-point distance for an arc of
/// angle θ is `4/3 · tan(θ/4) · r`.
fn arc_to(pb: &mut PathBuilder, c: Point, r: f32, a: Point, b: Point) {
    let (ax, ay) = (a.x - c.x, a.y - c.y);
    let (bx, by) = (b.x - c.x, b.y - c.y);
    let theta = ((ax * bx + ay * by) / (r * r)).clamp(-1.0, 1.0).acos();
    let h = 4.0 / 3.0 * (theta / 4.0).tan();
    // Tangents are perpendicular to the radii, turned towards `b`.
    let s = if ax * by - ay * bx >= 0.0 { 1.0 } else { -1.0 };
    let (tax, tay) = (-ay * s, ax * s);
    let (tbx, tby) = (-by * s, bx * s);
    pb.cubic_to(
        a.x + tax * h,
        a.y + tay * h,
        b.x - tbx * h,
        b.y - tby * h,
        b.x,
        b.y,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> [Handle; 2] {
        Handle::pair(
            Rect::new(100.0, 200.0, 300.0, 24.0),
            Rect::new(60.0, 260.0, 120.0, 24.0),
        )
    }

    #[test]
    fn handles_stand_on_the_outer_edges_of_the_first_and_last_band() {
        let [start, end] = pair();
        assert_eq!((start.x, start.top, start.bottom), (100.0, 200.0, 224.0));
        assert_eq!((end.x, end.top, end.bottom), (180.0, 260.0, 284.0));
    }

    #[test]
    fn the_hit_area_is_wider_and_taller_than_the_bar() {
        let handles = pair();
        // 12 px either side of the bar, 12 px past each end.
        assert_eq!(handle_at(&handles, 88.5, 190.0), Some(Edge::Start));
        assert_eq!(handle_at(&handles, 111.5, 235.0), Some(Edge::Start));
        assert_eq!(handle_at(&handles, 87.0, 210.0), None);
        assert_eq!(handle_at(&handles, 100.0, 187.0), None);
        assert_eq!(handle_at(&handles, 180.0, 295.0), Some(Edge::End));
        // Text in the middle of the selection is not a handle.
        assert_eq!(handle_at(&handles, 250.0, 212.0), None);
    }

    #[test]
    fn overlapping_handles_prefer_the_end() {
        let one = Rect::new(100.0, 200.0, 4.0, 24.0);
        let handles = Handle::pair(one, one);
        assert_eq!(handle_at(&handles, 102.0, 212.0), Some(Edge::End));
    }

    #[test]
    fn painting_marks_the_bar_and_the_grip_and_nothing_else() {
        let mut out = Pixmap::new(400, 400).unwrap();
        let handles = pair();
        paint(&mut out, &handles, Rgba::new(11, 11, 11, 255), 1.0);
        let dark = |x: u32, y: u32| out.pixel(x, y).unwrap().alpha() > 0;
        // On the start bar, mid-height.
        assert!(dark(100, 212) || dark(99, 212));
        // In the start grip, just above the bar's top.
        assert!(dark(100, 197));
        // In the end grip, just below the bar's bottom.
        assert!(dark(180, 287));
        // Well clear of both.
        assert!(!dark(250, 212));
        assert!(!dark(100, 180));
        assert!(!dark(180, 300));
    }
}
