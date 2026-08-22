use crate::geometry::{EdgeSizes, Size};

/// Physical page geometry that layout targets, in CSS px.
///
/// The entire layout/paint pipeline works in CSS px; hidpi scaling happens
/// only inside a rasterization backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageMetrics {
    /// Full page size, including margins.
    pub size: Size,
    /// Reader chrome margins around the content box.
    pub margins: EdgeSizes,
    /// Device pixel ratio, passed through to rasterizers; does not affect layout.
    pub dpi_scale: f32,
}

impl PageMetrics {
    pub fn new(size: Size, margins: EdgeSizes, dpi_scale: f32) -> Self {
        PageMetrics {
            size,
            margins,
            dpi_scale,
        }
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
}

impl Default for ReadingSettings {
    fn default() -> Self {
        ReadingSettings {
            base_font_px: 18.0,
            line_height: 1.5,
            justify: false,
            publisher_styles: true,
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
        h.finish()
    }
}
