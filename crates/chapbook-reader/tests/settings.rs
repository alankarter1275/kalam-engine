//! Reading settings: persistence, per-book overrides, and relayout
//! that keeps the reader's place.

mod common;
use common::*;

#[test]
fn settings_survive_a_restart_and_can_be_overridden_per_book() {
    use chapbook_reader::SettingsScope;

    let source = fixture("epub/minimal.epub");
    {
        let mut s = open_isolated("epub-settings", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");
        assert_eq!(s.settings().base_font_px, 18.0, "the built-in default");

        s.adjust_font(4.0);
        s.cycle_theme();
        assert_eq!(s.settings().base_font_px, 22.0);
        assert_eq!(s.settings().theme, chapbook_core::Theme::Sepia);
    }

    // Font size used not to survive a restart. It does now.
    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert_eq!(s.settings().base_font_px, 22.0);
    assert_eq!(s.settings().theme, chapbook_core::Theme::Sepia);

    // The fields no shell could reach before.
    let mut wide = s.settings().clone();
    wide.justify = true;
    wide.line_height = 1.9;
    wide.publisher_styles = false;
    s.set_settings(wide, SettingsScope::ThisBook);
    assert!(s.settings().justify);
    drop(s);

    // A per-book override outlives a later change to the default.
    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert!(s.settings().justify, "the override loaded");
    assert_eq!(s.settings().line_height, 1.9);
    assert!(!s.settings().publisher_styles);

    let mut plain = s.settings().clone();
    plain.base_font_px = 12.0;
    plain.justify = false;
    s.set_settings(plain, SettingsScope::Global);
    drop(s);

    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert!(
        s.settings().justify,
        "the book keeps its override when the default moves"
    );
    assert_eq!(s.settings().base_font_px, 22.0);

    // Clearing the override hands the book back to the default.
    s.clear_book_settings();
    assert_eq!(s.settings().base_font_px, 12.0);
    assert!(!s.settings().justify);
    drop(s);

    let mut s = reopen_isolated("epub-settings", &source);
    s.set_metrics(metrics());
    assert_eq!(s.settings().base_font_px, 12.0, "and it stays cleared");
}

#[test]
fn a_settings_change_relayouts_and_keeps_the_place() {
    use chapbook_reader::SettingsScope;

    let mut s = open_isolated("epub-settings-relayout", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");
    let before = s.render().expect("page renders").data().to_vec();

    let mut bigger = s.settings().clone();
    bigger.base_font_px += 6.0;
    s.set_settings(bigger, SettingsScope::Global);
    assert_eq!(
        s.frame().unwrap().intent,
        chapbook_reader::chapbook_paint::FrameIntent::Relayout
    );
    assert_ne!(
        s.render().expect("page renders").data(),
        &before[..],
        "the page reflowed"
    );
    assert_eq!(s.spine(), 0, "and the reader stayed put");
}

// ---- The reader's chosen typeface ----

/// The read-back half of the choice. A picker cannot offer what it cannot
/// enumerate, and this is the list it offers.
#[test]
fn the_session_enumerates_the_families_it_can_match() {
    let mut s = open_isolated("epub-font-list", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let families = s.font_families();
    assert!(
        families.contains(&"Crimson Text".to_string()),
        "the vendored fixture face should be offered: {families:?}"
    );
    assert!(
        families.windows(2).all(|w| w[0] <= w[1]),
        "families come back sorted: {families:?}"
    );
    // A picker must never offer the math face: it is a rendering resource,
    // not something anyone reads a novel in.
    assert!(
        !families.contains(&"STIX Two Math".to_string()),
        "the math face leaked into the picker: {families:?}"
    );
}

/// The setting travels the same road as every other one: persisted per
/// scope, restored on reopen, cleared with the rest of a book's override.
#[test]
fn a_chosen_font_survives_a_restart_and_scopes_per_book() {
    use chapbook_reader::SettingsScope;

    let source = fixture("epub/minimal.epub");
    {
        let mut s = open_isolated("epub-font-choice", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");
        assert_eq!(
            s.settings().font_family,
            None,
            "the built-in default is the publisher's font"
        );

        let chosen = chapbook_core::ReadingSettings {
            font_family: Some("Crimson Text".into()),
            ..s.settings().clone()
        };
        s.set_settings(chosen, SettingsScope::ThisBook);
        assert_eq!(s.settings().font_family.as_deref(), Some("Crimson Text"));
    }

    let mut s = reopen_isolated("epub-font-choice", &source);
    s.set_metrics(metrics());
    assert_eq!(
        s.settings().font_family.as_deref(),
        Some("Crimson Text"),
        "the choice did not come back from the database"
    );

    s.clear_book_settings();
    assert_eq!(
        s.settings().font_family,
        None,
        "clearing the override returns the book to the publisher's font"
    );
}

/// Changing the typeface reflows the book, so it has to be part of what
/// keys a layout cache — otherwise the reader picks a font and the page
/// does not move.
#[test]
fn choosing_a_font_relayouts_and_keeps_the_place() {
    let mut s = open_isolated("epub-font-reflow", &fixture("epub/long.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");
    for _ in 0..3 {
        s.next_page();
    }
    let before = s.position();

    let plain = s.settings().clone();
    let chosen = chapbook_core::ReadingSettings {
        font_family: Some("Crimson Text".into()),
        ..plain.clone()
    };
    assert_ne!(
        plain.cache_key(),
        chosen.cache_key(),
        "a font change that does not move the cache key cannot reflow"
    );

    s.set_settings(chosen, chapbook_reader::SettingsScope::Global);
    s.render().expect("page renders after the change");
    assert_eq!(
        s.position().spine,
        before.spine,
        "the reader should still be in the same unit"
    );
}
