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
    // answers for serif/sans-serif/monospace on every platform. `Platform`
    // is the same when this target has no table of its own.
    let generics = match &source.generics {
        Generics::Host => None,
        Generics::Platform => platform_generics(),
        Generics::Explicit(g) => Some(g.clone()),
    };
    if let Some(GenericFamilies {
        serif,
        sans_serif,
        monospace,
        cursive,
        fantasy,
    }) = &generics
    {
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

    // The math font of last resort, after the emptiness check (a session
    // whose *source* produced nothing is still refused) and after the
    // report (which describes the source, not the build). Loaded last on
    // purpose: math font selection prefers the latest-loaded MATH-table
    // face, so a publisher's `@font-face` math font (registered later)
    // wins, and this face wins over a math-capable host font.
    #[cfg(feature = "mathml")]
    db.load_font_data(include_bytes!("../assets/STIXTwoMath-Regular.otf").to_vec());

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

/// Family name of the embedded math face. A rendering resource, not a
/// reading typeface — family enumeration for font pickers excludes it.
#[cfg(feature = "mathml")]
pub const MATH_FONT_FAMILY: &str = "STIX Two Math";

/// What the five CSS generics should mean on the target this was built
/// for, or `None` where the platform's own answer is already right.
///
/// `None` is not a gap. On a Linux desktop fontconfig has resolved the
/// generics properly before we are asked, and on macOS and Windows
/// fontdb's Microsoft family names are installed and correct — a table
/// there could only restate the answer or get it wrong. Android and iOS
/// are where nothing corrects fontdb, so they are where a table earns its
/// place.
///
/// Anything this returns can still be wrong for a particular device, which
/// is what [`FontReport::unresolved_generics`] is for: a family naming
/// nothing installed is reported rather than discovered later as missing
/// glyphs. A shell that knows better passes `Generics::Explicit`.
pub fn platform_generics() -> Option<GenericFamilies> {
    generics_for(std::env::consts::OS)
}

/// The table for a named target OS, split out so every table compiles on
/// every host and can be asserted from anywhere.
///
/// A `cfg` per platform would have been the obvious shape and is the wrong
/// one: it makes the Android table code that no Linux build compiles and
/// no Linux test can read, so both a typo and a wrong family name reach a
/// device before they reach anyone. `std::env::consts::OS` is a
/// compile-time constant, so this costs nothing at runtime and the arms
/// that do not apply are eliminated — while staying ordinary code that
/// `the_android_table_names_no_microsoft_family` can call directly.
fn generics_for(os: &str) -> Option<GenericFamilies> {
    match os {
        // Android's own `/system/etc/fonts.xml`, read off an emulator
        // image, with the family names read out of the faces themselves
        // rather than guessed from filenames: DroidSansMono.ttf is
        // "Droid Sans Mono" and DancingScript-Regular.ttf is
        // "Dancing Script".
        //
        // `fantasy` is an alias to `serif` in that file — not to sans,
        // which is what this table used to say before anyone checked.
        // Android ships no fantasy face and its own answer is Noto Serif.
        "android" => Some(GenericFamilies {
            serif: "Noto Serif".into(),
            sans_serif: "Roboto".into(),
            monospace: "Droid Sans Mono".into(),
            cursive: "Dancing Script".into(),
            fantasy: "Noto Serif".into(),
        }),

        // iOS ships the Microsoft core names fontdb happens to default to
        // — Times New Roman, Arial, Courier New and Impact were all found
        // in a recursive scan of `/System/Library/Fonts` during bring-up —
        // so four of the five resolve by luck. Stating them makes it
        // design, and means a future fontdb default cannot quietly move
        // them.
        //
        // `cursive` is the one that missed: no Comic Sans, and no script
        // face this has evidence for. iOS does ship script faces, but
        // naming one from memory is how a table starts lying, so it
        // aliases to the serif choice — a face known present — the same
        // way Android's own file aliases fantasy. The cost is that
        // `cursive` and `serif` render alike on iOS until someone with a
        // device says otherwise; the alternative is a family name that may
        // resolve to nothing.
        "ios" => Some(GenericFamilies {
            serif: "Times New Roman".into(),
            sans_serif: "Helvetica".into(),
            monospace: "Courier New".into(),
            cursive: "Times New Roman".into(),
            fantasy: "Impact".into(),
        }),

        // Linux and the BSDs have fontconfig, macOS and Windows have the
        // Microsoft names actually installed. A table would restate the
        // answer or get it wrong.
        _ => None,
    }
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

    /// Whatever a target's table says, it has to say all five. An empty
    /// name is not a family, and `set_serif_family("")` fails as missing
    /// glyphs rather than as an error — which is the failure mode this
    /// whole module exists to convert into a loud one.
    #[test]
    fn every_platform_table_names_every_generic() {
        for os in ["android", "ios", "linux", "macos", "windows", "freebsd"] {
            let Some(g) = generics_for(os) else { continue };
            for (label, name) in [
                ("serif", &g.serif),
                ("sans-serif", &g.sans_serif),
                ("monospace", &g.monospace),
                ("cursive", &g.cursive),
                ("fantasy", &g.fantasy),
            ] {
                assert!(!name.trim().is_empty(), "{os}: {label} names nothing");
            }
        }
    }

    /// The Android table's whole reason for existing. fontdb defaults to
    /// Microsoft's core family names, none of which is on an Android
    /// device — a scan of an emulator image found 214 faces and not one of
    /// these — so a table that named any of them would be no better than
    /// the default it replaced.
    ///
    /// iOS is deliberately exempt: it *does* ship those families, which is
    /// why four fifths of its table is exactly them.
    #[test]
    fn the_android_table_names_no_microsoft_family() {
        let g = generics_for("android").expect("Android has a table");
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

    /// Read off `/system/etc/fonts.xml` on a running emulator: Android
    /// aliases `fantasy` to `serif`, having no fantasy face of its own.
    /// This table said `Roboto` — the sans face — until someone looked.
    #[test]
    fn android_fantasy_follows_its_serif_the_way_the_platform_does() {
        let g = generics_for("android").expect("Android has a table");
        assert_eq!(g.fantasy, g.serif);
        assert_ne!(g.fantasy, g.sans_serif);
    }

    /// On a target with no table of its own, asking for chapbook's answer
    /// and asking the platform's are the same question. A shell should not
    /// have to know which targets those are.
    #[test]
    fn platform_generics_fall_through_to_the_host_where_there_is_no_table() {
        if platform_generics().is_some() {
            return;
        }
        let source = |generics| FontSource {
            faces: vec![Faces::Dir(fixture_dir())],
            generics,
            fallback: Fallbacks::None,
            locale: Some("en-US".into()),
        };
        let (platform, _) = build_font_system(&source(Generics::Platform)).unwrap();
        let (host, _) = build_font_system(&source(Generics::Host)).unwrap();
        for family in [
            fontdb::Family::Serif,
            fontdb::Family::SansSerif,
            fontdb::Family::Monospace,
            fontdb::Family::Cursive,
            fontdb::Family::Fantasy,
        ] {
            assert_eq!(
                platform.db().family_name(&family),
                host.db().family_name(&family),
                "{family:?} differs between Platform and Host"
            );
        }
    }
}
