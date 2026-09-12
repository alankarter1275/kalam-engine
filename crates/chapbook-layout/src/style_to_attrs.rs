//! The `ComputedValues` → cosmic-text mapping, in one place.
//!
//! Supported subset (everything else falls back, documented here):
//! family (the first in the list the font database knows, else the
//! list's generic — [`family_for`]; glyph-level fallback beyond that is
//! cosmic-text's job), weight, italic (oblique ≈ italic), size, color,
//! per-block line height, text-align. Unsupported → fallback: `font-stretch` → normal;
//! `letter-spacing`/`word-spacing` → none (M5); `font-variant`/small-caps →
//! plain; `text-indent` → not applied (needs first-line indent support in
//! the line breaker; tracked M5 gap); `text-transform` → none.

use cosmic_text::{
    Align, Attrs, Color as CosmicColor, Family, Style as CosmicStyle, TextDecoration,
    UnderlineStyle, Weight,
};
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

/// The family cosmic-text is asked for, from the computed `font-family`
/// list.
///
/// kalam: the list is walked, as CSS says: the first named family the
/// database knows wins, a generic always matches, and a list nothing in
/// it matches falls to `serif`, the reading default. Upstream took the
/// first name and left the rest to cosmic-text, whose fallback is by
/// *glyph*, not by list: for `Helvetica, Verdana, sans-serif` on a machine
/// without Helvetica it produced whichever face its platform list or its
/// database order put first (`Noto Sans` heads the Linux list), and for
/// `Georgia, serif` the same — a publisher's serif body came out sans
/// wherever the named face was absent, which is every machine but the
/// publisher's.
///
/// `known` answers whether a family name has a face in the database; the
/// caller decides how to cache that. Matching is exact, as cosmic-text's
/// own is and as `register_font`'s aliasing assumes.
pub fn family_for(style: &ComputedValues, mut known: impl FnMut(&str) -> bool) -> Family<'_> {
    for family in style.get_font().font_family.families.iter() {
        match family {
            SingleFontFamily::FamilyName(name) => {
                if known(&name.name) {
                    return Family::Name(&name.name);
                }
            }
            SingleFontFamily::Generic(generic) => {
                return match generic {
                    GenericFontFamily::SansSerif => Family::SansSerif,
                    GenericFontFamily::Monospace => Family::Monospace,
                    GenericFontFamily::Cursive => Family::Cursive,
                    GenericFontFamily::Fantasy => Family::Fantasy,
                    _ => Family::Serif,
                };
            }
        }
    }
    Family::Serif
}

/// cosmic-text `Attrs` for a styled inline run. `metadata` is the span index
/// the layout uses to map glyphs back to their source run; `family` is the
/// run's resolved typeface, from [`family_for`].
pub fn attrs_for<'s>(style: &'s ComputedValues, metadata: usize, family: Family<'s>) -> Attrs<'s> {
    let font = style.get_font();

    let weight = Weight(font.font_weight.value().round() as u16);
    let style_flag = if font.font_style == style::values::computed::font::FontStyle::NORMAL {
        CosmicStyle::Normal
    } else {
        CosmicStyle::Italic
    };
    let color = text_color(style);

    let mut attrs = Attrs::new()
        .family(family)
        .weight(weight)
        .style(style_flag)
        .color(CosmicColor::rgba(color.r, color.g, color.b, color.a))
        .metadata(metadata);

    // text-decoration-line → cosmic-text decorations (underline offsets and
    // thickness come from the font at shaping time).
    use style::values::specified::text::TextDecorationLine;
    let deco = style.get_text().text_decoration_line;
    let mut td = TextDecoration::new();
    if deco.contains(TextDecorationLine::UNDERLINE) {
        td.underline = UnderlineStyle::Single;
    }
    if deco.contains(TextDecorationLine::LINE_THROUGH) {
        td.strikethrough = true;
    }
    attrs.text_decoration = td;

    // letter-spacing: computed px → cosmic-text EM-unit tracking.
    let tracking_px = letter_spacing_px(style);
    if tracking_px != 0.0 {
        let size = font_size_px(style);
        attrs.letter_spacing_opt = Some(cosmic_text::LetterSpacing(tracking_px / size));
    }
    attrs
}

fn letter_spacing_px(style: &ComputedValues) -> f32 {
    // Computed letter-spacing is a LengthPercentage; percentages are
    // font-size-relative and resolved against it.
    let font_size = app_units::Au::from_f64_px(font_size_px(style) as f64);
    style
        .get_inherited_text()
        .letter_spacing
        .0
        .to_used_value(font_size)
        .to_f64_px() as f32
}

/// Buffer alignment from computed text-align, resolved against the inline
/// base direction.
///
/// `start` and `end` are *logical*, so they depend on `direction` — and
/// `start` is the initial value, which means getting this wrong left every
/// Hebrew and Arabic paragraph in the corpus flush against the wrong
/// margin. It did: `Start` used to map unconditionally to `Align::Left`.
///
/// One narrower thing this still does not fix. cosmic-text resolves the
/// bidi *base level* per line from the first strong character, and 0.19
/// offers no way to state it, so a paragraph declared `dir="rtl"` whose
/// first word is Latin reorders as if it were LTR while aligning right.
/// Declared direction wins the alignment; the reordering is still a guess.
/// `bidi_base_level_comes_from_the_text_not_the_declaration` in
/// `tests/bidi.rs` pins that as known behaviour rather than a surprise.
pub fn align_for(style: &ComputedValues) -> Option<Align> {
    use style::properties::longhands::direction::computed_value::T as Direction;
    use style::values::computed::text::TextAlign;

    let rtl = style.clone_direction() == Direction::Rtl;
    match style.clone_text_align() {
        TextAlign::Left | TextAlign::MozLeft => Some(Align::Left),
        TextAlign::Right | TextAlign::MozRight => Some(Align::Right),
        TextAlign::Start if rtl => Some(Align::Right),
        TextAlign::Start => Some(Align::Left),
        TextAlign::End if rtl => Some(Align::Left),
        TextAlign::End => Some(Align::Right),
        TextAlign::Center | TextAlign::MozCenter => Some(Align::Center),
        TextAlign::Justify => Some(Align::Justified),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::StyleEngine;
    use crate::dom::parse_xhtml;
    use chapbook_core::{PageMetrics, ReadingSettings};

    type ServoArcComputed = style::servo_arc::Arc<ComputedValues>;

    fn styled_p(css: &str) -> ServoArcComputed {
        let html = format!(r#"<html><body><p id="p" style="{css}">x</p></body></html>"#);
        let mut doc = parse_xhtml(html.as_bytes(), "t.xhtml").unwrap();
        let mut engine = StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
        engine.set_author_sheets(&[]);
        engine.style_document(&mut doc);
        let p = doc.element_by_id("p").expect("the paragraph");
        doc.primary_styles(p).expect("styled")
    }

    #[test]
    fn the_first_known_named_family_wins() {
        let style = styled_p("font-family: Helvetica, 'Noto Sans', sans-serif");
        assert_eq!(
            family_for(&style, |name| name == "Noto Sans"),
            Family::Name("Noto Sans")
        );
    }

    #[test]
    fn an_unknown_name_falls_to_the_lists_generic_not_to_cosmic_texts_guess() {
        let style = styled_p("font-family: Helvetica, Verdana, sans-serif");
        assert_eq!(family_for(&style, |_| false), Family::SansSerif);
        let style = styled_p("font-family: Georgia, 'Times New Roman', serif");
        assert_eq!(family_for(&style, |_| false), Family::Serif);
        let style = styled_p("font-family: Consolas, monospace");
        assert_eq!(family_for(&style, |_| false), Family::Monospace);
    }

    #[test]
    fn a_list_nothing_matches_is_the_reading_default() {
        let style = styled_p("font-family: Helvetica, Verdana");
        assert_eq!(family_for(&style, |_| false), Family::Serif);
    }

    #[test]
    fn a_generic_ahead_of_a_known_name_still_wins_because_order_is_the_list() {
        let style = styled_p("font-family: serif, 'Noto Sans'");
        assert_eq!(family_for(&style, |_| true), Family::Serif);
    }
}
