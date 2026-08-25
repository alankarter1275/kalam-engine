//! Run the shell conformance harness over every format the reader opens.
//!
//! The harness is the deliverable, not these tests: it ships so a shell
//! author outside this workspace can point it at their own `Session` and
//! find out whether they drive it correctly. What runs here is the
//! harness against itself — proof that the rules it asserts are rules
//! this engine actually keeps, on a text book, an image book, and a PDF.

use std::path::PathBuf;
use std::time::Duration;

use chapbook_reader::conformance::{self, Check, Harness, Outcome};
use chapbook_reader::Session;

fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// The library dir is process-global env, and tests run in parallel: hold
/// the lock across set-and-open, exactly as `tests/session.rs` does.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A factory the harness can call repeatedly. Each call reopens the *same*
/// library dir, which is what `PositionSurvivesARestart` needs in order to
/// have anything to restore from; the dir is wiped once, here, rather than
/// The vendored fixture faces, all three axes pinned, so this suite means
/// the same thing on Linux, on a Mac and on a device. Taking the host's
/// fonts is what pinned two of these assertions to one machine's
/// collection.
fn fixture_fonts() -> chapbook_core::FontSource {
    chapbook_core::FontSource::embedded(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts"),
        "Crimson Text",
    )
}

/// per call.
fn factory(name: &'static str, source: String) -> impl FnMut() -> Session {
    let dir = std::env::temp_dir().join(format!(
        "chapbook-conformance-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    move || {
        let guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);
        let session = Session::open(&source, fixture_fonts()).unwrap();
        drop(guard);
        session
    }
}

fn run(name: &'static str, rel: &str) -> conformance::Report {
    Harness::new(factory(name, fixture(rel)))
        .budget(Duration::from_secs(20))
        .run()
}

/// A skip is an honest answer, but a run that skipped everything would
/// pass while proving nothing — so name the checks each fixture is
/// expected to actually exercise.
#[track_caller]
fn assert_ran(report: &conformance::Report, expected: &[Check]) {
    report.assert_ok();
    for want in expected {
        let finding = report
            .findings
            .iter()
            .find(|f| f.check == *want)
            .unwrap_or_else(|| panic!("{want} was not in the report\n{report}"));
        assert_eq!(
            finding.outcome,
            Outcome::Passed,
            "{want} was expected to run here, not skip\n{report}"
        );
    }
}

/// `illustrated.epub` is one chapter, with images and embedded fonts —
/// the layout-heavy fixture. It has no unit boundary, so the crossing
/// check is expected to skip here and is covered by `minimal.epub`.
#[test]
fn a_text_book_conforms() {
    let report = run("epub", "epub/illustrated.epub");
    assert_ran(
        &report,
        &[
            Check::PageTurnsWalkTheWholeBook,
            Check::TurnsAreReversible,
            Check::TheEndOfTheBookStandsStill,
            Check::ResizeKeepsThePlace,
            Check::RotationIsNotAReflow,
            Check::FrameConsumesTheChangeRecord,
            Check::DamageStaysInsideThePage,
            Check::ASelectionLivesAndDiesWithThePage,
            Check::PositionSurvivesARestart,
        ],
    );
}

/// Two chapters, so a forward turn has a unit boundary to cross — the
/// case the fbdev shell got wrong.
#[test]
fn a_multi_unit_text_book_conforms() {
    let report = run("epub-units", "epub/minimal.epub");
    assert_ran(
        &report,
        &[
            Check::ACrossedUnitStillMoves,
            Check::PageTurnsWalkTheWholeBook,
            Check::TurnsAreReversible,
            Check::TheEndOfTheBookStandsStill,
            Check::ResizeKeepsThePlace,
            Check::FrameConsumesTheChangeRecord,
            Check::PositionSurvivesARestart,
        ],
    );
}

#[test]
#[cfg(feature = "cbz")]
fn an_image_book_conforms() {
    let report = run("cbz", "cbz/minimal.cbz");
    // A comic has no text layer, so the selection check is expected to
    // skip; everything about turning and restoring still applies.
    assert_ran(
        &report,
        &[
            Check::ACrossedUnitStillMoves,
            Check::PageTurnsWalkTheWholeBook,
            Check::TheEndOfTheBookStandsStill,
            Check::PendingLoadsConverge,
            Check::PositionSurvivesARestart,
        ],
    );
    let selection = report
        .findings
        .iter()
        .find(|f| f.check == Check::ASelectionLivesAndDiesWithThePage)
        .unwrap();
    assert!(matches!(selection.outcome, Outcome::Skipped(_)));
}

#[test]
#[cfg(feature = "pdf")]
fn a_pdf_conforms() {
    let report = run("pdf", "pdf/minimal.pdf");
    assert_ran(
        &report,
        &[
            Check::PageTurnsWalkTheWholeBook,
            Check::TheEndOfTheBookStandsStill,
            Check::PendingLoadsConverge,
        ],
    );
}

/// The harness has to be able to fail, or running it proves nothing. Drive
/// a shell's exit test the broken way the fbdev example was written the
/// first time — compare `page()` across a turn — and confirm it stops
/// early on a book that opens with a one-page unit, while the fixed
/// comparison walks the whole thing.
#[test]
fn the_broken_exit_test_is_still_broken() {
    let mut open = factory("regression", fixture("epub/minimal.epub"));
    let metrics = conformance::default_metrics();

    let mut session = open();
    session.set_metrics(metrics);
    conformance::settle(&mut session, Duration::from_secs(20));
    let one_page_unit = session.page_count() == 1;

    // How the shell used to decide it had moved.
    let mut by_page = 0;
    loop {
        let before = session.page();
        session.next_page();
        if session.page() == before {
            break;
        }
        by_page += 1;
        conformance::settle(&mut session, Duration::from_secs(20));
    }

    // How it decides now.
    let mut session = open();
    session.set_metrics(metrics);
    conformance::settle(&mut session, Duration::from_secs(20));
    let mut by_position = 0;
    while session.next_page() {
        by_position += 1;
        conformance::settle(&mut session, Duration::from_secs(20));
    }

    assert!(
        by_position > by_page,
        "the page-only comparison was supposed to stop short: {by_page} turns \
         vs {by_position}"
    );
    if one_page_unit {
        assert_eq!(
            by_page, 0,
            "a book opening on a single-page unit should stop the page-only \
             comparison on the very first turn"
        );
    }
}
