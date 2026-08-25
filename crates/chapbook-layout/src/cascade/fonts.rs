//! Font metrics for stylo's `ex`/`ch`/`cap`-unit resolution.
//!
//! Returning no metrics makes stylo fall back to its own approximations
//! (`ex` ≈ 0.5em etc.), which is adequate for book CSS where those units
//! are rare. Querying real font tables would mean handing this provider
//! the layout engine's `FontSystem`; that has not been worth the coupling
//! so far, so the approximations stand.

use style::device::servo::FontMetricsProvider;
use style::font_metrics::FontMetrics;
use style::properties::style_structs::Font as FontStyles;
use style::values::computed::font::QueryFontMetricsFlags;
use style::values::computed::CSSPixelLength;

#[derive(Debug, Clone, Default)]
pub struct BookFontMetricsProvider;

impl FontMetricsProvider for BookFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font: &FontStyles,
        _font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics::default()
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> CSSPixelLength {
        // Browser-conventional defaults: 13px monospace, 16px otherwise.
        CSSPixelLength::new(match generic {
            style::values::computed::font::GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        })
    }
}
