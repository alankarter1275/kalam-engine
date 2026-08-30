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
