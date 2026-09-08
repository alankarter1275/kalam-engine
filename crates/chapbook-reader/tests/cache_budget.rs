//! The cache budget and LRU eviction.

mod common;
use chapbook_reader::{Session, SessionConfig};
use common::*;

/// The caches had no ceiling: `layouts` and `images` were cleared only
/// wholesale, and for an image book not even then. Measured, reading forty
/// 1600x2400 comic pages forward took a process to 676 MB and no API could
/// give any of it back. These pin the ceiling that fixes it.
///
/// The CBZ fixture decodes to 86,400 bytes a page, which is what the
/// budgets below are counted in.
mod cache_budget {
    use super::*;

    /// One page of the comic fixture, decoded.
    #[cfg(feature = "cbz")]
    const PAGE: usize = 120 * 180 * 4;

    #[cfg(feature = "cbz")]
    fn comic(name: &str, budget: usize) -> Session {
        let dir = std::env::temp_dir().join(format!(
            "chapbook-budget-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut session = Session::open_with(
            fixture("cbz/minimal.cbz"),
            SessionConfig::new(fixture_fonts())
                .with_library_dir(&dir)
                .with_cache_budget(budget),
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
    fn the_default_applies_when_the_host_says_nothing() {
        let session = open_isolated("budget-default", &fixture("epub/minimal.epub"));
        assert_eq!(
            session.cache_budget(),
            chapbook_reader::DEFAULT_CACHE_BUDGET
        );
    }

    #[test]
    #[cfg(feature = "cbz")]
    fn paging_a_comic_forward_stays_under_the_budget() {
        // Room for two pages, not three.
        let budget = PAGE * 2 + PAGE / 2;
        let mut session = comic("forward", budget);
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            assert!(
                session.cache_bytes() <= budget,
                "cache {} over budget {budget}",
                session.cache_bytes()
            );
            session.next_unit();
        }
    }

    #[test]
    #[cfg(feature = "cbz")]
    fn the_unit_being_read_is_never_evicted() {
        // A budget nothing can fit. One page over is better than a reader
        // with nothing on screen.
        let mut session = comic("pinned", 1);
        let pixmap = render_loaded(&mut session);
        assert!(session.cache_bytes() > 0, "the current page survived");
        assert!(pixmap.width() > 0);
    }

    #[test]
    #[cfg(feature = "cbz")]
    fn an_evicted_page_comes_back_the_same() {
        // Eviction is only safe because nothing cached is authoritative: a
        // comic page is re-read from the archive and decoded again. This is
        // that claim, checked.
        let mut session = comic("revisit", PAGE + PAGE / 2);
        let first = render_loaded(&mut session).data().to_vec();

        session.next_unit();
        render_loaded(&mut session);
        session.next_unit();
        render_loaded(&mut session);

        session.goto(chapbook_core::Locator {
            spine_index: 0,
            char_offset: 0,
        });
        let again = render_loaded(&mut session).data().to_vec();
        assert_eq!(first, again, "a re-decoded page is the same page");
    }

    #[test]
    #[cfg(feature = "cbz")]
    fn lowering_the_budget_evicts_immediately() {
        // What a host does on a memory warning: it does not get to wait
        // for the next page turn.
        let mut session = comic("lowered", PAGE * 8);
        for _ in 0..session.spine_len() {
            render_loaded(&mut session);
            session.next_unit();
        }
        assert!(session.cache_bytes() > PAGE, "several pages are held");

        session.set_cache_budget(PAGE + PAGE / 2);
        assert!(
            session.cache_bytes() <= PAGE + PAGE / 2,
            "cache {} did not shrink on the spot",
            session.cache_bytes()
        );
    }

    #[test]
    fn laid_out_chapters_count_against_it_too() {
        // Text is the smaller term — 0.3 MB a chapter against 15 MB a comic
        // page — but it accumulates the same way, so one budget covers
        // both rather than leaving a second unbounded cache behind.
        let dir =
            std::env::temp_dir().join(format!("chapbook-budget-test-{}-text", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let budget = 64 * 1024;
        let mut session = Session::open_with(
            fixture("epub/minimal.epub"),
            SessionConfig::new(fixture_fonts())
                .with_library_dir(&dir)
                .with_cache_budget(budget),
        )
        .unwrap();
        session.set_metrics(chapbook_core::PageMetrics {
            size: chapbook_core::Size::new(400.0, 600.0),
            margins: chapbook_core::EdgeSizes::uniform(20.0),
            dpi_scale: 1.0,
            rotation: chapbook_core::Rotation::None,
        });
        for _ in 0..session.spine_len() {
            session.render().expect("render");
            session.next_unit();
        }
        // The pinned unit may exceed it alone; nothing else may accumulate.
        assert!(
            session.cache_bytes() <= budget.max(PAGE),
            "text layouts accumulated to {}",
            session.cache_bytes()
        );
    }
}
