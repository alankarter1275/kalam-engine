use crate::geometry::{EdgeSizes, Size};
use crate::Rgba;

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

/// kalam: exact page colours a shell supplies, overriding the preset the
/// [`Theme`] variant would otherwise paint with.
///
/// [`Theme`] stays the light/dark *flavour* — it still decides the
/// `prefers-color-scheme` books see and whether publisher colours are
/// forced (`Dark`) or only defaulted — while the palette supplies the
/// actual pixels. A host with its own theme system (Kalam has four reading
/// themes with hand-picked paper and ink) sets one; every other caller
/// leaves it `None` and nothing changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Palette {
    /// Page ground.
    pub background: Rgba,
    /// Default text colour.
    pub foreground: Rgba,
    /// Default link colour.
    pub link: Rgba,
    /// Transient selection fill (semi-transparent, painted under text).
    pub selection: Rgba,
    /// Stored-highlight fill when a highlight names no colour of its own.
    pub highlight: Rgba,
}

impl Palette {
    /// The colours `theme` paints with by default, as a palette a caller
    /// can then adjust field by field.
    pub fn of(theme: Theme) -> Palette {
        Palette {
            background: theme.background(),
            foreground: theme.foreground(),
            link: theme.link(),
            selection: theme.selection(),
            highlight: theme.highlight(),
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

    /// Map a point from page space out to panel coordinates — the inverse
    /// of [`PageMetrics::panel_to_page`], for handing page-space geometry
    /// (text-run rects, highlight rects) to a shell that draws in panel
    /// space. Identity when unrotated.
    pub fn page_to_panel(&self, x: f32, y: f32) -> (f32, f32) {
        let (w, h) = (self.size.w, self.size.h);
        match self.rotation {
            Rotation::None => (x, y),
            Rotation::Quarter => (h - y, x),
            Rotation::Half => (w - x, h - y),
            Rotation::ThreeQuarter => (y, w - x),
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
    /// The reader's chosen typeface, or `None` for the publisher's.
    ///
    /// A family name, matched against the session's own font database —
    /// `Session::font_families` is the list a picker offers, and a name
    /// nothing answers to leaves the page in whatever the cascade resolves
    /// next, exactly as an unknown family in a publisher's stylesheet
    /// would.
    ///
    /// Unlike [`Self::base_font_px`] and [`Self::line_height`], which are
    /// UA-origin and so lose to a publisher that specifies, this one wins:
    /// nearly every real EPUB sets `body { font-family }`, and a font
    /// choice that silently did nothing on nearly every book would not be
    /// a font choice. Monospace is left alone — a code listing in the
    /// reader's serif is a bug people report. `publisher_styles: false`
    /// remains the blunter instrument.
    pub font_family: Option<String>,
    /// Color theme: page ground, default text/link colors, and the
    /// `prefers-color-scheme` the cascade sees.
    pub theme: Theme,
    /// kalam: exact colours to paint with instead of `theme`'s presets.
    /// `None` — the default, and what every upstream caller has — paints
    /// the preset. See [`Palette`].
    pub palette: Option<Palette>,
    /// kalam: the host's own stylesheet — its reading skin — appended at
    /// user origin after the theme and typeface sheets. `None` (the
    /// default, and what every upstream caller has) adds nothing.
    ///
    /// Per the cascade, its plain declarations beat the UA defaults and
    /// lose to the publisher's; its `!important` ones beat everything,
    /// the publisher's own `!important` included. That is the instrument
    /// a shell needs to say "the book's CSS stands, except these few
    /// things are mine" without a setting per thing.
    pub user_css: Option<String>,
}

impl Default for ReadingSettings {
    fn default() -> Self {
        ReadingSettings {
            base_font_px: 18.0,
            line_height: 1.5,
            justify: false,
            publisher_styles: true,
            font_family: None,
            theme: Theme::default(),
            palette: None,
            user_css: None,
        }
    }
}

impl ReadingSettings {
    /// Stable hash of everything that affects layout, for keying layout caches.
    /// kalam: the colours this configuration paints with — the explicit
    /// palette if the shell set one, else the theme's presets.
    pub fn palette(&self) -> Palette {
        self.palette.unwrap_or_else(|| Palette::of(self.theme))
    }

    pub fn cache_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.base_font_px.to_bits().hash(&mut h);
        self.line_height.to_bits().hash(&mut h);
        self.justify.hash(&mut h);
        self.publisher_styles.hash(&mut h);
        self.font_family.hash(&mut h);
        self.theme.hash(&mut h);
        self.palette.hash(&mut h);
        self.user_css.hash(&mut h);
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_to_panel_inverts_panel_to_page() {
        let base = PageMetrics::new(Size::new(600.0, 800.0), EdgeSizes::uniform(40.0), 1.0);
        for rotation in [
            Rotation::None,
            Rotation::Quarter,
            Rotation::Half,
            Rotation::ThreeQuarter,
        ] {
            let metrics = base.with_rotation(rotation);
            for (x, y) in [(0.0, 0.0), (600.0, 800.0), (123.5, 456.25)] {
                let (px, py) = metrics.page_to_panel(x, y);
                let (rx, ry) = metrics.panel_to_page(px, py);
                assert_eq!((rx, ry), (x, y), "round trip under {rotation:?}");
            }
        }
    }
}
