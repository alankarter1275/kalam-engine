//! Reading settings: applying them, reading them back, and relayout that
//! keeps the reader's place. (kalam: persistence is the host's now.)

mod common;
use common::*;

#[test]
fn settings_apply_for_the_session_and_a_fresh_session_starts_clean() {
    // kalam: upstream persisted settings per scope through its library and
    // proved they came back on reopen. The library is gone: the host
    // (Kalam) keeps its preferences and applies them on every open, so
    // what the engine owes is that a change sticks for the life of the
    // session, that every field a shell can set is honoured, and that a
    // new session does *not* inherit anything from an old one.
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

        // The fields no shell could reach before.
        let mut wide = s.settings().clone();
        wide.justify = true;
        wide.line_height = 1.9;
        wide.publisher_styles = false;
        s.set_settings(wide, SettingsScope::ThisBook);
        assert!(s.settings().justify);
        assert_eq!(s.settings().line_height, 1.9);
        assert!(!s.settings().publisher_styles);
        assert_eq!(
            s.settings().base_font_px,
            22.0,
            "the rest survived the call"
        );
    }

    let s = reopen_isolated("epub-settings", &source);
    assert_eq!(
        s.settings().base_font_px,
        18.0,
        "nothing leaks between sessions"
    );
    assert_eq!(s.settings().theme, chapbook_core::Theme::Light);
    assert!(!s.settings().justify);
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

/// The setting travels the same road as every other one: set through
/// `set_settings`, read back from `settings()`, and gone with the session.
#[test]
fn a_chosen_font_is_read_back_and_does_not_outlive_the_session() {
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

    let s = reopen_isolated("epub-font-choice", &source);
    assert_eq!(
        s.settings().font_family,
        None,
        "a fresh session starts on the publisher's font again"
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
