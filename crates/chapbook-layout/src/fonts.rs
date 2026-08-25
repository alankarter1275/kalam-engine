//! Turning a [`FontSource`] into a cosmic-text [`FontSystem`].
//!
//! `chapbook_core::font` says why the description exists and what each of
//! its three axes is for; this module is the half that knows about fontdb
//! and cosmic-text. Nothing here calls `FontSystem::new()`, and that is the
//! point: `new()` decides all three axes from `cfg` and says nothing about
//! having decided.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use cosmic_text::{fontdb, Fallback, FontSystem};
use unicode_script::Script;

use chapbook_core::{
    ChapbookError, Faces, FallbackFamilies, Fallbacks, FontReport, FontSource, GenericFamilies,
    Generics, Result, ScriptTag,
};

/// Build the font system a source describes, with a report of anything that
/// resolved to nothing.
///
/// Fails when the source produces no faces: a fontless session paginates
/// every book to one blank page and still lays out, renders, paints and
/// conforms, so the only place it can be caught is here.
pub fn build_font_system(source: &FontSource) -> Result<(FontSystem, FontReport)> {
    let mut db = fontdb::Database::new();
    for faces in &source.faces {
        match faces {
            Faces::Dir(dir) => db.load_fonts_dir(dir),
            Faces::Bytes(bytes) => db.load_font_data(bytes.to_vec()),
            Faces::Host => db.load_system_fonts(),
        }
    }
    if db.is_empty() {
        return Err(ChapbookError::Font(format!(
            "font source produced no faces ({}). A session with no fonts \
             lays out and renders but paginates every book to one blank \
             page, so this is refused here rather than discovered later.",
            describe(&source.faces)
        )));
    }

    // Generics. `Host` means whatever fontdb (and, on Linux, fontconfig)
    // already put there during `load_system_fonts` — deliberately *not*
    // cosmic-text's three hardcoded overrides, which discard fontconfig's
    // answers for serif/sans-serif/monospace on every platform.
    if let Generics::Explicit(g) = &source.generics {
        let GenericFamilies {
            serif,
            sans_serif,
            monospace,
            cursive,
            fantasy,
        } = g;
        db.set_serif_family(serif);
        db.set_sans_serif_family(sans_serif);
        db.set_monospace_family(monospace);
        db.set_cursive_family(cursive);
        db.set_fantasy_family(fantasy);
    }

    let report = FontReport {
        faces: db.len(),
        unresolved_generics: unresolved_generics(&db),
    };

    let locale = source
        .locale
        .clone()
        .unwrap_or_else(|| sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string()));

    let fonts = match &source.fallback {
        Fallbacks::Platform => FontSystem::new_with_locale_and_db(locale, db),
        Fallbacks::None => {
            FontSystem::new_with_locale_and_db_and_fallback(locale, db, ListFallback::empty())
        }
        Fallbacks::Explicit(families) => {
            FontSystem::new_with_locale_and_db_and_fallback(locale, db, ListFallback::new(families))
        }
    };
    Ok((fonts, report))
}

fn describe(faces: &[Faces]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for source in faces {
        parts.push(match source {
            Faces::Dir(dir) => format!("dir {}", dir.display()),
            Faces::Bytes(bytes) => format!("{} bytes", bytes.len()),
            Faces::Host => "host fonts — fontdb has no Android, iOS or wasm branch".to_string(),
        });
    }
    if parts.is_empty() {
        return "no sources given".to_string();
    }
    parts.join(", ")
}

/// Generic families naming something no loaded face answers to.
fn unresolved_generics(db: &fontdb::Database) -> Vec<(&'static str, String)> {
    let installed: HashSet<String> = db
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.to_lowercase()))
        .collect();
    [
        ("serif", fontdb::Family::Serif),
        ("sans-serif", fontdb::Family::SansSerif),
        ("monospace", fontdb::Family::Monospace),
        ("cursive", fontdb::Family::Cursive),
        ("fantasy", fontdb::Family::Fantasy),
    ]
    .into_iter()
    .filter_map(|(label, family)| {
        let name = db.family_name(&family).to_string();
        (!installed.contains(&name.to_lowercase())).then_some((label, name))
    })
    .collect()
}

/// Every family name a source has ever named, as `&'static str`.
///
/// cosmic-text's [`Fallback`] returns `&[&'static str]`, which suits a list
/// compiled into the binary and not one a caller assembled at runtime — and
/// a caller assembling one at runtime is the whole reason [`FontSource`]
/// exists. Interning rather than leaking per call, so that constructing many
/// sessions with the same source (a test suite does exactly that) costs the
/// strings once.
fn intern(name: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashSet::new()));
    let mut pool = pool.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = pool.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// [`Fallback`] over an owned list.
#[derive(Debug)]
struct ListFallback {
    common: Vec<&'static str>,
    forbidden: Vec<&'static str>,
    /// Sorted by tag so a lookup is a binary search rather than a scan; the
    /// shaper asks once per script run.
    per_script: Vec<(ScriptTag, Vec<&'static str>)>,
}

impl ListFallback {
    fn empty() -> ListFallback {
        ListFallback {
            common: Vec::new(),
            forbidden: Vec::new(),
            per_script: Vec::new(),
        }
    }

    fn new(families: &FallbackFamilies) -> ListFallback {
        let intern_all = |names: &[String]| names.iter().map(|n| intern(n)).collect();
        let mut per_script: Vec<(ScriptTag, Vec<&'static str>)> = families
            .per_script
            .iter()
            .map(|(tag, names)| (*tag, intern_all(names)))
            .collect();
        per_script.sort_by_key(|(tag, _)| *tag);
        ListFallback {
            common: intern_all(&families.common),
            forbidden: intern_all(&families.forbidden),
            per_script,
        }
    }
}

impl Fallback for ListFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &self.common
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        &self.forbidden
    }

    fn script_fallback(&self, script: Script, _locale: &str) -> &[&'static str] {
        // `short_name` is the ISO 15924 code, which is what `ScriptTag` is.
        let Some(tag) = ScriptTag::new(script.short_name()) else {
            return &[];
        };
        match self.per_script.binary_search_by_key(&tag, |(t, _)| *t) {
            Ok(i) => &self.per_script[i].1,
            Err(_) => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts")
    }

    #[test]
    fn an_embedded_source_pins_every_generic_to_its_one_family() {
        let source = FontSource::embedded(fixture_dir(), "Crimson Text");
        let (fonts, report) = build_font_system(&source).unwrap();
        assert_eq!(report.faces, 4);
        assert!(report.is_clean(), "{report}");
        for family in [
            fontdb::Family::Serif,
            fontdb::Family::SansSerif,
            fontdb::Family::Monospace,
            fontdb::Family::Cursive,
            fontdb::Family::Fantasy,
        ] {
            assert_eq!(fonts.db().family_name(&family), "Crimson Text");
        }
    }

    /// The failure this whole type exists to convert into an error: fontdb
    /// has no Android, iOS or wasm branch, and an empty database still lays
    /// out, renders, paints and conforms.
    #[test]
    fn a_source_that_finds_nothing_is_an_error_not_an_empty_database() {
        let source = FontSource::embedded("/nonexistent/font/dir", "Whatever");
        let err = build_font_system(&source).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("no faces"), "{message}");
        assert!(message.contains("/nonexistent/font/dir"), "{message}");
    }

    /// A generic naming a family nobody installed is exactly what a Linux
    /// box does today for `cursive`. Reported, not fatal.
    #[test]
    fn a_generic_naming_an_absent_family_is_reported() {
        let mut source = FontSource::embedded(fixture_dir(), "Crimson Text");
        source.generics = Generics::Explicit(GenericFamilies {
            serif: "Crimson Text".into(),
            sans_serif: "Crimson Text".into(),
            monospace: "Crimson Text".into(),
            cursive: "IranNastaliq".into(),
            fantasy: "Crimson Text".into(),
        });
        let (_fonts, report) = build_font_system(&source).unwrap();
        assert_eq!(
            report.unresolved_generics,
            vec![("cursive", "IranNastaliq".to_string())]
        );
        assert!(!report.is_clean());
        assert!(report.to_string().contains("not installed"), "{report}");
    }

    #[test]
    fn script_fallback_answers_by_iso_15924_tag() {
        let families = FallbackFamilies {
            common: vec!["Noto Sans".into()],
            per_script: vec![
                (ScriptTag::new("Grek").unwrap(), vec!["Noto Serif".into()]),
                (
                    ScriptTag::new("Hani").unwrap(),
                    vec!["Noto Sans CJK SC".into()],
                ),
            ],
            forbidden: vec!["Bad Font".into()],
        };
        let fallback = ListFallback::new(&families);

        assert_eq!(fallback.common_fallback(), &["Noto Sans"]);
        assert_eq!(fallback.forbidden_fallback(), &["Bad Font"]);
        assert_eq!(
            fallback.script_fallback(Script::Greek, "en-US"),
            &["Noto Serif"]
        );
        assert_eq!(
            fallback.script_fallback(Script::Han, "en-US"),
            &["Noto Sans CJK SC"]
        );
        // A script with no entry falls through to `common`, which the
        // shaper appends itself.
        assert!(fallback
            .script_fallback(Script::Cyrillic, "en-US")
            .is_empty());
    }

    /// Interning, not leaking per call: the same name twice is the same
    /// pointer, so a suite that builds hundreds of sessions pays once.
    #[test]
    fn family_names_intern() {
        let a = intern("Noto Sans Interning Probe");
        let b = intern("Noto Sans Interning Probe");
        assert!(std::ptr::eq(a, b));
    }

    /// `Fallbacks::None` is what makes a golden mean the same thing on every
    /// host. Pinning the faces alone does not: the platform list varies
    /// underneath, and cosmic-text picks it by `cfg`.
    #[test]
    fn the_embedded_preset_takes_no_platform_fallback() {
        assert_eq!(
            FontSource::embedded(fixture_dir(), "Crimson Text").fallback,
            Fallbacks::None
        );
    }
}
