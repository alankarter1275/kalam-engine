//! Kalam's reading preferences, and how they become engine settings.
//!
//! Every number and colour here is copied from Kalam's own source
//! (`src/pages/reader/*`, `src/theme.rs`): the four reading themes with
//! their paper and ink, the selection tints, the `reader.font_px` /
//! `reader.line_height` / `reader.column_px` preference ranges. Nothing is
//! invented — when Kalam changes a colour, change it here too.

use chapbook_core::{Palette, ReadingSettings, Rgba, Theme};

/// The typeface every book is laid out in. Kalam's page CSS ended its body
/// font stack with `Literata, Georgia, serif`; the widget compiles Literata
/// in (see `fonts.rs`), so the first choice is always available.
pub const BODY_FONT: &str = "Literata";

/// Kalam's four reading themes (`ReadingTheme` in the reader page).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KalamTheme {
    Light,
    /// Kalam's default (`reader.theme` preference).
    #[default]
    Sepia,
    Dark,
    Ink,
}

impl KalamTheme {
    pub const ALL: [KalamTheme; 4] = [
        KalamTheme::Light,
        KalamTheme::Sepia,
        KalamTheme::Dark,
        KalamTheme::Ink,
    ];

    /// The name Kalam stores in its `reader.theme` preference.
    pub fn name(self) -> &'static str {
        match self {
            KalamTheme::Light => "light",
            KalamTheme::Sepia => "sepia",
            KalamTheme::Dark => "dark",
            KalamTheme::Ink => "ink",
        }
    }

    /// Inverse of [`KalamTheme::name`]; `None` for a name Kalam never wrote.
    pub fn from_name(name: &str) -> Option<KalamTheme> {
        match name.trim().to_ascii_lowercase().as_str() {
            "light" => Some(KalamTheme::Light),
            "sepia" => Some(KalamTheme::Sepia),
            "dark" => Some(KalamTheme::Dark),
            "ink" => Some(KalamTheme::Ink),
            _ => None,
        }
    }

    /// The next theme in Kalam's order (Light → Sepia → Dark → Ink → Light).
    pub fn next(self) -> KalamTheme {
        match self {
            KalamTheme::Light => KalamTheme::Sepia,
            KalamTheme::Sepia => KalamTheme::Dark,
            KalamTheme::Dark => KalamTheme::Ink,
            KalamTheme::Ink => KalamTheme::Light,
        }
    }

    /// Night themes invert images and force publisher colours away.
    pub fn is_dark(self) -> bool {
        matches!(self, KalamTheme::Dark | KalamTheme::Ink)
    }

    /// Page background — Kalam's "paper" (`bg` in its theme table).
    pub fn background(self) -> Rgba {
        match self {
            KalamTheme::Light => hex(0xfaf8f5),
            KalamTheme::Sepia => hex(0xf5f0e8),
            KalamTheme::Dark => hex(0x1b1e24),
            KalamTheme::Ink => hex(0x0d0d0d),
        }
    }

    /// Text colour — Kalam's "ink".
    pub fn foreground(self) -> Rgba {
        match self {
            KalamTheme::Light => hex(0x1c1917),
            KalamTheme::Sepia => hex(0x2c2820),
            KalamTheme::Dark => hex(0xabb2bf),
            KalamTheme::Ink => hex(0xc8c8c8),
        }
    }

    /// Selection tint, translucent so text stays readable through it.
    /// Kalam's `::selection` rgba values, alpha rounded to a byte.
    pub fn selection(self) -> Rgba {
        match self {
            // rgba(211,137,148,.42)
            KalamTheme::Light => Rgba::new(211, 137, 148, 107),
            // rgba(202,126,136,.44)
            KalamTheme::Sepia => Rgba::new(202, 126, 136, 112),
            // rgba(184,93,112,.52)
            KalamTheme::Dark => Rgba::new(184, 93, 112, 133),
            // rgba(204,104,132,.52)
            KalamTheme::Ink => Rgba::new(204, 104, 132, 133),
        }
    }

    /// Selection handle colour — deliberately not the selection's tint:
    /// near black on the light themes, bright on the dark ones, so the
    /// grips read against the paper. Kalam's `selection_style()` second
    /// value.
    pub fn handle(self) -> Rgba {
        match self {
            KalamTheme::Light | KalamTheme::Sepia => hex(0x0b0b0b),
            KalamTheme::Dark | KalamTheme::Ink => hex(0xffd166),
        }
    }

    /// The engine flavour underneath: decides `prefers-color-scheme` and
    /// whether publisher colours are forced (night) or only defaulted.
    fn engine_theme(self) -> Theme {
        if self.is_dark() {
            Theme::Dark
        } else {
            Theme::Sepia
        }
    }

    /// The exact colours, as the engine's palette. Kalam styles links like
    /// body text (no colour, no underline), so `link` is the ink. The
    /// fallback highlight colour is Kalam's yellow marker; stored
    /// highlights carry their own colour anyway.
    fn palette(self) -> Palette {
        Palette {
            background: self.background(),
            foreground: self.foreground(),
            link: self.foreground(),
            selection: self.selection(),
            highlight: HighlightColor::Yellow.rgba(),
        }
    }
}

/// Kalam's highlight marker colours (`HighlightColor` in its annotations
/// code), with the CSS the engine stores per highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HighlightColor {
    #[default]
    Yellow,
    Green,
    Blue,
    Pink,
    Orange,
}

impl HighlightColor {
    /// Kalam's stored name for the colour.
    pub fn name(self) -> &'static str {
        match self {
            HighlightColor::Yellow => "yellow",
            HighlightColor::Green => "green",
            HighlightColor::Blue => "blue",
            HighlightColor::Pink => "pink",
            HighlightColor::Orange => "orange",
        }
    }

    pub fn from_name(name: &str) -> Option<HighlightColor> {
        match name.trim().to_ascii_lowercase().as_str() {
            "yellow" => Some(HighlightColor::Yellow),
            "green" => Some(HighlightColor::Green),
            "blue" => Some(HighlightColor::Blue),
            "pink" => Some(HighlightColor::Pink),
            "orange" => Some(HighlightColor::Orange),
            _ => None,
        }
    }

    /// `#rrggbbaa` as the engine stores a highlight's colour — Kalam's
    /// `.kalam-hl-*` rules: `rgba(244,211,94,.64)` and friends, the alpha
    /// rounded to a byte (0.64 → a3, 0.68 → ad).
    pub fn css(self) -> &'static str {
        match self {
            HighlightColor::Yellow => "#f4d35ea3",
            HighlightColor::Green => "#8acb9ca3",
            HighlightColor::Blue => "#8bb7f2a3",
            HighlightColor::Pink => "#e99bbdad",
            HighlightColor::Orange => "#f2ae72a3",
        }
    }

    /// The same colour as the engine's pixel type.
    pub fn rgba(self) -> Rgba {
        // Every `css()` value is a well-formed `#rrggbbaa`; the fallback
        // is unreachable and exists only so this cannot panic.
        Rgba::from_hex(self.css(), 0xa3).unwrap_or(Rgba::new(0xf4, 0xd3, 0x5e, 0xa3))
    }
}

/// What Kalam remembers about how the reader should look. Mirrors the
/// `reader.*` preferences one-to-one; the ranges are Kalam's own and
/// [`KalamPrefs::clamped`] enforces them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KalamPrefs {
    pub theme: KalamTheme,
    /// `reader.font_px`, 13–24, default 17.
    pub font_px: f32,
    /// `reader.line_height`, 1.3–2.5 in steps of 0.1, default 1.8.
    pub line_height: f32,
    /// `reader.column_px`, 400–860, default 620: the widest a line of text
    /// may run. Wider windows centre the column.
    pub column_px: f32,
}

impl KalamPrefs {
    pub const FONT_PX_RANGE: (f32, f32) = (13.0, 24.0);
    pub const LINE_HEIGHT_RANGE: (f32, f32) = (1.3, 2.5);
    pub const COLUMN_PX_RANGE: (f32, f32) = (400.0, 860.0);

    /// The preferences brought back inside Kalam's ranges.
    pub fn clamped(self) -> KalamPrefs {
        KalamPrefs {
            theme: self.theme,
            font_px: self
                .font_px
                .clamp(Self::FONT_PX_RANGE.0, Self::FONT_PX_RANGE.1),
            line_height: self
                .line_height
                .clamp(Self::LINE_HEIGHT_RANGE.0, Self::LINE_HEIGHT_RANGE.1),
            column_px: self
                .column_px
                .clamp(Self::COLUMN_PX_RANGE.0, Self::COLUMN_PX_RANGE.1),
        }
    }

    /// The engine settings these preferences mean.
    ///
    /// The book's stylesheet stands (`publisher_styles: true`): its
    /// indents, alignment, margins, relative sizes and any typeface it
    /// names for a particular element are the publisher's design, and the
    /// old WebKit reader respected them. Kalam claims exactly what that
    /// reader claimed — the colours, the body typeface, the base size and
    /// the line spacing — and does so with the same instrument: a sheet of
    /// `!important` rules appended after the book's ([`Self::skin_css`]),
    /// plus the engine's own typeface rule for `html, body`.
    pub fn reading_settings(&self) -> ReadingSettings {
        let prefs = self.clamped();
        ReadingSettings {
            base_font_px: prefs.font_px,
            line_height: prefs.line_height,
            // The book decides; a publisher's ragged-right poem stays so.
            justify: false,
            publisher_styles: true,
            font_family: Some(BODY_FONT.to_string()),
            theme: prefs.theme.engine_theme(),
            palette: Some(prefs.theme.palette()),
            user_css: Some(prefs.skin_css()),
        }
    }

    /// Kalam's reading skin: the rules the old WebKit reader injected
    /// after the book's stylesheets (`epub_book.rs`, `reading_css()` on
    /// calibre-alt), reduced to what the engine's cascade needs to hear.
    ///
    /// Everything here is `!important` on purpose. A user-origin rule
    /// without it is a *default* the publisher overrides, and every real
    /// publisher stylesheet sets `line-height` on `body` and `p`; the
    /// reading preference would otherwise show on no book at all.
    ///
    /// * Colours: the ink on everything, and no backgrounds, in every
    ///   theme — the engine's own themes force colours only at night and
    ///   let a publisher's grey subtitle through by day; the old reader
    ///   did not, and "Kalam's colours" means Kalam's.
    /// * Size: forced on the roots only, so the book's `1.57em` heading is
    ///   still 1.57 times the reader's size.
    /// * Line height: forced on the text blocks too, because a publisher's
    ///   `p { line-height: 1.2 }` on a page the reader set to 1.8 is
    ///   exactly the complaint the preference exists to answer. Headings
    ///   keep the old reader's tighter 1.25.
    /// * Page geometry is Kalam's (column width, page margins), so the
    ///   body's own margins and padding go, as they did before.
    /// * Links read as text: no underline (their colour is the ink
    ///   already, via the palette). The book's other decorations stay.
    ///
    /// Images need no rule: the engine fits them to the measure.
    fn skin_css(&self) -> String {
        let prefs = self.clamped();
        let ink = prefs.theme.foreground();
        let ink = format!("#{:02x}{:02x}{:02x}", ink.r, ink.g, ink.b);
        let size = prefs.font_px;
        let lh = prefs.line_height;
        format!(
            "* {{ color: {ink} !important; background-color: transparent !important; }}\n\
             html, body {{ font-size: {size}px !important; line-height: {lh} !important; \
             margin: 0 !important; padding: 0 !important; }}\n\
             p, div, li, dd, dt, td, th, blockquote, section, article, aside, header, footer, \
             main, figcaption {{ line-height: {lh} !important; }}\n\
             h1, h2, h3, h4, h5, h6 {{ font-weight: bold !important; \
             line-height: 1.25 !important; margin-top: 1.4em !important; }}\n\
             a {{ text-decoration: none !important; }}\n"
        )
    }
}

impl Default for KalamPrefs {
    fn default() -> Self {
        KalamPrefs {
            theme: KalamTheme::default(),
            font_px: 17.0,
            line_height: 1.8,
            column_px: 620.0,
        }
    }
}

const fn hex(rgb: u32) -> Rgba {
    Rgba::new(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
        255,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_round_trip_by_name() {
        for theme in KalamTheme::ALL {
            assert_eq!(KalamTheme::from_name(theme.name()), Some(theme));
        }
        assert_eq!(KalamTheme::from_name("solarized"), None);
    }

    #[test]
    fn colours_are_kalams() {
        assert_eq!(
            KalamTheme::Sepia.background(),
            Rgba::new(0xf5, 0xf0, 0xe8, 255)
        );
        assert_eq!(
            KalamTheme::Dark.foreground(),
            Rgba::new(0xab, 0xb2, 0xbf, 255)
        );
        assert_eq!(
            KalamTheme::Ink.background(),
            Rgba::new(0x0d, 0x0d, 0x0d, 255)
        );
    }

    #[test]
    fn settings_carry_the_permanent_overrides() {
        let settings = KalamPrefs::default().reading_settings();
        // The book's stylesheet stands; Kalam's claims ride on top of it.
        assert!(settings.publisher_styles);
        assert!(!settings.justify);
        assert_eq!(settings.font_family.as_deref(), Some(BODY_FONT));
        let skin = settings.user_css.as_deref().expect("the reading skin");
        assert!(skin.contains("font-size: 17px !important"), "{skin}");
        assert!(skin.contains("line-height: 1.8 !important"), "{skin}");
        assert!(skin.contains("font-weight: bold !important"), "{skin}");
        // Sepia ink, in every theme's skin the same way.
        assert!(skin.contains("color: #2c2820 !important"), "{skin}");
        assert!(skin.contains("text-decoration: none !important"), "{skin}");
        assert_eq!(settings.theme, Theme::Sepia);
        assert_eq!(
            settings.palette().background,
            KalamTheme::Sepia.background()
        );
        let night = KalamPrefs {
            theme: KalamTheme::Ink,
            ..KalamPrefs::default()
        }
        .reading_settings();
        assert_eq!(night.theme, Theme::Dark);
        assert_eq!(night.palette().foreground, KalamTheme::Ink.foreground());
    }

    #[test]
    fn the_skin_follows_the_preferences_and_the_cache_key_with_it() {
        let a = KalamPrefs::default();
        let b = KalamPrefs {
            line_height: 1.3,
            ..a
        };
        let (sa, sb) = (a.reading_settings(), b.reading_settings());
        assert!(sa.user_css.as_deref().unwrap().contains("line-height: 1.8"));
        assert!(sb.user_css.as_deref().unwrap().contains("line-height: 1.3"));
        assert_ne!(sa.cache_key(), sb.cache_key());
    }

    #[test]
    fn prefs_are_clamped_to_kalams_ranges() {
        let wild = KalamPrefs {
            theme: KalamTheme::Light,
            font_px: 99.0,
            line_height: 0.1,
            column_px: 10_000.0,
        }
        .clamped();
        assert_eq!(wild.font_px, 24.0);
        assert_eq!(wild.line_height, 1.3);
        assert_eq!(wild.column_px, 860.0);
    }

    #[test]
    fn highlight_colours_parse() {
        for color in [
            HighlightColor::Yellow,
            HighlightColor::Green,
            HighlightColor::Blue,
            HighlightColor::Pink,
            HighlightColor::Orange,
        ] {
            assert!(
                Rgba::from_hex(color.css(), 255).is_some(),
                "{}",
                color.name()
            );
            assert_eq!(HighlightColor::from_name(color.name()), Some(color));
            let alpha = if color == HighlightColor::Pink {
                0xad
            } else {
                0xa3
            };
            assert_eq!(color.rgba().a, alpha);
        }
    }
}
