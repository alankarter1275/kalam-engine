//! The typefaces compiled into the widget.
//!
//! Kalam ships no font files and its reader page named `Literata, Georgia,
//! serif`. Depending on the host having Literata installed is how a book
//! ends up in whatever fontconfig picks; compiling the faces in is how it
//! ends up in Literata on every machine, with no font directory to find
//! at run time. Eight faces, about 3 MB of binary:
//!
//! * **Literata** regular / italic / bold / bold italic — body text.
//! * **Noto Sans** regular / italic / bold / bold italic — what a book's
//!   `font-family: sans-serif` resolves to (tables, captions), and the
//!   widest Latin/Greek/Cyrillic coverage of any open sans.
//!
//! Both are SIL Open Font License 1.1; the licence texts sit beside the
//! files in `fonts/` and travel with any binary (see `NOTICE`).
//!
//! The host's own fonts are *not* scanned unless asked
//! ([`ReaderOptions::host_fonts`](crate::ReaderOptions::host_fonts)).
//! Scanning means touching every font file on the disk — a thousand of
//! them on a desktop with Nerd Fonts installed — and on a cold hard disk
//! that alone is seconds before the first page. The price of skipping it
//! is that characters neither face covers (CJK, Devanagari, Arabic,
//! emoji) draw as empty boxes until the host fonts are switched on.

use std::sync::Arc;

use chapbook_core::{Faces, Fallbacks, FontSource, GenericFamilies, Generics};

/// Family name of the bundled sans face.
pub const SANS_FONT: &str = "Noto Sans";

macro_rules! face {
    ($file:literal) => {
        Faces::Bytes(Arc::from(&include_bytes!(concat!("../fonts/", $file))[..]))
    };
}

/// The font source a session opens with. `host_fonts` adds the system's
/// fonts after the bundled ones, for script fallback only — the bundled
/// faces still win for every family name they cover.
pub fn font_source(host_fonts: bool) -> FontSource {
    let mut faces = vec![
        face!("Literata-Regular.ttf"),
        face!("Literata-Italic.ttf"),
        face!("Literata-Bold.ttf"),
        face!("Literata-BoldItalic.ttf"),
        face!("NotoSans-Regular.ttf"),
        face!("NotoSans-Italic.ttf"),
        face!("NotoSans-Bold.ttf"),
        face!("NotoSans-BoldItalic.ttf"),
    ];
    if host_fonts {
        faces.push(Faces::Host);
    }
    FontSource {
        faces,
        // Say what the five CSS generics mean rather than let fontconfig
        // answer: `serif` is Literata, everything else Noto Sans. With the
        // host's fonts loaded, `monospace` is left to them (a code listing
        // in a proportional face comes apart); without them it is Noto
        // Sans too, the only sans there is.
        generics: Generics::Explicit(GenericFamilies {
            serif: crate::prefs::BODY_FONT.to_string(),
            sans_serif: SANS_FONT.to_string(),
            monospace: if host_fonts { "monospace" } else { SANS_FONT }.to_string(),
            cursive: SANS_FONT.to_string(),
            fantasy: SANS_FONT.to_string(),
        }),
        // The platform's fallback list is names; a name that was never
        // loaded is skipped, so with the host off this quietly reduces to
        // "whichever bundled face has the glyph".
        fallback: Fallbacks::Platform,
        locale: None,
    }
}
