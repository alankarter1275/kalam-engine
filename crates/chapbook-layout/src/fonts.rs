//! Font system construction.
//!
//! Layout results depend on the fonts available, so tests and golden dumps
//! use a font system built from a fixed directory of vendored fonts with
//! pinned default families — never the host's font collection.

use std::path::Path;

use cosmic_text::{fontdb, FontSystem};

/// Font system from the host's installed fonts (viewer default).
pub fn system_font_system() -> FontSystem {
    FontSystem::new()
}

/// Deterministic font system: only the fonts in `dir`, with every generic
/// family defaulted to `default_family`. Used by tests, golden dumps, and
/// the CLI so layout output is identical on every machine.
pub fn fixture_font_system(dir: &Path, default_family: &str) -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_fonts_dir(dir);
    db.set_serif_family(default_family);
    db.set_sans_serif_family(default_family);
    db.set_monospace_family(default_family);
    db.set_cursive_family(default_family);
    db.set_fantasy_family(default_family);
    FontSystem::new_with_locale_and_db("en-US".to_string(), db)
}
