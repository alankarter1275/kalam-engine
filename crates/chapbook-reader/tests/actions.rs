//! The input seam: `Action`s routing to the verbs a shell would have
//! called by hand, and the tap zones the book's direction flips.

mod common;
use common::*;

#[test]
fn actions_route_to_the_verbs_a_shell_would_have_called_by_hand() {
    use chapbook_core::{Action, ActionOutcome};

    let mut s = open_isolated("epub-apply-nav", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    let start = s.locator();
    assert_eq!(s.apply(Action::NextPage), ActionOutcome::Changed);
    let after = s.locator();
    assert_ne!(after, start, "and it went there");

    assert_eq!(s.apply(Action::PrevPage), ActionOutcome::Changed);
    assert_eq!(s.locator(), start, "back where it started");

    // Nothing before the first page, so nothing to repaint — but the key
    // is still the reader's. A shell that propagated this one would be
    // handing the platform a press it had already claimed, which on
    // Android is how the volume slider ends up over the book.
    let stuck = s.apply(Action::PrevPage);
    assert_eq!(stuck, ActionOutcome::Unchanged, "no page before the first");
    assert!(!stuck.needs_redraw());
    assert!(stuck.consumed(), "and the reader still took the key");
    assert_eq!(s.locator(), start);

    if s.spine_len() > 1 {
        assert_eq!(s.apply(Action::NextUnit), ActionOutcome::Changed);
        assert_eq!(s.page(), 0, "a unit skip lands on its first page");
        assert_eq!(s.apply(Action::PrevUnit), ActionOutcome::Changed);
    }
    assert_eq!(
        s.apply(Action::PrevUnit),
        ActionOutcome::Unchanged,
        "no unit before the first"
    );
}

#[test]
fn back_declines_at_the_bottom_of_the_stack_so_the_platform_can_have_it() {
    use chapbook_core::{Action, ActionOutcome, Locator};

    let mut s = open_isolated("epub-apply-back", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert!(s.spine_len() > 1, "the fixture needs somewhere to jump");

    // The case a bool could not express. The engine understands `Back`
    // perfectly well and is declining it anyway, because an empty trail
    // is exactly where both mobile platforms expect their own Back to
    // take over and leave the reader. A shell can forward the gesture
    // unconditionally instead of shadowing the history to know when not
    // to.
    assert!(!s.can_go_back());
    let empty = s.apply(Action::Back);
    assert_eq!(empty, ActionOutcome::Unhandled);
    assert!(!empty.consumed(), "the system Back gets the gesture");

    // With a trail it is an ordinary reading action again.
    let start = s.locator();
    assert!(s.goto(Locator::chapter_start(1)), "jumped to chapter two");
    assert!(s.can_go_back());
    let went = s.apply(Action::Back);
    assert_eq!(went, ActionOutcome::Changed);
    assert!(went.consumed(), "and this one the reader keeps");
    assert_eq!(s.locator(), start, "back where it started");
}

#[test]
fn font_actions_step_by_the_engines_amount_and_stop_at_the_clamp() {
    use chapbook_core::{Action, ActionOutcome};
    use chapbook_reader::FONT_STEP_PX;

    let mut s = open_isolated("epub-apply-font", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    let start = s.settings().base_font_px;
    assert_eq!(s.apply(Action::FontUp), ActionOutcome::Changed);
    assert_eq!(s.settings().base_font_px, start + FONT_STEP_PX);
    assert_eq!(s.apply(Action::FontDown), ActionOutcome::Changed);
    assert_eq!(s.settings().base_font_px, start, "and back down again");

    // `adjust_font` clamps, so at the stop the outcome has to say nothing
    // moved rather than ask for a redraw of an identical page. Bounded
    // well above the number of steps the 10–40 range can hold.
    let mut steps = 0;
    while s.apply(Action::FontDown).needs_redraw() {
        steps += 1;
        assert!(steps < 100, "the clamp never arrived");
    }
    let floor = s.settings().base_font_px;
    let stuck = s.apply(Action::FontDown);
    assert_eq!(
        stuck,
        ActionOutcome::Unchanged,
        "still refused at the floor"
    );
    assert!(stuck.consumed(), "a clamped font key is not the platform's");
    assert_eq!(s.settings().base_font_px, floor, "and did not drift");
    assert_eq!(
        s.apply(Action::FontUp),
        ActionOutcome::Changed,
        "the other direction still moves"
    );
}

#[test]
fn cycling_the_theme_is_an_action_and_the_menu_is_not_the_engines() {
    use chapbook_core::{Action, ActionOutcome, Theme};

    let mut s = open_isolated("epub-apply-misc", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    render_loaded(&mut s);

    let start = s.settings().theme;
    assert_eq!(s.apply(Action::CycleTheme), ActionOutcome::Changed);
    assert_eq!(s.settings().theme, start.cycle());

    // Three variants, so the cycle comes home and every step redraws.
    assert_eq!(s.apply(Action::CycleTheme), ActionOutcome::Changed);
    assert_eq!(s.apply(Action::CycleTheme), ActionOutcome::Changed);
    assert_eq!(s.settings().theme, start);
    assert_eq!(Theme::default().cycle().cycle().cycle(), Theme::default());

    // `Unhandled`, not `Unchanged`: the engine has no chrome to toggle,
    // so the shell that bound this key has to get the event back rather
    // than read a quiet "nothing moved" and swallow it.
    let before = s.locator();
    let menu = s.apply(Action::ToggleMenu);
    assert_eq!(menu, ActionOutcome::Unhandled);
    assert!(!menu.consumed(), "the shell gets its own key back");
    assert!(!menu.needs_redraw());
    assert_eq!(s.locator(), before, "and it touched nothing on the way");
}

#[test]
fn an_rtl_book_flips_the_tap_zones_without_the_shell_deciding_anything() {
    use chapbook_core::{Action, ReadingDirection, TapZones};

    let mut ltr = open_isolated("epub-dir-ltr", &fixture("epub/minimal.epub"));
    ltr.set_metrics(metrics());
    render_loaded(&mut ltr);
    assert_eq!(ltr.reading_direction(), ReadingDirection::Ltr);

    let mut rtl = open_isolated("epub-dir-rtl", &fixture("epub/rtl.epub"));
    rtl.set_metrics(metrics());
    render_loaded(&mut rtl);
    assert_eq!(rtl.reading_direction(), ReadingDirection::Rtl);

    let m = ltr.metrics().expect("metrics were set");
    let (near, far, mid) = (m.size.w * 0.1, m.size.w * 0.9, m.size.h / 2.0);

    // One line of shell code, written once, correct on both books —
    // which is the whole reason the direction is the engine's to report.
    let ltr_zones = TapZones::new(ltr.reading_direction());
    let rtl_zones = TapZones::new(rtl.reading_direction());

    assert_eq!(ltr_zones.action_at(near, mid, &m), Some(Action::PrevPage));
    assert_eq!(ltr_zones.action_at(far, mid, &m), Some(Action::NextPage));
    // The same tap on the same page box, meaning the opposite thing,
    // because the package said so and nothing else had to.
    assert_eq!(rtl_zones.action_at(near, mid, &m), Some(Action::NextPage));
    assert_eq!(rtl_zones.action_at(far, mid, &m), Some(Action::PrevPage));

    // And the bug this prevents: a shell reaching for `TapZones::default`
    // gets the left-to-right answer on the right-to-left book, silently,
    // and the reader pages backwards.
    assert_eq!(
        TapZones::default().action_at(near, mid, &m),
        Some(Action::PrevPage)
    );
}
