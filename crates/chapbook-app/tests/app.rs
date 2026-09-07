//! The application model, driven the way a front end drives it.
//!
//! Every test gets its own library directory through `App::open`'s
//! explicit argument, so unlike the session tests there is no process
//! environment to serialize over.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chapbook_app::chapbook_library::{Library, ReadingState};
use chapbook_app::chapbook_opds::progression::Device;
use chapbook_app::chapbook_opds::{HttpClient, HttpError, HttpRequest, HttpResponse};
use chapbook_app::chapbook_reader::chapbook_core::{
    Action, EdgeSizes, PageMetrics, Rotation, Size,
};
use chapbook_app::chapbook_reader::Session;
use chapbook_app::chapbook_sync::{PositionReport, SyncEngine};
use chapbook_app::{App, ShelfFilter, SyncDriver, SyncRequest, SyncStatus};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("chapbook-app-{tag}-{}", std::process::id()));
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir).expect("create test dir");
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/epub")
        .join(name)
}

fn metrics() -> PageMetrics {
    PageMetrics {
        size: Size::new(400.0, 600.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
        rotation: Rotation::None,
    }
}

/// Lay the session out so navigation and position capture have pages to
/// work with.
fn settle(session: &mut Session) {
    session.set_metrics(metrics());
    let _ = session.render();
}

#[test]
fn an_imported_book_is_on_the_shelf_with_its_series() {
    let dir = TempDir::new("import");
    let mut app = App::open(Some(dir.path())).expect("open app");
    let record = app.import(&fixture("series.epub")).expect("import");
    assert!(!record.title.is_empty());
    assert!(record.series.is_some(), "series.epub declares a series");

    let shelf = app.shelf(&ShelfFilter::default()).expect("shelf");
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf[0].id, record.id);
}

#[test]
fn search_narrows_and_a_miss_is_empty() {
    let dir = TempDir::new("search");
    let mut app = App::open(Some(dir.path())).expect("open app");
    let series = app.import(&fixture("series.epub")).expect("import series");
    app.import(&fixture("minimal.epub"))
        .expect("import minimal");

    let hit = app
        .shelf(&ShelfFilter {
            search: series.title.clone(),
            ..ShelfFilter::default()
        })
        .expect("narrowed shelf");
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].id, series.id);

    let miss = app
        .shelf(&ShelfFilter {
            search: "no-such-book-zzz".into(),
            ..ShelfFilter::default()
        })
        .expect("missed shelf");
    assert!(miss.is_empty());
}

#[test]
fn a_reopened_book_is_where_the_reader_left_it() {
    let dir = TempDir::new("reopen");
    let mut app = App::open(Some(dir.path())).expect("open app");
    let record = app.import(&fixture("long.epub")).expect("import");

    let mut session = app.open_book(record.id).expect("open");
    settle(&mut session);
    assert!(session.apply(Action::NextPage).needs_redraw());
    let left_at = session.position();
    assert_ne!((left_at.spine, left_at.page), (0, 0), "the page turned");
    session.save_position();
    drop(session);

    let mut session = app.open_book(record.id).expect("reopen");
    settle(&mut session);
    let restored = session.position();
    assert_eq!(
        (restored.spine, restored.page),
        (left_at.spine, left_at.page)
    );
}

#[test]
fn finished_shows_under_its_own_filter() {
    let dir = TempDir::new("finished");
    let mut app = App::open(Some(dir.path())).expect("open app");
    let record = app.import(&fixture("minimal.epub")).expect("import");
    app.set_finished(record.id, true).expect("finish");

    let finished = app
        .shelf(&ShelfFilter {
            state: Some(ReadingState::Finished),
            ..ShelfFilter::default()
        })
        .expect("finished shelf");
    assert_eq!(finished.len(), 1);

    let unread = app
        .shelf(&ShelfFilter {
            state: Some(ReadingState::Unread),
            ..ShelfFilter::default()
        })
        .expect("unread shelf");
    assert!(unread.is_empty());
}

#[test]
fn sync_with_nothing_syncable_declines_without_a_driver() {
    let dir = TempDir::new("nosync");
    let mut app = App::open(Some(dir.path())).expect("open app");
    app.import(&fixture("minimal.epub")).expect("import");
    let started = app.sync_all(Arc::new(|| {})).expect("sync_all");
    assert!(!started, "a sideloaded shelf has nothing to sync");
    assert!(app.sync_events().is_empty());
}

/// A transport with nobody on the other end.
struct DeadHttp;

impl HttpClient for DeadHttp {
    fn get(&self, _request: HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::new("nobody home"))
    }
}

#[test]
fn the_driver_reports_an_unreachable_service_per_book() {
    let dir = TempDir::new("driver");
    let mut app = App::open(Some(dir.path())).expect("open app");
    let record = app.import(&fixture("minimal.epub")).expect("import");

    // Move and save, so the position owes a push and the sync genuinely
    // has to reach the (dead) service.
    let mut session = app.open_book(record.id).expect("open");
    settle(&mut session);
    session.apply(Action::NextPage);
    session.save_position();
    drop(session);

    let mut library = Library::open(app.dir()).expect("second connection");
    library
        .set_sync_targets(record.id, Some("http://127.0.0.1:1/progression"), None)
        .expect("targets");

    let engine = SyncEngine::new(
        Library::open(app.dir()).expect("engine connection"),
        Arc::new(DeadHttp),
        Device {
            id: "test-device".into(),
            name: "test".into(),
        },
    );
    let driver = SyncDriver::spawn(
        engine,
        Arc::new(chapbook_app::chapbook_reader::chapbook_core::EnvCredentials),
        Arc::new(|| {}),
    );
    assert!(driver.request(SyncRequest::All));

    let first = driver.next_status().expect("a report");
    match first {
        SyncStatus::Book(report) => {
            assert_eq!(report.book, record.id);
            assert!(
                matches!(report.position, PositionReport::Failed(_)),
                "a dead transport fails the position half, got {:?}",
                report.position
            );
        }
        other => panic!("expected a book report, got {other:?}"),
    }
    match driver.next_status().expect("a finish") {
        SyncStatus::Finished { books } => assert_eq!(books, 1),
        other => panic!("expected the batch to finish, got {other:?}"),
    }
}

#[test]
fn removing_a_book_empties_the_shelf() {
    let dir = TempDir::new("remove");
    let mut app = App::open(Some(dir.path())).expect("open app");
    let record = app.import(&fixture("minimal.epub")).expect("import");
    app.remove(record.id).expect("remove");
    assert!(app
        .shelf(&ShelfFilter::default())
        .expect("shelf")
        .is_empty());
    assert!(app.book(record.id).expect("lookup").is_none());
}

#[test]
fn the_state_line_matches_the_shelf() {
    let dir = TempDir::new("state");
    let mut app = App::open(Some(dir.path())).expect("open app");
    let record = app.import(&fixture("minimal.epub")).expect("import");
    assert_eq!(chapbook_app::describe_state(&record), "unread");
    app.set_finished(record.id, true).expect("finish");
    let record = app.book(record.id).expect("lookup").expect("still there");
    assert_eq!(chapbook_app::describe_state(&record), "finished");
}
