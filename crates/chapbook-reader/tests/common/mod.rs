//! Shared fixtures and helpers for the session integration tests. Each
//! test binary uses its own subset, hence the module-wide dead_code
//! allow.
#![allow(dead_code)]

use std::path::PathBuf;

use chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::{Session, SessionConfig};

pub fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// Open a session against a per-test library dir.
///
/// This used to set `CHAPBOOK_LIBRARY_DIR` behind a mutex, because the only
/// way to place a library was a process-global variable and these tests run
/// in parallel. `SessionConfig::with_library_dir` is an argument, so the
/// lock is gone and so is the serialization.
pub fn open_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, true)
}

/// Reopen against the same per-test library (position-persistence tests).
/// The vendored fixture faces, all three axes pinned, so this suite means
/// the same thing on Linux, on a Mac and on a device. Taking the host's
/// fonts is what pinned two of these assertions to one machine's
/// collection.
pub fn fixture_fonts() -> chapbook_core::FontSource {
    chapbook_core::FontSource::embedded(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts"),
        "Crimson Text",
    )
}

pub fn reopen_isolated(name: &str, source: &str) -> Session {
    open_library(name, source, false)
}

pub fn open_library(name: &str, source: &str, fresh: bool) -> Session {
    let dir = library_dir(name);
    if fresh {
        let _ = std::fs::remove_dir_all(&dir);
    }
    Session::open_with(
        source,
        SessionConfig::new(fixture_fonts()).with_library_dir(dir),
    )
    .unwrap()
}

/// A library dir of this test's own, stable across a reopen.
pub fn library_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "chapbook-session-test-{}-{name}",
        std::process::id()
    ))
}

/// Drive the async load path to completion: render (queues the load),
/// then poll until the unit lands. Panics after ~5s.
pub fn render_loaded(s: &mut Session) -> chapbook_reader::tiny_skia::Pixmap {
    for _ in 0..200 {
        s.render().expect("render");
        if !s.has_pending_loads() {
            // One more poll+render in case the last result just arrived.
            s.poll_loaded();
            return s.render().expect("render");
        }
        s.poll_loaded();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("unit never finished loading");
}

/// Anchor a selection on the first hit-testable text on the current page.
/// The placed image rect depends on the page's natural size and the
/// margins, so sweep for a hit rather than hardcoding coordinates.
pub fn sweep_for_text(s: &mut Session) -> (f32, f32) {
    for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            if s.selection_begin(x as f32, y as f32) {
                return (x as f32, y as f32);
            }
        }
    }
    panic!("no hit-testable text on this page");
}

/// Find a point on the current page that sits inside a hyperlink. Where
/// the link lands depends on the fixture font and page size, so sweep.
pub fn sweep_for_link(s: &mut Session) -> Option<(f32, f32, String)> {
    for y in (40..760).step_by(4) {
        for x in (40..560).step_by(4) {
            if let Some(href) = s.link_at(x as f32, y as f32) {
                return Some((x as f32, y as f32, href));
            }
        }
    }
    None
}

pub fn metrics() -> PageMetrics {
    PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
        rotation: Rotation::None,
    }
}

/// The one locator range where `needle` occurs in the open unit.
///
/// Offsets come from search rather than being written down. `search_unit`
/// and `select_range` share the unit's locator space, and neither has any
/// idea which fonts laid the page out — so a range obtained this way means
/// the same thing on every host, which a hand-written offset does not.
pub fn only_hit(s: &mut Session, needle: &str) -> (u32, u32) {
    let spine = s.spine();
    let hits = s.search_unit(spine, needle);
    assert_eq!(hits.len(), 1, "{needle:?} should occur exactly once");
    (hits[0].locator.char_offset, hits[0].end)
}
