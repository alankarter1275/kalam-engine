//! The panel update seam: how a rasterized page reaches a screen that is
//! not a swapchain.
//!
//! An electrophoretic panel does not present frames; it is *asked* to
//! change, and how it is asked decides how the change looks. The same
//! pixels can arrive as a 100ms two-level flicker or a one-second
//! black-white flash, and picking wrong is the difference between an
//! ereader that feels responsive and one that does not.
//!
//! That choice has two halves. Only the engine knows *what kind of change
//! this is* — a selection tracking under a finger, a page turn, a full
//! reflow. Only the device knows *what its controller calls that* — a
//! waveform number, a temperature, a dithering flag. [`UpdateClass`] is
//! the vocabulary between them: chapbook names the kind, the panel
//! implementation names the waveform. Nothing device-specific appears in
//! this module, and nothing about books appears below it.
//!
//! [`super::PixelFormat`] and [`Rotation`] are the pixel-side half of the
//! same story; `chapbook_paint::quantize` and `rotate` apply them.

use crate::{ChapbookError, PixelFormat, Rect, Result, Rotation, Size};

/// What a change needs the panel to do in order to show it correctly — the
/// vendor-neutral half of a waveform choice.
///
/// Each variant is the *least disruptive* update that still renders its
/// change faithfully, so the ordering runs from cheapest and ugliest to
/// slowest and cleanest. Escalating is always safe; the picture is right
/// either way, it just costs more and disturbs more of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum UpdateClass {
    /// Nothing needs to reach the panel at all.
    #[default]
    None,
    /// Two levels, as fast as the controller can go, visible artifacts
    /// acceptable. For changes that are still moving — a selection
    /// following a finger — where latency matters more than fidelity and
    /// the result will be redrawn properly when the gesture ends.
    /// Typically `A2`.
    Monochrome,
    /// Fast, few grey levels, no flash. For small settled changes: a
    /// highlight appearing, a status line ticking over. Typically `DU`.
    Fast,
    /// The panel's full grey range, without a flash. The normal case for
    /// new content — a page turn, a chapter, an image that finished
    /// decoding. Typically `GC16_FAST` or a Regal mode.
    Quality,
    /// Full grey range *with* a black/white flash to clear accumulated
    /// ghosting. Slow and visually loud, so it is reserved for moments the
    /// user already expects a beat, or for paying off the ghost debt that
    /// [`RefreshPolicy`] tracks. Typically `GC16` with a full update mode.
    Flash,
}

impl UpdateClass {
    /// Whether this class puts anything on the glass.
    pub fn is_visible(self) -> bool {
        self != UpdateClass::None
    }

    /// Whether the class leaves its region visibly degraded once it
    /// settles, so a driver owes it a full-fidelity pass later.
    ///
    /// Only [`UpdateClass::Monochrome`] qualifies. It drops the region to
    /// two levels, which turns antialiased text into something you would
    /// not want to read once the finger stops moving. `Fast` also loses
    /// grey levels, but stays legible, and cleaning after every settled
    /// highlight would blink the screen more than the ghosting is worth —
    /// [`RefreshPolicy`] catches its slower accumulation instead.
    pub fn is_provisional(self) -> bool {
        self == UpdateClass::Monochrome
    }
}

/// A rectangle in whole panel pixels, already turned into the panel's own
/// orientation.
///
/// Panel controllers take integer regions, so rounding has to happen
/// somewhere; doing it here means every backend rounds the same way.
/// Conversion always rounds *outward* — a region trimmed by half a pixel
/// leaves a stale sliver on screen, and on e-ink a stale sliver stays
/// there until something else disturbs it.
///
/// A rect handed to a panel is a *minimum*, never an allowance. See
/// [`Panel::submit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PanelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PanelRect {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        PanelRect { x, y, w, h }
    }

    /// The whole panel.
    pub fn full(width: u32, height: u32) -> Self {
        PanelRect::new(0, 0, width, height)
    }

    /// Map a page-coordinate rect (CSS px, page orientation) onto the
    /// panel: scale to device pixels, turn by `rotation`, round outward,
    /// clamp to the panel.
    ///
    /// `page` is the page size in CSS px — the same `Size` the display
    /// list carries — so the caller never has to work out the rotated
    /// panel bounds itself.
    pub fn from_page(rect: Rect, page: Size, scale: f32, rotation: Rotation) -> Self {
        let (pw, ph) = (page.w * scale, page.h * scale);
        let (x0, y0) = (rect.min_x() * scale, rect.min_y() * scale);
        let (x1, y1) = (rect.max_x() * scale, rect.max_y() * scale);

        // The forward turn of `chapbook_paint::rotate`, applied to a span
        // rather than a pixel: the two corners swap roles on the axes the
        // rotation reverses.
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

    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn max_x(self) -> u32 {
        self.x + self.w
    }

    pub fn max_y(self) -> u32 {
        self.y + self.h
    }

    /// Whether two regions overlap. A panel must not be written to while
    /// an update covering the same pixels is still in flight, so this is
    /// the test a driver uses to decide whether it has to wait.
    pub fn intersects(self, other: PanelRect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.max_x()
            && other.x < self.max_x()
            && self.y < other.max_y()
            && other.y < self.max_y()
    }

    /// Whether `other` lies entirely inside this region. An empty region
    /// is contained by anything and contains nothing.
    pub fn contains(self, other: PanelRect) -> bool {
        if other.is_empty() {
            return true;
        }
        !self.is_empty()
            && self.x <= other.x
            && self.y <= other.y
            && self.max_x() >= other.max_x()
            && self.max_y() >= other.max_y()
    }

    /// The smallest region covering both. An empty operand contributes
    /// nothing.
    pub fn union(self, other: PanelRect) -> PanelRect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        PanelRect::new(
            x,
            y,
            self.max_x().max(other.max_x()) - x,
            self.max_y().max(other.max_y()) - y,
        )
    }
}

/// What a panel is, as far as anything above it needs to know.
///
/// Deliberately *not* a rotation. [`crate::PageMetrics::rotation`] is
/// already the one place a turn is decided and applied, and a panel that
/// reported its own would be a second field meaning nearly the same thing
/// with nothing to say which wins. The mounting is already accounted for
/// here: `width` and `height` describe the panel as the controller
/// addresses it, so a portrait-mounted panel read in landscape is a
/// `PageMetrics` concern on the way in, not a `PanelInfo` one on the way
/// out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelInfo {
    /// Panel dimensions in device pixels, as the controller addresses them.
    pub width: u32,
    pub height: u32,
    /// What the panel can show, for `chapbook_paint::quantize`.
    pub format: PixelFormat,
}

/// A submitted update, returned by [`Panel::submit`] and consumed by
/// [`Panel::wait`].
///
/// Opaque on purpose: on an mxcfb device this is the controller's update
/// marker, but nothing above the panel should care.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UpdateToken(pub u32);

/// A screen that is asked to change rather than presented to.
///
/// The split between [`blit`](Panel::blit) and [`submit`](Panel::submit)
/// is not incidental. On a real controller these are two operations — a
/// memcpy into mapped memory, then an ioctl — and an update takes long
/// enough (100ms to a second) that folding the wait into the submit would
/// make page turns feel broken. `submit` returns immediately with a
/// token; the caller decides when it has to know the pixels have landed.
///
/// **Implementors and callers both owe one rule:** never `blit` a region
/// while an update covering it is still in flight. The controller is
/// reading that memory, and writing under it tears or leaves the panel
/// showing a mixture of both frames. [`PanelRect::intersects`] is the
/// test; waiting on the outstanding token is the fix.
pub trait Panel {
    /// Geometry and capabilities. Cheap; callers may ask per frame.
    fn info(&self) -> PanelInfo;

    /// Take `rect` from a full panel-sized RGBA buffer, converted to
    /// whatever this device wants and held wherever this device holds it.
    ///
    /// Deliberately not "copy into the panel's memory": that is true of a
    /// mapped framebuffer and of a locked platform surface, but a panel on
    /// the far end of a bus has no memory to copy into. There, `blit`
    /// stages and [`submit`](Panel::submit) transmits. The obligation is
    /// only that the pixels are taken by the time this returns, since the
    /// caller may reuse the buffer.
    ///
    /// `rgba` covers the whole panel with a stride of `info().width * 4`,
    /// already quantized to `info().format` and already turned, so it is
    /// in panel orientation; `rect` says which part of it is new. Passing
    /// the whole buffer rather than pre-cropped pixels lets the caller
    /// keep one page buffer and state damage separately, which is how a
    /// display list already arrives.
    ///
    /// Conversion lives here because only the panel knows its own spelling
    /// — four-bit grey packed two to a byte, RGB565, 1bpp packed,
    /// inverted, whatever.
    fn blit(&mut self, rgba: &[u8], rect: PanelRect) -> Result<()>;

    /// Ask the panel to show *at least* `rect`, using whatever waveform
    /// this device uses for `class`. Returns as soon as the request is
    /// queued.
    ///
    /// A panel may refresh **more** than it was asked to and must never
    /// refresh less. Hardware routinely forces this: controllers impose
    /// alignment on the region, panels driven over a bus refresh
    /// byte-aligned windows or nothing smaller than the whole screen, and
    /// a flash covers everything by definition. Widening only costs time;
    /// narrowing leaves the screen showing something that is no longer
    /// true, which on e-ink persists.
    ///
    /// A panel that widens owns the consequence. The rule against writing
    /// under a live update is enforced above here against the region that
    /// was *requested*, because that is all a caller knows; a panel that
    /// went further is the only party that knows it did, so it must make
    /// its own [`blit`](Panel::blit) safe against the region it actually
    /// took. On a bus-attached panel that falls out for free — it cannot
    /// transmit while the controller is busy anyway.
    fn submit(&mut self, rect: PanelRect, class: UpdateClass) -> Result<UpdateToken>;

    /// Block until a submitted update has finished reaching the glass.
    ///
    /// Waiting on an already-completed or unknown token succeeds — a
    /// caller that lost track should not be punished for asking.
    fn wait(&mut self, token: UpdateToken) -> Result<()>;
}

/// Ghosting debt: when to spend a [`UpdateClass::Flash`] the content did
/// not ask for.
///
/// Every non-flashing update leaves a little charge behind, and after
/// enough of them the page acquires a grey shadow of what used to be
/// there. The cure is a full flash, and the cost is a visible black-white
/// blink, so it has to be rationed rather than avoided.
///
/// This is deliberately separate from the intent-to-class mapping.
/// "What does this change need?" and "is the screen due for a clean?" are
/// different questions, and only the second one has a knob a user might
/// want to turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshPolicy {
    flash_every: u32,
    since_flash: u32,
}

impl RefreshPolicy {
    /// Six updates between flashes, matching what mainstream readers
    /// settle on: often enough that ghosting stays below notice, rare
    /// enough that the blink reads as punctuation.
    pub const DEFAULT_FLASH_EVERY: u32 = 6;

    /// Flash after `flash_every` updates that could have caused ghosting.
    /// Zero disables scheduled flashes entirely.
    pub fn new(flash_every: u32) -> Self {
        RefreshPolicy {
            flash_every,
            since_flash: 0,
        }
    }

    /// Never flash unless the content itself calls for it.
    pub fn never() -> Self {
        RefreshPolicy::new(0)
    }

    /// Decide what actually goes to the panel, escalating to a flash when
    /// the debt has come due.
    ///
    /// Only [`UpdateClass::Quality`] escalates. Fast and monochrome
    /// updates accrue debt but never trigger the flash themselves,
    /// because they are what a gesture in progress produces — flashing
    /// the screen in the middle of a selection drag would be worse than
    /// any amount of ghosting.
    pub fn resolve(&mut self, class: UpdateClass) -> UpdateClass {
        match class {
            UpdateClass::None => UpdateClass::None,
            UpdateClass::Flash => {
                self.since_flash = 0;
                UpdateClass::Flash
            }
            UpdateClass::Monochrome | UpdateClass::Fast => {
                self.since_flash = self.since_flash.saturating_add(1);
                class
            }
            UpdateClass::Quality => {
                self.since_flash = self.since_flash.saturating_add(1);
                if self.flash_every > 0 && self.since_flash >= self.flash_every {
                    self.since_flash = 0;
                    UpdateClass::Flash
                } else {
                    UpdateClass::Quality
                }
            }
        }
    }

    /// Updates since the last flash — for a shell that wants to flash on
    /// its own schedule, such as when the reader goes idle.
    pub fn since_flash(&self) -> u32 {
        self.since_flash
    }

    /// Forget the accumulated debt, as after a flash this policy did not
    /// decide on.
    pub fn reset(&mut self) {
        self.since_flash = 0;
    }
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        RefreshPolicy::new(RefreshPolicy::DEFAULT_FLASH_EVERY)
    }
}

/// A [`Panel`] that keeps a log instead of a screen.
///
/// Refresh behavior is the part of a device port that is both easy to get
/// wrong and impossible to eyeball — nobody can see, by looking at an
/// ereader, that a drag issued one update per pixel of travel. Driving a
/// shell against this makes those questions ordinary assertions that run
/// on a build machine with no panel attached.
#[derive(Debug, Clone)]
pub struct RecordingPanel {
    info: PanelInfo,
    next_token: u32,
    /// Every update submitted, oldest first.
    pub updates: Vec<(PanelRect, UpdateClass)>,
    /// Every region written, oldest first.
    pub blits: Vec<PanelRect>,
    /// Tokens passed to [`Panel::wait`], oldest first.
    pub waits: Vec<UpdateToken>,
}

impl RecordingPanel {
    pub fn new(info: PanelInfo) -> Self {
        RecordingPanel {
            info,
            next_token: 1,
            updates: Vec::new(),
            blits: Vec::new(),
            waits: Vec::new(),
        }
    }

    /// How many updates of each visible class have been submitted.
    pub fn count(&self, class: UpdateClass) -> usize {
        self.updates.iter().filter(|(_, c)| *c == class).count()
    }

    pub fn clear(&mut self) {
        self.updates.clear();
        self.blits.clear();
        self.waits.clear();
    }
}

impl Panel for RecordingPanel {
    fn info(&self) -> PanelInfo {
        self.info
    }

    fn blit(&mut self, rgba: &[u8], rect: PanelRect) -> Result<()> {
        let expected = (self.info.width as usize) * (self.info.height as usize) * 4;
        if rgba.len() != expected {
            return Err(ChapbookError::Panel(format!(
                "buffer is {} bytes, panel needs {expected}",
                rgba.len()
            )));
        }
        self.blits.push(rect);
        Ok(())
    }

    fn submit(&mut self, rect: PanelRect, class: UpdateClass) -> Result<UpdateToken> {
        self.updates.push((rect, class));
        let token = UpdateToken(self.next_token);
        self.next_token += 1;
        Ok(token)
    }

    fn wait(&mut self, token: UpdateToken) -> Result<()> {
        self.waits.push(token);
        Ok(())
    }
}

/// Drives a [`Panel`] correctly: the rules a shell would otherwise have to
/// remember, in one place that can be tested without hardware.
///
/// There are three, and each is the kind of thing that works on a desk and
/// fails on a device.
///
/// **Do not write under a live update.** The controller is reading the
/// staged pixels while it drives the film. Blitting a region an in-flight
/// update covers tears, or leaves the panel showing a mixture of two
/// frames. The driver waits only when the regions actually overlap, so an
/// unrelated corner of the screen never pays for a slow refresh elsewhere.
///
/// This is enforced against the region that was *requested*, which is all
/// a driver can know: [`Panel::submit`] is free to refresh more than it
/// was asked to, and a panel that widens is responsible for its own
/// safety, as that contract says.
///
/// **Clean up after fast updates.** A monochrome update leaves its region
/// at two levels, and e-ink holds that until something disturbs it — so
/// the selected lines stay chunky after the finger lifts. The session
/// cannot see that moment: it has no way to tell a mid-drag selection from
/// the last one. Only the shell knows the pointer came up, so it calls
/// [`settle`](PanelDriver::settle) and the driver repaints what it left
/// provisional.
///
/// **Ration the flash.** Delegated to [`RefreshPolicy`].
#[derive(Debug)]
pub struct PanelDriver<P: Panel> {
    panel: P,
    policy: RefreshPolicy,
    /// The update still reaching the glass, and the region it covers.
    in_flight: Option<(UpdateToken, PanelRect)>,
    /// Region left at reduced fidelity, owed a full-fidelity pass.
    provisional: Option<PanelRect>,
}

impl<P: Panel> PanelDriver<P> {
    /// Drive `panel` with the default ghosting cadence.
    pub fn new(panel: P) -> Self {
        PanelDriver::with_policy(panel, RefreshPolicy::default())
    }

    pub fn with_policy(panel: P, policy: RefreshPolicy) -> Self {
        PanelDriver {
            panel,
            policy,
            in_flight: None,
            provisional: None,
        }
    }

    pub fn info(&self) -> PanelInfo {
        self.panel.info()
    }

    pub fn panel(&self) -> &P {
        &self.panel
    }

    pub fn panel_mut(&mut self) -> &mut P {
        &mut self.panel
    }

    pub fn into_panel(self) -> P {
        self.panel
    }

    /// The region owed a full-fidelity repaint, if any.
    pub fn provisional(&self) -> Option<PanelRect> {
        self.provisional
    }

    /// Put a page on the panel.
    ///
    /// `rgba` covers the whole panel, already quantized and turned;
    /// `damage` is the region that changed, with `None` meaning the whole
    /// panel — the same convention `Frame::damage` uses, so a shell passes
    /// one through to the other. Returns the token of the update it
    /// submitted, or `None` when there was nothing to do.
    pub fn present(
        &mut self,
        rgba: &[u8],
        damage: Option<PanelRect>,
        class: UpdateClass,
    ) -> Result<Option<UpdateToken>> {
        let info = self.panel.info();
        let full = PanelRect::full(info.width, info.height);
        let class = self.policy.resolve(class);
        if class == UpdateClass::None {
            return Ok(None);
        }
        // A flash exists to clear the whole screen's accumulated charge;
        // flashing a corner would spend the visual cost without buying
        // the result.
        let rect = if class == UpdateClass::Flash {
            full
        } else {
            damage.unwrap_or(full)
        };
        if rect.is_empty() {
            return Ok(None);
        }
        self.submit_region(rgba, rect, class).map(Some)
    }

    /// Repaint whatever a fast update left degraded — what a shell calls
    /// when a gesture ends.
    ///
    /// `Ok(None)` when nothing is owed, which is the common case and
    /// deliberately cheap: a shell can call this on every idle tick.
    pub fn settle(&mut self, rgba: &[u8]) -> Result<Option<UpdateToken>> {
        let Some(rect) = self.provisional else {
            return Ok(None);
        };
        // Through the policy like any other update: the cleanup pass is a
        // real change to the panel and may itself come due for a flash.
        let class = self.policy.resolve(UpdateClass::Quality);
        if class == UpdateClass::None {
            return Ok(None);
        }
        let rect = if class == UpdateClass::Flash {
            let info = self.panel.info();
            PanelRect::full(info.width, info.height)
        } else {
            rect
        };
        self.submit_region(rgba, rect, class).map(Some)
    }

    /// Block until the outstanding update has reached the glass. A shell
    /// owes this before it tears the panel down, and nowhere else.
    pub fn flush(&mut self) -> Result<()> {
        if let Some((token, _)) = self.in_flight.take() {
            self.panel.wait(token)?;
        }
        Ok(())
    }

    fn submit_region(
        &mut self,
        rgba: &[u8],
        rect: PanelRect,
        class: UpdateClass,
    ) -> Result<UpdateToken> {
        // Only an overlap forces a wait; a change elsewhere on the page
        // proceeds while the previous refresh is still running.
        if let Some((token, live)) = self.in_flight {
            if live.intersects(rect) {
                self.panel.wait(token)?;
                self.in_flight = None;
            }
        }
        self.panel.blit(rgba, rect)?;
        let token = self.panel.submit(rect, class)?;
        self.in_flight = Some((token, rect));

        self.provisional = if class.is_provisional() {
            // Successive fast updates chase a moving gesture; the debt is
            // everything they have touched.
            Some(match self.provisional {
                Some(owed) => owed.union(rect),
                None => rect,
            })
        } else {
            // A full-fidelity update pays off only what it actually
            // covered.
            self.provisional.filter(|owed| !rect.contains(*owed))
        };
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(width: u32, height: u32) -> PanelInfo {
        PanelInfo {
            width,
            height,
            format: PixelFormat::Grey {
                levels: 16,
                dither: false,
            },
        }
    }

    // ---- PanelRect ----

    #[test]
    fn unrotated_page_rect_scales_to_device_pixels() {
        let r = PanelRect::from_page(
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
        let r = PanelRect::from_page(
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
        // The span mapping has to agree with chapbook_paint::rotate, which
        // sends page (x, y) to panel (h - 1 - y, x). A rect in the page's
        // top-left lands in the panel's top-right.
        let page = Size::new(100.0, 200.0);
        let r = PanelRect::from_page(
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
            let r = PanelRect::from_page(rect, page, 1.5, rotation);
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
            let r = PanelRect::from_page(rect, page, 1.0, rotation);
            r.w * r.h
        };
        for rotation in [Rotation::Quarter, Rotation::Half, Rotation::ThreeQuarter] {
            assert_eq!(area(rotation), area(Rotation::None), "{rotation:?}");
        }
    }

    #[test]
    fn intersects_is_exclusive_at_the_edge() {
        let a = PanelRect::new(0, 0, 10, 10);
        assert!(a.intersects(PanelRect::new(9, 9, 5, 5)));
        // Touching edges share no pixel.
        assert!(!a.intersects(PanelRect::new(10, 0, 5, 5)));
        assert!(!a.intersects(PanelRect::new(0, 10, 5, 5)));
        assert!(!a.intersects(PanelRect::new(0, 0, 0, 0)));
    }

    #[test]
    fn union_ignores_empty_operands() {
        let a = PanelRect::new(5, 5, 10, 10);
        assert_eq!(a.union(PanelRect::default()), a);
        assert_eq!(PanelRect::default().union(a), a);
        assert_eq!(
            a.union(PanelRect::new(20, 0, 5, 5)),
            PanelRect::new(5, 0, 20, 15)
        );
    }

    // ---- RefreshPolicy ----

    #[test]
    fn a_drag_never_flashes() {
        // The whole reason fast classes do not escalate: flashing the
        // screen under a moving finger is worse than any ghosting.
        let mut policy = RefreshPolicy::new(6);
        for _ in 0..200 {
            assert_ne!(
                policy.resolve(UpdateClass::Monochrome),
                UpdateClass::Flash,
                "a selection drag triggered a flash"
            );
        }
        for _ in 0..200 {
            assert_ne!(policy.resolve(UpdateClass::Fast), UpdateClass::Flash);
        }
    }

    #[test]
    fn page_turns_flash_on_cadence() {
        let mut policy = RefreshPolicy::new(3);
        let seen: Vec<_> = (0..7)
            .map(|_| policy.resolve(UpdateClass::Quality))
            .collect();
        use UpdateClass::{Flash, Quality};
        assert_eq!(
            seen,
            vec![Quality, Quality, Flash, Quality, Quality, Flash, Quality]
        );
    }

    #[test]
    fn a_content_flash_pays_off_the_debt() {
        let mut policy = RefreshPolicy::new(3);
        policy.resolve(UpdateClass::Quality);
        policy.resolve(UpdateClass::Quality);
        // A relayout flashes on its own account, which cleans the panel.
        assert_eq!(policy.resolve(UpdateClass::Flash), UpdateClass::Flash);
        assert_eq!(policy.since_flash(), 0);
        assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Quality);
    }

    #[test]
    fn fast_updates_still_accrue_debt() {
        // They do not trigger the flash, but they do dirty the panel, so
        // the next settled update should come due sooner.
        let mut policy = RefreshPolicy::new(3);
        policy.resolve(UpdateClass::Monochrome);
        policy.resolve(UpdateClass::Fast);
        assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Flash);
    }

    #[test]
    fn nothing_is_free() {
        let mut policy = RefreshPolicy::new(2);
        for _ in 0..50 {
            assert_eq!(policy.resolve(UpdateClass::None), UpdateClass::None);
        }
        assert_eq!(policy.since_flash(), 0);
    }

    #[test]
    fn zero_disables_scheduled_flashes() {
        let mut policy = RefreshPolicy::never();
        for _ in 0..500 {
            assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Quality);
        }
    }

    // ---- RecordingPanel ----

    #[test]
    fn recording_panel_logs_what_a_shell_did() {
        let mut panel = RecordingPanel::new(info(4, 4));
        let buffer = vec![0u8; 4 * 4 * 4];
        let rect = PanelRect::new(0, 0, 4, 2);
        panel.blit(&buffer, rect).unwrap();
        let token = panel.submit(rect, UpdateClass::Fast).unwrap();
        panel.wait(token).unwrap();

        assert_eq!(panel.blits, vec![rect]);
        assert_eq!(panel.updates, vec![(rect, UpdateClass::Fast)]);
        assert_eq!(panel.waits, vec![token]);
        assert_eq!(panel.count(UpdateClass::Fast), 1);
        assert_eq!(panel.count(UpdateClass::Flash), 0);
    }

    #[test]
    fn tokens_are_distinct() {
        let mut panel = RecordingPanel::new(info(4, 4));
        let rect = PanelRect::full(4, 4);
        let first = panel.submit(rect, UpdateClass::Quality).unwrap();
        let second = panel.submit(rect, UpdateClass::Quality).unwrap();
        assert_ne!(first, second);
    }

    // ---- PanelDriver ----

    fn driver(flash_every: u32) -> (PanelDriver<RecordingPanel>, Vec<u8>) {
        let info = info(100, 200);
        let buffer = vec![0u8; 100 * 200 * 4];
        (
            PanelDriver::with_policy(RecordingPanel::new(info), RefreshPolicy::new(flash_every)),
            buffer,
        )
    }

    #[test]
    fn a_drag_issues_fast_updates_and_never_flashes() {
        let (mut driver, buffer) = driver(6);
        for y in 0..40 {
            let rect = PanelRect::new(0, y, 100, 20);
            driver
                .present(&buffer, Some(rect), UpdateClass::Monochrome)
                .unwrap();
        }
        let panel = driver.panel();
        assert_eq!(panel.count(UpdateClass::Monochrome), 40);
        assert_eq!(panel.count(UpdateClass::Flash), 0, "flashed under a drag");
    }

    #[test]
    fn settling_after_a_drag_repaints_what_it_degraded() {
        let (mut driver, buffer) = driver(60);
        for y in [0, 20, 40] {
            driver
                .present(
                    &buffer,
                    Some(PanelRect::new(0, y, 100, 20)),
                    UpdateClass::Monochrome,
                )
                .unwrap();
        }
        // The debt is everything the gesture touched, not just its last step.
        assert_eq!(driver.provisional(), Some(PanelRect::new(0, 0, 100, 60)));

        driver.settle(&buffer).unwrap().expect("a cleanup update");
        assert_eq!(driver.provisional(), None);
        let (rect, class) = *driver.panel().updates.last().unwrap();
        assert_eq!(class, UpdateClass::Quality);
        assert_eq!(rect, PanelRect::new(0, 0, 100, 60));
    }

    #[test]
    fn settling_with_nothing_owed_does_nothing() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(&buffer, None, UpdateClass::Quality)
            .unwrap()
            .expect("the page itself");
        let before = driver.panel().updates.len();
        // Cheap enough for a shell to call on every idle tick.
        for _ in 0..10 {
            assert!(driver.settle(&buffer).unwrap().is_none());
        }
        assert_eq!(driver.panel().updates.len(), before);
    }

    #[test]
    fn an_overlapping_update_waits_for_the_one_in_flight() {
        let (mut driver, buffer) = driver(60);
        let first = driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap()
            .unwrap();
        driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 10, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap();
        assert_eq!(
            driver.panel().waits,
            vec![first],
            "writing under a live update tears the panel"
        );
    }

    #[test]
    fn an_unrelated_region_does_not_wait() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap();
        driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 100, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap();
        assert!(
            driver.panel().waits.is_empty(),
            "a far corner paid for a refresh it did not overlap"
        );
    }

    #[test]
    fn a_flash_covers_the_whole_panel() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 0, 10, 10)),
                UpdateClass::Flash,
            )
            .unwrap();
        let (rect, class) = driver.panel().updates[0];
        assert_eq!(class, UpdateClass::Flash);
        assert_eq!(
            rect,
            PanelRect::full(100, 200),
            "a flash of one corner spends the blink without clearing the screen"
        );
    }

    #[test]
    fn nothing_to_show_touches_nothing() {
        let (mut driver, buffer) = driver(60);
        // An annotation on another page states a region that is not here.
        assert!(driver
            .present(&buffer, Some(PanelRect::default()), UpdateClass::Fast)
            .unwrap()
            .is_none());
        assert!(driver
            .present(&buffer, None, UpdateClass::None)
            .unwrap()
            .is_none());
        assert!(driver.panel().updates.is_empty());
        assert!(driver.panel().blits.is_empty());
    }

    #[test]
    fn unstated_damage_means_the_whole_panel() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(&buffer, None, UpdateClass::Quality)
            .unwrap()
            .unwrap();
        assert_eq!(driver.panel().updates[0].0, PanelRect::full(100, 200));
    }

    #[test]
    fn a_covering_repaint_pays_the_debt_without_settling() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Monochrome,
            )
            .unwrap();
        assert!(driver.provisional().is_some());
        // A page turn repaints the lines anyway; the cleanup is already done.
        driver.present(&buffer, None, UpdateClass::Quality).unwrap();
        assert_eq!(driver.provisional(), None);
        assert!(driver.settle(&buffer).unwrap().is_none());
    }

    #[test]
    fn a_partial_repaint_leaves_the_rest_owed() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 0, 100, 60)),
                UpdateClass::Monochrome,
            )
            .unwrap();
        driver
            .present(
                &buffer,
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Quality,
            )
            .unwrap();
        assert_eq!(
            driver.provisional(),
            Some(PanelRect::new(0, 0, 100, 60)),
            "a repaint that covered part of the region cleared all of it"
        );
    }

    #[test]
    fn the_flash_cadence_survives_the_driver() {
        let (mut driver, buffer) = driver(3);
        for _ in 0..6 {
            driver.present(&buffer, None, UpdateClass::Quality).unwrap();
        }
        assert_eq!(driver.panel().count(UpdateClass::Flash), 2);
        assert_eq!(driver.panel().count(UpdateClass::Quality), 4);
    }

    #[test]
    fn flush_waits_for_the_outstanding_update() {
        let (mut driver, buffer) = driver(60);
        let token = driver
            .present(&buffer, None, UpdateClass::Quality)
            .unwrap()
            .unwrap();
        driver.flush().unwrap();
        assert_eq!(driver.panel().waits, vec![token]);
        // Nothing outstanding: a second flush is free.
        driver.flush().unwrap();
        assert_eq!(driver.panel().waits.len(), 1);
    }

    #[test]
    fn a_wrongly_sized_buffer_is_rejected() {
        // Catches the shell that hands over a page-sized buffer when the
        // panel wanted a rotated one — silent garbage on real hardware.
        let mut panel = RecordingPanel::new(info(4, 8));
        let buffer = vec![0u8; 4 * 4 * 4];
        let err = panel.blit(&buffer, PanelRect::full(4, 8)).unwrap_err();
        assert!(matches!(err, ChapbookError::Panel(_)), "{err:?}");
    }
}
