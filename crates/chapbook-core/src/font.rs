//! Where fonts come from, said out loud.
//!
//! A [`FontSource`] is a *description*, not a font system: plain paths,
//! bytes and family names, with no cosmic-text or fontdb types in it. That
//! keeps this crate dependency-light, lets the description cross a C ABI
//! (Android hands over a directory path, iOS hands over bytes), and puts the
//! realization where the text engine already lives —
//! `chapbook_layout::build_font_system`.
//!
//! # Why this is a type and not a `FontSystem::new()` call
//!
//! `FontSystem::new()` decides three independent things, and a build can
//! succeed at any one of them while failing the others:
//!
//! 1. **Which faces exist.** fontdb scans per-target directories. It has no
//!    Android branch and no iOS branch, so on both the database comes up
//!    *empty* and every page lays out with no faces at all.
//! 2. **What the five CSS generics mean.** `serif`, `sans-serif`,
//!    `monospace`, `cursive`, `fantasy` are strings, and nothing anywhere
//!    checks that the family named is installed. On a Linux dev box today,
//!    `cursive` resolves through fontconfig to a Persian nastaliq family
//!    that is not present in the database.
//! 3. **What is tried when a glyph is missing.** cosmic-text's script
//!    fallback is chosen by `cfg`, and the list is *empty* on Android and
//!    wasm and names Linux families on iOS. An Android device holds
//!    complete Noto coverage in `/system/fonts` that nothing will ever
//!    reach for.
//!
//! None of those three fails loudly. A session with no fonts lays out,
//! renders, paints, and conforms — it just paginates every book to a single
//! blank page, which breaks navigation, search and the table of contents
//! rather than merely looking wrong. That is why the source is a required
//! argument rather than a setting with a default.
//!
//! The full evidence is in `docs/FFI.md`, *Fonts, on every platform*.

use std::path::PathBuf;
use std::sync::Arc;

/// A complete description of the fonts a session may use.
///
/// Build one with [`FontSource::host`] (desktop), [`FontSource::embedded`]
/// (tests, goldens, and any build that must lay out identically everywhere),
/// or field-by-field for a platform that needs its own answer.
#[derive(Debug, Clone)]
pub struct FontSource {
    /// Where faces come from, in order. Later entries add to earlier ones.
    pub faces: Vec<Faces>,
    /// What the five CSS generic families resolve to.
    pub generics: Generics,
    /// What to try when the selected face has no glyph for a character.
    pub fallback: Fallbacks,
    /// BCP-47 tag. Disambiguates Han between Chinese, Japanese and Korean
    /// during fallback and affects nothing else. `None` asks the host.
    ///
    /// The one axis here that may be left implicit, because unlike the
    /// other three, guessing wrong is a wrong *glyph shape* rather than no
    /// glyph at all.
    pub locale: Option<String>,
}

/// One source of font faces.
#[derive(Debug, Clone)]
pub enum Faces {
    /// Every font file under a directory, recursively. Android's
    /// `/system/fonts`, iOS's `/System/Library/Fonts`, a device rootfs's
    /// bundled set.
    Dir(PathBuf),
    /// One face's bytes, already in memory: compiled into the binary with
    /// `include_bytes!`, read out of an app bundle, or handed over by
    /// CoreText.
    Bytes(Arc<[u8]>),
    /// Whatever fontdb finds by scanning this target's usual locations.
    ///
    /// Named rather than implied so that host-dependent layout is a thing a
    /// caller chose and a reviewer can grep for. Empty on Android, iOS and
    /// wasm, where realizing the source is an error rather than an empty
    /// database.
    Host,
}

/// What the five CSS generic families resolve to.
///
/// No `Default`. Every platform's built-in answer is wrong somewhere, and
/// the failure is silent, so the choice is made at the call site or not at
/// all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Generics {
    /// Ask the platform's font configuration — fontconfig on Linux, fontdb's
    /// per-target defaults elsewhere.
    ///
    /// Honest but rarely right: fontdb's built-ins are Microsoft family
    /// names, and fontconfig's alias chain can name families that are not
    /// installed. Realizing the source reports any that resolve to nothing
    /// (see [`FontReport`]) rather than letting them fail as missing
    /// glyphs.
    Host,
    /// Spell all five out. The only option that means the same thing on
    /// every machine.
    Explicit(GenericFamilies),
}

/// The five CSS generic family names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericFamilies {
    pub serif: String,
    pub sans_serif: String,
    pub monospace: String,
    pub cursive: String,
    pub fantasy: String,
}

impl GenericFamilies {
    /// Point all five at one family. What a fixture corpus wants: layout
    /// then depends on exactly the faces vendored beside it.
    pub fn all(family: impl Into<String>) -> Self {
        let family = family.into();
        GenericFamilies {
            serif: family.clone(),
            sans_serif: family.clone(),
            monospace: family.clone(),
            cursive: family.clone(),
            fantasy: family,
        }
    }
}

/// What to try when the selected face has no glyph for a character.
///
/// No `Default`, for the same reason as [`Generics`]: the per-target answer
/// is empty on Android and wasm, and names Linux families on iOS, and in
/// both cases the symptom is tofu rather than an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fallbacks {
    /// cosmic-text's compiled-in per-target list. Correct on Linux, macOS
    /// and Windows; **empty** on Android and wasm; **wrong** on iOS, which
    /// takes the Linux list and chases families it does not have.
    Platform,
    /// Nothing. A missing glyph renders as tofu, deterministically.
    ///
    /// What a fixture corpus wants, so a golden means the same thing on
    /// every host — pinning the faces alone does not, because the platform
    /// list still varies underneath.
    None,
    /// An explicit list.
    Explicit(FallbackFamilies),
}

/// Family names to fall back through, most specific first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FallbackFamilies {
    /// Tried for any missing glyph, after any script-specific list.
    pub common: Vec<String>,
    /// Tried first, for text in a particular script.
    pub per_script: Vec<(ScriptTag, Vec<String>)>,
    /// Never fall back to these, however well they match.
    pub forbidden: Vec<String>,
}

/// An ISO 15924 script code — `Latn`, `Grek`, `Cyrl`, `Hani`, `Arab`.
///
/// Four bytes rather than an enum of two hundred variants, because this
/// crosses a C ABI and because the text engine's own script type is a
/// dependency this crate does not want.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScriptTag([u8; 4]);

impl ScriptTag {
    /// From a four-ASCII-character code. Non-ASCII or wrong-length input is
    /// rejected rather than silently truncated.
    pub fn new(tag: &str) -> Option<ScriptTag> {
        let bytes = tag.as_bytes();
        if bytes.len() != 4 || !tag.is_ascii() {
            return None;
        }
        Some(ScriptTag([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn as_str(&self) -> &str {
        // Constructed only from validated ASCII.
        std::str::from_utf8(&self.0).unwrap_or("Zzzz")
    }
}

impl std::fmt::Display for ScriptTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FontSource {
    /// The host's installed fonts, its idea of the generics, and its
    /// fallback list.
    ///
    /// Layout then depends on the machine, which is fine for a desktop shell
    /// and wrong for a test — see [`FontSource::embedded`]. Produces no
    /// faces at all on Android, iOS and wasm, where it fails at
    /// construction rather than rendering blank pages.
    pub fn host() -> FontSource {
        FontSource {
            faces: vec![Faces::Host],
            generics: Generics::Host,
            fallback: Fallbacks::Platform,
            locale: None,
        }
    }

    /// Exactly the fonts in `dir`, every generic pointed at `family`, no
    /// fallback, and a fixed locale.
    ///
    /// All three axes pinned, which is what makes a golden mean the same
    /// thing on Linux, on a Mac and on a device.
    pub fn embedded(dir: impl Into<PathBuf>, family: impl Into<String>) -> FontSource {
        FontSource {
            faces: vec![Faces::Dir(dir.into())],
            generics: Generics::Explicit(GenericFamilies::all(family)),
            fallback: Fallbacks::None,
            locale: Some("en-US".to_string()),
        }
    }

    /// Android's system fonts: `/system/fonts`, the families actually
    /// shipped there, and Noto for fallback.
    ///
    /// The one preset that is a platform's answer rather than a policy,
    /// because Android is the only target where every axis fails and none
    /// fails by accident. Verified against an emulator image: 214 faces,
    /// `Noto Serif` and `Roboto` present, `Comic Sans MS` and friends
    /// nowhere.
    pub fn android_system() -> FontSource {
        FontSource {
            faces: vec![Faces::Dir(PathBuf::from("/system/fonts"))],
            generics: Generics::Explicit(GenericFamilies {
                serif: "Noto Serif".into(),
                sans_serif: "Roboto".into(),
                monospace: "Droid Sans Mono".into(),
                cursive: "Dancing Script".into(),
                fantasy: "Roboto".into(),
            }),
            fallback: Fallbacks::Explicit(FallbackFamilies {
                common: vec![
                    "Noto Sans".into(),
                    "Noto Sans Symbols".into(),
                    "Noto Color Emoji".into(),
                ],
                per_script: android_script_fallback(),
                forbidden: Vec::new(),
            }),
            locale: None,
        }
    }
}

/// The Noto families Android ships per script. Not exhaustive — the common
/// scripts a book is likely to carry, plus emoji.
fn android_script_fallback() -> Vec<(ScriptTag, Vec<String>)> {
    const LISTS: &[(&str, &[&str])] = &[
        ("Hani", &["Noto Sans CJK SC", "Noto Serif CJK SC"]),
        ("Hira", &["Noto Sans CJK JP", "Noto Serif CJK JP"]),
        ("Kana", &["Noto Sans CJK JP", "Noto Serif CJK JP"]),
        ("Hang", &["Noto Sans CJK KR", "Noto Serif CJK KR"]),
        ("Arab", &["Noto Naskh Arabic", "Noto Sans Arabic"]),
        ("Hebr", &["Noto Sans Hebrew", "Noto Serif Hebrew"]),
        ("Deva", &["Noto Sans Devanagari", "Noto Serif Devanagari"]),
        ("Thai", &["Noto Sans Thai", "Noto Serif Thai"]),
        ("Cyrl", &["Noto Sans", "Noto Serif"]),
        ("Grek", &["Noto Sans", "Noto Serif"]),
    ];
    LISTS
        .iter()
        .filter_map(|(tag, families)| {
            Some((
                ScriptTag::new(tag)?,
                families.iter().map(|f| f.to_string()).collect(),
            ))
        })
        .collect()
}

/// What realizing a [`FontSource`] actually produced.
///
/// Exists because the interesting failures here are silent. A caller that
/// prints this once at startup can tell a misconfigured device from a
/// working one without rendering a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontReport {
    /// Faces in the database after every source was applied.
    pub faces: usize,
    /// Generic families that resolved to a name no loaded face carries, as
    /// `(generic, family)` — `("cursive", "IranNastaliq")`. Not an error:
    /// a book that never asks for that generic never notices. It is the
    /// difference between diagnosable and mysterious.
    pub unresolved_generics: Vec<(&'static str, String)>,
}

impl FontReport {
    pub fn is_clean(&self) -> bool {
        self.unresolved_generics.is_empty()
    }
}

impl std::fmt::Display for FontReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} faces", self.faces)?;
        for (generic, family) in &self.unresolved_generics {
            write!(f, "; {generic} -> {family:?} not installed")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_tags_are_four_ascii_bytes_or_nothing() {
        assert_eq!(ScriptTag::new("Grek").unwrap().as_str(), "Grek");
        assert!(ScriptTag::new("Gre").is_none());
        assert!(ScriptTag::new("Greek").is_none());
        // Four chars but eight bytes: rejected rather than truncated.
        assert!(ScriptTag::new("日本語です").is_none());
    }

    #[test]
    fn the_embedded_preset_pins_all_three_axes() {
        let source = FontSource::embedded("/fonts", "Crimson Text");
        assert!(matches!(source.faces[..], [Faces::Dir(_)]));
        assert_eq!(
            source.generics,
            Generics::Explicit(GenericFamilies::all("Crimson Text"))
        );
        assert_eq!(source.fallback, Fallbacks::None);
        assert!(source.locale.is_some());
    }

    #[test]
    fn the_android_preset_names_no_microsoft_family() {
        let Generics::Explicit(g) = FontSource::android_system().generics else {
            panic!("android preset must not defer to the host");
        };
        for name in [g.serif, g.sans_serif, g.monospace, g.cursive, g.fantasy] {
            assert!(
                ![
                    "Times New Roman",
                    "Arial",
                    "Courier New",
                    "Comic Sans MS",
                    "Impact"
                ]
                .contains(&name.as_str()),
                "{name} is a fontdb default, not an Android family"
            );
        }
    }
}
