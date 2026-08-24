use crate::geometry::{EdgeSizes, Size};
use crate::Rgba;

/// What the target panel can actually show. E-ink panels are 16-level grey
/// or 1-bit, and converting for them belongs in the render pipeline — a
/// shell that does it itself gets a different answer per shell.
///
/// The conversion produces grey *in RGBA*. Packing those levels into a
/// device's own buffer layout belongs to whoever addresses the hardware —
/// [`crate::Panel::blit`], which is the only party that knows the panel's
/// word order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PixelFormat {
    /// Full color, as laid out. The default.
    #[default]
    Rgba,
    /// Luminance quantized to `levels` steps (clamped to 2..=16: 1-bit and
    /// 4-bit panels are the real cases). `dither` diffuses the
    /// quantization error into neighboring pixels, which images need and
    /// body text usually does not.
    Grey { levels: u8, dither: bool },
}

/// Quarter-turn rotation between the page as laid out and the panel it is
/// painted into, clockwise. Devices mount panels in a fixed orientation, so
/// reading in landscape on a portrait panel — or on a device held upside
/// down — is a property of the output, not of the layout: the same page,
/// turned on its way to the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    #[default]
    None,
    Quarter,
    Half,
    ThreeQuarter,
}

impl Rotation {
    /// Whether this turn swaps width and height.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::Quarter | Rotation::ThreeQuarter)
    }
}

/// A reading color theme. `Light` is the identity theme: it changes
/// nothing about how a book renders today. The others repaint the page
/// ground and the *default* text/link colors — publisher-specified colors
/// are deliberately left alone (user-origin sheet, normal declarations).
///
/// Dark also flips the media `prefers-color-scheme`, so books shipping
/// their own dark-mode rules get them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Theme {
    #[default]
    Light,
    Sepia,
    Dark,
}

impl Theme {
    /// Page ground, painted as the display list's first op.
    pub fn background(self) -> Rgba {
        match self {
            Theme::Light => Rgba::WHITE,
            Theme::Sepia => Rgba::new(246, 240, 226, 255),
            Theme::Dark => Rgba::new(18, 18, 18, 255),
        }
    }

    /// Default text color where the publisher didn't specify one.
    pub fn foreground(self) -> Rgba {
        match self {
            Theme::Light => Rgba::new(0, 0, 0, 255),
            Theme::Sepia => Rgba::new(91, 70, 54, 255),
            Theme::Dark => Rgba::new(220, 220, 220, 255),
        }
    }

    /// Default link color (overrides the UA `a` color, not author colors).
    pub fn link(self) -> Rgba {
        match self {
            Theme::Light => Rgba::new(0, 0, 238, 255),
            Theme::Sepia => Rgba::new(139, 90, 43, 255),
            Theme::Dark => Rgba::new(138, 180, 248, 255),
        }
    }

    /// Selection-highlight fill (semi-transparent; paints under the text).
    pub fn selection(self) -> Rgba {
        match self {
            Theme::Light => Rgba::new(66, 133, 244, 90),
            Theme::Sepia => Rgba::new(139, 90, 43, 70),
            Theme::Dark => Rgba::new(100, 140, 220, 110),
        }
    }

    /// Stored-highlight fill. Warm where the transient selection is cool,
    /// so a highlight reads as a mark on the page rather than as the thing
    /// the pointer is doing right now.
    pub fn highlight(self) -> Rgba {
        match self {
            Theme::Light => Rgba::new(255, 214, 0, 90),
            Theme::Sepia => Rgba::new(214, 158, 46, 85),
            Theme::Dark => Rgba::new(200, 160, 40, 85),
        }
    }

    /// Whether media queries should see `prefers-color-scheme: dark`.
    pub fn is_dark(self) -> bool {
        matches!(self, Theme::Dark)
    }

    /// Next theme in the viewer's cycle order.
    pub fn cycle(self) -> Theme {
        match self {
            Theme::Light => Theme::Sepia,
            Theme::Sepia => Theme::Dark,
            Theme::Dark => Theme::Light,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Sepia => "sepia",
            Theme::Dark => "dark",
        }
    }

    pub fn from_name(name: &str) -> Option<Theme> {
        match name.to_ascii_lowercase().as_str() {
            "light" => Some(Theme::Light),
            "sepia" => Some(Theme::Sepia),
            "dark" => Some(Theme::Dark),
            _ => None,
        }
    }
}

/// Physical page geometry that layout targets, in CSS px.
///
/// The entire layout/paint pipeline works in CSS px; hidpi scaling happens
/// only inside a rasterization backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageMetrics {
    /// Full page size, including margins — in reading orientation, which
    /// is what layout works in whatever the panel does.
    pub size: Size,
    /// Reader chrome margins around the content box.
    pub margins: EdgeSizes,
    /// Device pixel ratio, passed through to rasterizers; does not affect layout.
    pub dpi_scale: f32,
    /// Turn applied on the way to the panel buffer; does not affect layout.
    pub rotation: Rotation,
}

impl PageMetrics {
    pub fn new(size: Size, margins: EdgeSizes, dpi_scale: f32) -> Self {
        PageMetrics {
            size,
            margins,
            dpi_scale,
            rotation: Rotation::None,
        }
    }

    pub fn with_rotation(self, rotation: Rotation) -> Self {
        PageMetrics { rotation, ..self }
    }

    /// Size of the buffer a rendered page lands in — the page size, axes
    /// swapped on a quarter turn. In CSS px; scale by `dpi_scale` for
    /// device pixels.
    pub fn panel_size(&self) -> Size {
        if self.rotation.swaps_axes() {
            Size::new(self.size.h, self.size.w)
        } else {
            self.size
        }
    }

    /// Map a point from panel coordinates back into page space, so input
    /// arrives in the space the page was laid out in. Identity when
    /// unrotated.
    pub fn panel_to_page(&self, x: f32, y: f32) -> (f32, f32) {
        let (w, h) = (self.size.w, self.size.h);
        match self.rotation {
            Rotation::None => (x, y),
            Rotation::Quarter => (y, h - x),
            Rotation::Half => (w - x, h - y),
            Rotation::ThreeQuarter => (w - y, x),
        }
    }

    /// Whether two metrics describe the same layout, differing at most in
    /// how the result is turned for the panel. A rotation alone doesn't
    /// reflow anything.
    pub fn same_layout(&self, other: &PageMetrics) -> bool {
        self.size == other.size
            && self.margins == other.margins
            && self.dpi_scale == other.dpi_scale
    }

    /// Width available to content after margins.
    pub fn content_width(&self) -> f32 {
        (self.size.w - self.margins.horizontal()).max(0.0)
    }

    /// Height available to content after margins — the fragmentainer height.
    pub fn content_height(&self) -> f32 {
        (self.size.h - self.margins.vertical()).max(0.0)
    }
}

impl Default for PageMetrics {
    fn default() -> Self {
        // A 6" ereader-ish portrait page at 96dpi.
        PageMetrics {
            size: Size::new(600.0, 800.0),
            margins: EdgeSizes::uniform(40.0),
            dpi_scale: 1.0,
            rotation: Rotation::None,
        }
    }
}

/// User reading preferences. Changing any field invalidates layout caches;
/// [`ReadingSettings::cache_key`] captures that.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadingSettings {
    /// Base font size in CSS px (maps to the root em).
    pub base_font_px: f32,
    /// Unitless line-height multiplier applied as the UA default.
    pub line_height: f32,
    /// Justify body text where the publisher didn't specify alignment.
    pub justify: bool,
    /// Honor publisher (author-origin) stylesheets; off = UA + user sheets only.
    pub publisher_styles: bool,
    /// Color theme: page ground, default text/link colors, and the
    /// `prefers-color-scheme` the cascade sees.
    pub theme: Theme,
}

impl Default for ReadingSettings {
    fn default() -> Self {
        ReadingSettings {
            base_font_px: 18.0,
            line_height: 1.5,
            justify: false,
            publisher_styles: true,
            theme: Theme::default(),
        }
    }
}

impl ReadingSettings {
    /// Stable hash of everything that affects layout, for keying layout caches.
    pub fn cache_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.base_font_px.to_bits().hash(&mut h);
        self.line_height.to_bits().hash(&mut h);
        self.justify.hash(&mut h);
        self.publisher_styles.hash(&mut h);
        self.theme.hash(&mut h);
        h.finish()
    }
}
