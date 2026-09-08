//! Positions: persistence across sessions, and restore staying in the
//! unit it was captured in.

mod common;
use common::*;

#[test]
fn cbz_position_persists_across_sessions() {
    let source = fixture("cbz/minimal.cbz");
    {
        let mut s = open_isolated("cbz-persist", &source);
        s.set_metrics(metrics());
        s.next_page();
        s.next_page();
        assert_eq!(s.spine(), 2);
        s.save_position();
    }
    let mut s = reopen_isolated("cbz-persist", &source);
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert_eq!(s.spine(), 2, "comic position restores by page progression");
}

/// A restored position lands in `frame()`, because an offset cannot become
/// a page until the unit has laid out. Nothing obliges a shell to paint
/// before it navigates, though — a batched turn, or the conformance
/// harness, walks with no frame in between — and the restore was then
/// applied to whatever unit the reader had reached by the time one
/// arrived, resolving one chapter's offset against another chapter's
/// pages and moving the reader without being asked.
///
/// Found by the Android spike, but nothing about it is Android: running
/// `examples/conform` twice against the same library failed the second
/// time, because the first run left a position for the second to restore.
/// "The end of the book stands still" was the check that broke — a
/// `frame()` there moved the reader backwards, and the turn that had just
/// refused then worked.
///
/// Swept over stopping points rather than aimed at one, because whether a
/// misapplied offset is *visible* depends on where it happens to land: an
/// offset from unit 4 resolved against unit 9 sometimes names the page the
/// reader was already on. The bug is the same either way, so the test
/// asks the invariant at every stop instead of picking a lucky one.
///
/// Reads `long.epub` because the invariant needs units to be wrong about:
/// eight of them, four to five pages each. It read a corpus book until
/// that turned out to mean the test only ran for whoever had run
/// `fixtures/fetch-corpus.sh` — which was nobody in CI.
#[test]
fn a_restore_does_not_follow_the_reader_into_another_unit() {
    let source = fixture("epub/long.epub");
    let saved = {
        let mut s = open_isolated("restore-follows", &source);
        s.set_metrics(metrics());
        for _ in 0..6 {
            s.next_page();
            let _ = s.frame();
        }
        let at = s.position();
        assert!(at.spine > 0 || at.page > 0, "moved off the first page");
        s.save_position();
        at
    };

    // One session per stopping point: the pending restore is consumed by
    // the first frame, so each session affords exactly one observation.
    for turns in 1..40 {
        let mut s = reopen_isolated("restore-follows", &source);
        s.set_metrics(metrics());
        assert_eq!(s.spine(), saved.spine, "reopened in the restored unit");

        // Walk without ever painting, so the restore is still pending.
        for _ in 0..turns {
            if !s.next_page() {
                break;
            }
        }
        let before = s.position();
        if before.spine == saved.spine {
            continue; // still in the restored unit: landing there is correct
        }

        let _ = s.frame();
        assert_eq!(
            s.position(),
            before,
            "after {turns} turns, a pending restore from unit {} moved the \
             reader inside unit {}",
            saved.spine,
            before.spine
        );
    }
}

/// What `long.epub` is for, stated as a test.
///
/// The restore sweep above needs a book it can walk out of: several units,
/// each several pages, and enough pages in total that walking past the end
/// is reachable. That is a property of a generated fixture, so it can drift
/// silently when somebody retunes `fixtures/epub/build-long.py` — and the
/// sweep would not fail, it would just stop covering anything, because
/// every stop would land in the unit it started in and `continue`.
#[test]
fn the_long_book_is_long_enough_to_walk_out_of() {
    let mut s = open_isolated("long-shape", &fixture("epub/long.epub"));
    s.set_metrics(metrics());

    let mut pages_in = vec![0usize; s.spine_len()];
    let mut total = 0usize;
    loop {
        let at = s.position();
        pages_in[at.spine] = at.page + 1;
        total += 1;
        assert!(total < 500, "walk did not terminate");
        if !s.next_page() {
            break;
        }
        let _ = s.frame();
    }

    assert!(
        s.spine_len() >= 6,
        "units to be wrong about: {}",
        s.spine_len()
    );
    assert!(
        pages_in.iter().all(|&p| p >= 2),
        "every unit must span pages, or a turn always changes unit: {pages_in:?}"
    );
    // The sweep walks up to 39 turns and wants the last of them to pile up
    // against the end of the book, which is where a misapplied offset was
    // visible as the end moving.
    assert!(
        (12..39).contains(&total),
        "a book the sweep can both cross and overrun: {total} pages"
    );
}
