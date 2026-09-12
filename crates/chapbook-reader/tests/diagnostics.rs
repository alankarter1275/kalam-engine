//! The engine has a voice, and a host can hear it.
//!
//! Everything chapbook reports about a degraded operation used to go to
//! `eprintln!` — which on Android is `/dev/null`, on iOS is not where a
//! developer looks, and in a browser does not exist. The diagnostics for
//! exactly the failures a device shell hits were the ones nobody could see.
//!
//! Its own test binary because `log` allows one logger per process: a
//! capturing logger here cannot fight the one another test file installs.

use std::sync::{Mutex, OnceLock};

use chapbook_core::{FontSource, Source};
use chapbook_reader::{Session, SessionConfig};

/// Records everything logged, so a test can ask what the engine said.
struct Capture;

fn records() -> &'static Mutex<Vec<(log::Level, String, String)>> {
    static RECORDS: OnceLock<Mutex<Vec<(log::Level, String, String)>>> = OnceLock::new();
    RECORDS.get_or_init(|| Mutex::new(Vec::new()))
}

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        // kalam: a host backend drops the parsers' stale warnings the way
        // the stderr logger does (xml5ever warns at the end of every
        // document it finishes); see `chapbook_core::is_dependency_noise`.
        if chapbook_core::is_dependency_noise(record) {
            return;
        }
        records().lock().unwrap().push((
            record.level(),
            record.args().to_string(),
            record.target().to_string(),
        ));
    }
    fn flush(&self) {}
}

/// Installs the capture and holds the floor.
///
/// The capture is process-global, like the logger it implements, so tests
/// that read it cannot run beside each other — one test's `drain` would
/// take another's records. Holding a guard for the body of each test is
/// what makes "what did the engine say" a question with one answer.
fn capturing() -> std::sync::MutexGuard<'static, ()> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    static FLOOR: Mutex<()> = Mutex::new(());
    ONCE.call_once(|| {
        log::set_logger(&Capture).expect("no other logger in this binary");
        log::set_max_level(log::LevelFilter::Trace);
    });
    let guard = FLOOR.lock().unwrap_or_else(|e| e.into_inner());
    drain();
    guard
}

/// Everything said since the last call, clearing as it goes.
fn drain() -> Vec<(log::Level, String, String)> {
    std::mem::take(&mut *records().lock().unwrap())
}

fn fixture(rel: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

fn fixture_fonts() -> FontSource {
    FontSource::embedded(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts"),
        "Crimson Text",
    )
}

#[test]
fn a_book_that_opens_cleanly_says_nothing_alarming() {
    let _floor = capturing();

    let mut session = Session::open_with(
        Source::from(fixture("epub/minimal.epub").as_str()),
        SessionConfig::new(fixture_fonts()),
    )
    .unwrap();
    session.set_metrics(chapbook_core::PageMetrics {
        size: chapbook_core::Size::new(400.0, 600.0),
        margins: chapbook_core::EdgeSizes::uniform(20.0),
        dpi_scale: 1.0,
        rotation: chapbook_core::Rotation::None,
    });
    // Several page turns, which is where a chatty engine would show up.
    for _ in 0..5 {
        session.render();
        session.next_page();
    }

    let said = drain();
    let loud: Vec<_> = said
        .iter()
        .filter(|(level, _, _)| *level <= log::Level::Warn)
        .collect();
    assert!(
        loud.is_empty(),
        "a session at rest should be silent, said: {loud:?}"
    );
}

#[test]
fn records_carry_the_crate_that_emitted_them() {
    // A host filtering `chapbook_reader` apart from `chapbook_layout`
    // needs the target to be the module path, which is what `log` gives by
    // default — worth pinning, because setting an explicit target anywhere
    // would silently take it away.
    let _floor = capturing();

    // kalam: upstream provoked a library warning here; the library is
    // gone, and opening a book reports its timing at `info` from the
    // engine, which is a real engine record all the same.
    let _ = Session::open_with(
        Source::from(fixture("epub/minimal.epub").as_str()),
        SessionConfig::new(fixture_fonts()),
    );
    let said = drain();
    assert!(
        said.iter()
            .any(|(_, _, target)| target.starts_with("chapbook_")),
        "records should be attributable to a chapbook crate: {said:?}"
    );
}
