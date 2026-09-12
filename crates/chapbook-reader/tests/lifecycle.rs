//! Memory-pressure release and the suspend lifecycle.

mod common;
use chapbook_reader::{Session, SessionConfig};
use common::*;

/// The two calls a platform makes when it is telling you something:
/// "memory is short" and "you are about to be stopped".
mod lifecycle {
    use super::*;

    #[cfg(feature = "cbz")]
    const PAGE: usize = 120 * 180 * 4;
    /// A retained unit is its decoded image *and* its laid-out page, so
    /// "one unit" is a little over one page's worth of pixels.
    #[cfg(feature = "cbz")]
    const ONE_UNIT: usize = PAGE + 8 * 1024;

    #[cfg(feature = "cbz")]
    fn comic(_name: &str) -> Session {
        let mut session = Session::open_with(
            fixture("cbz/minimal.cbz"),
            SessionConfig::new(fixture_fonts()),
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
    #[cfg(feature = "cbz")]
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
    #[cfg(feature = "cbz")]
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
    fn suspend_keeps_the_place_and_the_session_stays_usable() {
        // kalam: upstream's suspend wrote the position to its library and
        // closed the database; a second session proved both. There is no
        // database now, so what suspend owes is narrower: the caches go,
        // the place does not, and the host can still ask for the durable
        // locator it stores itself.
        let source = fixture("epub/minimal.epub");
        let mut session =
            Session::open_with(source.as_str(), SessionConfig::new(fixture_fonts())).unwrap();
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

        assert_eq!(
            session.locator(),
            where_we_were,
            "suspend does not move the reader"
        );
        let durable = session.layered_locator().expect("a text unit captures");
        assert_eq!(durable.spine_index, where_we_were.spine_index);
        assert!(session.render().is_some(), "and it still renders");
    }

    #[test]
    #[cfg(feature = "cbz")]
    fn a_suspended_session_keeps_working() {
        // `onStop` is often followed by `onStart` with the process still
        // alive. A session that stopped saving after the first suspend
        // would lose every position from then on, silently.
        let mut session = comic("suspend-resume");
        render_loaded(&mut session);
        session.suspend();

        session.next_unit();
        assert!(render_loaded(&mut session).width() > 0, "still renders");
        session.suspend();
        assert!(session.render().is_some(), "and survives a second one");
    }

    #[test]
    #[cfg(feature = "cbz")]
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
