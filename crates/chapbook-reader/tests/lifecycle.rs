//! Memory-pressure release and the suspend lifecycle.

mod common;
use chapbook_reader::{Session, SessionConfig};
use common::*;

/// The two calls a platform makes when it is telling you something:
/// "memory is short" and "you are about to be stopped".
mod lifecycle {
    use super::*;

    const PAGE: usize = 120 * 180 * 4;
    /// A retained unit is its decoded image *and* its laid-out page, so
    /// "one unit" is a little over one page's worth of pixels.
    const ONE_UNIT: usize = PAGE + 8 * 1024;

    fn dir_for(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chapbook-lifecycle-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn comic(name: &str) -> Session {
        let mut session = Session::open_with(
            fixture("cbz/minimal.cbz"),
            SessionConfig::new(fixture_fonts()).with_library_dir(dir_for(name)),
        )
        .unwrap();
        session.set_metrics(chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(0.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        });
        session
    }

    #[test]
    fn release_caches_keeps_the_page_on_screen_and_drops_the_rest() {
        let mut session = comic("release");
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            session.next_unit();
        }
        session.prev_unit();
        render_loaded(&mut session);
        let before = session.cache_bytes();
        assert!(before > PAGE, "more than one page is held: {before}");

        session.release_caches();
        assert!(
            session.cache_bytes() <= ONE_UNIT,
            "only the current unit should survive, held {}",
            session.cache_bytes()
        );
        // And the reader still has something to show, immediately.
        assert!(session.render().is_some());
    }

    #[test]
    fn what_release_drops_comes_back_identical() {
        let mut session = comic("release-refill");
        let first = render_loaded(&mut session).data().to_vec();
        session.next_unit();
        render_loaded(&mut session);

        session.release_caches();
        session.goto(chapbook_core::Locator {
            spine_index: 0,
            char_offset: 0,
        });
        assert_eq!(render_loaded(&mut session).data().to_vec(), first);
    }

    #[test]
    fn suspend_persists_the_position_and_lets_go_of_the_database() {
        let dir = dir_for("suspend");
        let source = fixture("epub/minimal.epub");
        let config = || SessionConfig::new(fixture_fonts()).with_library_dir(&dir);

        let mut session = Session::open_with(source.as_str(), config()).unwrap();
        session.set_metrics(chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(20.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        });
        session.next_unit();
        session.render();
        let where_we_were = session.locator();
        session.suspend();

        // Another session opening the same library sees the position,
        // which is only possible if suspend wrote it *and* released the
        // lock on the way out.
        let reopened = Session::open_with(source.as_str(), config()).unwrap();
        assert_eq!(reopened.locator().spine_index, where_we_were.spine_index);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_suspended_session_keeps_working() {
        // `onStop` is often followed by `onStart` with the process still
        // alive. A session that stopped saving after the first suspend
        // would lose every position from then on, silently.
        let mut session = comic("suspend-resume");
        render_loaded(&mut session);
        session.suspend();

        session.next_unit();
        assert!(render_loaded(&mut session).width() > 0, "still renders");
        session.save_position();
        session.suspend();
        assert!(session.render().is_some(), "and survives a second one");
    }

    #[test]
    fn suspending_gives_the_caches_back_too() {
        let mut session = comic("suspend-caches");
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            session.next_unit();
        }
        session.prev_unit();
        render_loaded(&mut session);
        assert!(session.cache_bytes() > PAGE);

        session.suspend();
        assert!(
            session.cache_bytes() <= ONE_UNIT,
            "a stopped app should not hold decoded pages: {}",
            session.cache_bytes()
        );
    }
}
