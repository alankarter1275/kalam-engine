//! The `ComputedValues` → cosmic-text mapping, in one place.
//!
//! Supported subset (everything else falls back, documented here):
//! family (first family in the list; the rest are cosmic-text's fallback
//! job), weight, italic (oblique ≈ italic), size, color, per-block line
//! height, text-align. Unsupported → fallback: `font-stretch` → normal;
//! `letter-spacing`/`word-spacing` → none (M5); `font-variant`/small-caps →
//! plain; `text-indent` → not applied (needs first-line indent support in
//! the line breaker; tracked M5 gap); `text-transform` → none.

use cosmic_text::{Align, Attrs, Color as CosmicColor, Family, Style as CosmicStyle, Weight};
use style::color::AbsoluteColor;
use style::properties::ComputedValues;
use style::values::computed::font::{GenericFontFamily, SingleFontFamily};

use chapbook_core::Rgba;

/// Font size in CSS px, clamped positive (cosmic-text asserts nonzero
/// metrics; `font-size: 0` appears in real-world book CSS).
pub fn font_size_px(style: &ComputedValues) -> f32 {
    style.get_font().font_size.used_size.0.px().max(0.5)
}

/// Used line height in CSS px for a block's inline content, clamped
/// positive (`line-height: 0` appears in the wild — decorative blocks in
/// Standard Ebooks CSS — and cosmic-text asserts against it).
pub fn line_height_px(style: &ComputedValues) -> f32 {
    use style::values::generics::font::LineHeight;
    let font_size = font_size_px(style);
    let lh = match style.get_font().line_height {
        LineHeight::Normal => font_size * 1.2,
        LineHeight::Number(n) => font_size * n.0,
        LineHeight::Length(l) => l.0.px(),
    };
    lh.max(0.5)
}

/// Computed color as non-premultiplied sRGB.
pub fn rgba(color: &AbsoluteColor) -> Rgba {
    let srgb = color.to_color_space(style::color::ColorSpace::Srgb);
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba::new(
        c(srgb.components.0),
        c(srgb.components.1),
        c(srgb.components.2),
        c(srgb.alpha),
    )
}

pub fn text_color(style: &ComputedValues) -> Rgba {
    rgba(&style.get_inherited_text().color)
}

/// cosmic-text `Attrs` for a styled inline run. `metadata` is the span index
/// the layout uses to map glyphs back to their source run.
pub fn attrs_for(style: &ComputedValues, metadata: usize) -> Attrs<'_> {
    let font = style.get_font();

    let family = match font.font_family.families.iter().next() {
        Some(SingleFontFamily::FamilyName(name)) => Family::Name(&name.name),
        Some(SingleFontFamily::Generic(generic)) => match generic {
            GenericFontFamily::SansSerif => Family::SansSerif,
            GenericFontFamily::Monospace => Family::Monospace,
            GenericFontFamily::Cursive => Family::Cursive,
            GenericFontFamily::Fantasy => Family::Fantasy,
            _ => Family::Serif,
        },
        None => Family::Serif,
    };

    let weight = Weight(font.font_weight.value().round() as u16);
    let style_flag = if font.font_style == style::values::computed::font::FontStyle::NORMAL {
        CosmicStyle::Normal
    } else {
        CosmicStyle::Italic
    };
    let color = text_color(style);

    Attrs::new()
        .family(family)
        .weight(weight)
        .style(style_flag)
        .color(CosmicColor::rgba(color.r, color.g, color.b, color.a))
        .metadata(metadata)
}

/// Buffer alignment from computed text-align.
pub fn align_for(style: &ComputedValues) -> Option<Align> {
    use style::values::computed::text::TextAlign;
    match style.clone_text_align() {
        TextAlign::Start | TextAlign::Left | TextAlign::MozLeft => Some(Align::Left),
        TextAlign::End | TextAlign::Right | TextAlign::MozRight => Some(Align::Right),
        TextAlign::Center | TextAlign::MozCenter => Some(Align::Center),
        TextAlign::Justify => Some(Align::Justified),
    }
}
