//! Pinch zoom for image books — the vocabulary a touch shell needs
//! before comics and PDFs are readable under fingers.
//!
//! Zoom is **view state, not reading state**: it changes which pixels of
//! the fitted page fill the panel, not what the page is, so nothing here
//! persists, syncs, or moves the position. It applies to image books
//! only. On reflowable text the same gesture means "make the text
//! bigger", which is a *settings* change — [`Session::apply`] with
//! `FontUp`/`FontDown` — and the shell owns that mapping;
//! [`Session::set_page_zoom`] answering `false` is how it knows to.
//!
//! The transform is the photo viewer's: a content point `c` shows at
//! `c * zoom + pan`, the pan clamped so the page always covers the panel
//! — no gaps at the edges, zoom anchored at the focal point so the
//! spot under the pinch stays under the pinch. Zoom survives a page
//! turn on purpose: manga readers page through at a fixed magnification,
//! and a shell that wants turn-resets calls `set_page_zoom(1.0, ..)` on
//! turn — composable policy rather than baked-in.
//!
//! Input crossing the session ([`Session::word_at`], selection, links,
//! highlights) is mapped through the inverse automatically. Output
//! geometry — [`Session::range_rects`], the text surface — deliberately
//! stays in fit-page space, the space every consumer already agrees on;
//! a shell drawing over a zoomed page maps a rect forward with
//! [`Session::page_zoom`] and [`Session::page_pan`], which is one
//! multiply and one add per corner.
//!
//! Sharpness has a known ceiling for PDFs: pages rasterize at a fixed
//! 2× and zooming past that shows it. The fix is a scale-aware
//! rasterization request to the loader, which is engine work this
//! vocabulary does not wait for. Comics decode at native resolution and
//! stay sharp until it.

use chapbook_core::{BookKind, Size};
#[cfg(any(feature = "library", test))]
use chapbook_core::{Point, Rect};

use crate::Session;

/// How far in a pinch may go. Past this a comic page is mostly grain.
const MAX_PAGE_ZOOM: f32 = 8.0;

/// Hold the pan inside the page, so the content always covers the panel.
///
/// At zoom `z` the content spans `[pan, pan + size * z]` and the panel
/// wants `[0, size]`, so `pan` runs from `size * (1 - z)` to zero. Written
/// once because it is needed in three places and wrong in all of them if
/// it disagrees with itself.
fn clamp_pan(x: f32, y: f32, zoom: f32, size: Size) -> (f32, f32) {
    (
        x.clamp(size.w * (1.0 - zoom), 0.0),
        y.clamp(size.h * (1.0 - zoom), 0.0),
    )
}

/// The zoomed view: `content * zoom + pan` fills the panel.
pub(crate) struct PageView {
    pub(crate) zoom: f32,
    pub(crate) pan_x: f32,
    pub(crate) pan_y: f32,
}

/// A fit-page rect in a view's coordinates: `view = fit * zoom + pan`,
/// the same forward map a shell is told to apply to its own overlays.
///
/// Reached only through `Session::view_rect`, whose one caller is
/// `library`-gated; the tests below exercise it directly.
#[cfg(any(feature = "library", test))]
fn map_rect(rect: Rect, view: &PageView) -> Rect {
    Rect {
        origin: Point::new(
            rect.origin.x * view.zoom + view.pan_x,
            rect.origin.y * view.zoom + view.pan_y,
        ),
        size: Size::new(rect.size.w * view.zoom, rect.size.h * view.zoom),
    }
}

impl Session {
    fn is_image_book(&self) -> bool {
        matches!(self.kind(), BookKind::Comic | BookKind::Pdf)
    }

    /// The current zoom, 1.0 at fit.
    pub fn page_zoom(&self) -> f32 {
        self.view.as_ref().map_or(1.0, |view| view.zoom)
    }

    /// The current pan, in page units — with [`Session::page_zoom`], the
    /// forward map for a shell drawing its own overlays on a zoomed page.
    pub fn page_pan(&self) -> (f32, f32) {
        self.view
            .as_ref()
            .map_or((0.0, 0.0), |view| (view.pan_x, view.pan_y))
    }

    /// Zoom the page around a focal point in panel coordinates — the
    /// pinch. Clamped to `[1.0, 8.0]`; at 1.0 the view returns to fit.
    /// Returns whether anything changed, and always `false` for
    /// reflowable text, where the same gesture is a font-size change the
    /// shell maps itself.
    pub fn set_page_zoom(&mut self, zoom: f32, focus_x: f32, focus_y: f32) -> bool {
        if !self.is_image_book() {
            return false;
        }
        let Some(metrics) = self.metrics else {
            return false;
        };
        let zoom = if zoom.is_finite() {
            zoom.clamp(1.0, MAX_PAGE_ZOOM)
        } else {
            1.0
        };
        if zoom <= 1.0 {
            let changed = self.view.take().is_some();
            if changed {
                // The whole page's pixels change — the same disturbance a
                // turn is, until FrameIntent grows a word for zoom.
                self.mark(chapbook_paint::FrameIntent::PageTurn);
            }
            return changed;
        }
        let (fx, fy) = metrics.panel_to_page(focus_x, focus_y);
        let (old_zoom, old_pan_x, old_pan_y) = match &self.view {
            Some(view) => (view.zoom, view.pan_x, view.pan_y),
            None => (1.0, 0.0, 0.0),
        };
        // Keep the content under the focal point under the focal point.
        let (cx, cy) = ((fx - old_pan_x) / old_zoom, (fy - old_pan_y) / old_zoom);
        let (pan_x, pan_y) = clamp_pan(fx - cx * zoom, fy - cy * zoom, zoom, metrics.size);
        let changed = match &self.view {
            Some(view) => view.zoom != zoom || view.pan_x != pan_x || view.pan_y != pan_y,
            None => true,
        };
        self.view = Some(PageView { zoom, pan_x, pan_y });
        if changed {
            self.mark(chapbook_paint::FrameIntent::PageTurn);
        }
        changed
    }

    /// Pan the zoomed page by a pointer delta in panel coordinates,
    /// clamped to the page's edges. Returns whether the view moved —
    /// `false` at fit, which is how a shell knows the same drag should
    /// fall through to whatever an unzoomed drag means (a selection, a
    /// swipe turn).
    pub fn pan_page(&mut self, dx: f32, dy: f32) -> bool {
        let Some(metrics) = self.metrics else {
            return false;
        };
        let Some(view) = &self.view else {
            return false;
        };
        // Deltas through the same map as positions, so a rotated panel
        // pans the way the finger moved: map two points, keep the
        // difference.
        let (ax, ay) = metrics.panel_to_page(0.0, 0.0);
        let (bx, by) = metrics.panel_to_page(dx, dy);
        let (pan_x, pan_y) = clamp_pan(
            view.pan_x + (bx - ax),
            view.pan_y + (by - ay),
            view.zoom,
            metrics.size,
        );
        let changed = pan_x != view.pan_x || pan_y != view.pan_y;
        if changed {
            self.view = Some(PageView {
                zoom: view.zoom,
                pan_x,
                pan_y,
            });
            self.mark(chapbook_paint::FrameIntent::PageTurn);
        }
        changed
    }

    /// Re-clamp the pan after the page box changed.
    ///
    /// The clamp is a function of the panel's size, so a pan that was
    /// legal at one size is not at a smaller one: shrink the window while
    /// zoomed and the old pan leaves a gap between the page's edge and the
    /// panel's — the one thing the clamp exists to prevent. Nothing else
    /// re-checks it, because `set_page_zoom` and `pan_page` clamp on the
    /// way in and a resize goes through neither.
    ///
    /// View state only: no relayout, no position, nothing persisted.
    pub(crate) fn reclamp_view(&mut self) {
        let (Some(metrics), Some(view)) = (self.metrics, &self.view) else {
            return;
        };
        let (pan_x, pan_y) = clamp_pan(view.pan_x, view.pan_y, view.zoom, metrics.size);
        if pan_x != view.pan_x || pan_y != view.pan_y {
            self.view = Some(PageView {
                zoom: view.zoom,
                pan_x,
                pan_y,
            });
            self.mark(chapbook_paint::FrameIntent::PageTurn);
        }
    }

    /// A panel point in fit-page content coordinates — the one door
    /// every hit test walks through, so zoom cannot be forgotten by a
    /// single call site.
    pub(crate) fn content_point(&self, x: f32, y: f32) -> (f32, f32) {
        let (px, py) = self.metrics.map_or((x, y), |m| m.panel_to_page(x, y));
        match &self.view {
            Some(view) => ((px - view.pan_x) / view.zoom, (py - view.pan_y) / view.zoom),
            None => (px, py),
        }
    }

    /// A fit-page rect in the zoomed view's coordinates — the forward map
    /// a shell is told to apply to its own overlays, applied here to the
    /// one rect the engine itself hands out in that space.
    ///
    /// Its one caller is `range_damage`, which is `library`-gated, so the
    /// mapping shares the gate rather than reading as dead code without it.
    #[cfg(feature = "library")]
    pub(crate) fn view_rect(&self, rect: Rect) -> Rect {
        match &self.view {
            Some(view) => map_rect(rect, view),
            None => rect,
        }
    }

    /// Scale a display list into the zoomed view. The first op is the
    /// page ground and keeps covering the panel; everything after it is
    /// content and moves.
    pub(crate) fn apply_view(&self, list: &mut chapbook_paint::DisplayList) {
        let Some(view) = &self.view else { return };
        for op in list.ops.iter_mut().skip(1) {
            match op {
                chapbook_paint::DisplayOp::FillRect { rect, .. } => {
                    rect.origin.x = rect.origin.x * view.zoom + view.pan_x;
                    rect.origin.y = rect.origin.y * view.zoom + view.pan_y;
                    rect.size.w *= view.zoom;
                    rect.size.h *= view.zoom;
                }
                chapbook_paint::DisplayOp::Image { dest, .. } => {
                    dest.origin.x = dest.origin.x * view.zoom + view.pan_x;
                    dest.origin.y = dest.origin.y * view.zoom + view.pan_y;
                    dest.size.w *= view.zoom;
                    dest.size.h *= view.zoom;
                }
                // Image books paint no glyph runs; a page number or other
                // furniture would arrive here if one ever does.
                chapbook_paint::DisplayOp::GlyphRun { origin, .. } => {
                    origin.x = origin.x * view.zoom + view.pan_x;
                    origin.y = origin.y * view.zoom + view.pan_y;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clamp holds the content over the panel: at zoom `z` the page
    /// spans `[pan, pan + size * z]` and the panel wants `[0, size]`.
    #[test]
    fn the_clamp_keeps_the_page_over_the_panel() {
        let size = Size::new(600.0, 800.0);

        // Hard against both corners.
        assert_eq!(clamp_pan(1000.0, 1000.0, 2.0, size), (0.0, 0.0));
        assert_eq!(clamp_pan(-9999.0, -9999.0, 2.0, size), (-600.0, -800.0));

        // And a legal pan is left alone.
        assert_eq!(clamp_pan(-100.0, -200.0, 2.0, size), (-100.0, -200.0));

        // The bound is a function of the *size*, which is why a resize has
        // to re-run it: what was legal at 600 wide is not at 400.
        assert_eq!(clamp_pan(-500.0, 0.0, 2.0, size), (-500.0, 0.0));
        assert_eq!(
            clamp_pan(-500.0, 0.0, 2.0, Size::new(400.0, 500.0)),
            (-400.0, 0.0),
            "the same pan, outside a smaller page, comes back to its edge"
        );
    }

    /// Damage and overlays cross into view space by this map, and it has
    /// to agree with what `apply_view` does to the ops themselves — the
    /// two disagreeing is a region that names pixels the change never
    /// touched.
    #[test]
    fn a_rect_maps_forward_the_way_the_display_list_does() {
        let view = PageView {
            zoom: 2.0,
            pan_x: -50.0,
            pan_y: -30.0,
        };
        let mapped = map_rect(
            Rect {
                origin: Point::new(10.0, 20.0),
                size: Size::new(100.0, 40.0),
            },
            &view,
        );
        assert_eq!(mapped.origin.x, 10.0 * 2.0 - 50.0);
        assert_eq!(mapped.origin.y, 20.0 * 2.0 - 30.0);
        assert_eq!(mapped.size.w, 200.0);
        assert_eq!(mapped.size.h, 80.0);

        // At fit the map is the identity, so an unzoomed page pays nothing
        // and reports exactly what it did before.
        let fit = PageView {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        };
        let rect = Rect {
            origin: Point::new(7.0, 9.0),
            size: Size::new(11.0, 13.0),
        };
        let same = map_rect(rect, &fit);
        assert_eq!((same.origin.x, same.origin.y), (7.0, 9.0));
        assert_eq!((same.size.w, same.size.h), (11.0, 13.0));
    }
}
