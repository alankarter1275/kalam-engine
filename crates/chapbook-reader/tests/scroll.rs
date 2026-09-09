//! kalam: the by-page surface a scrolling shell composes a continuous
//! view from (`scroll.rs`). The shell is not here — no strip, no
//! scrollbar — only what it relies on: extents that glue pages into a
//! flow, frames and hit-tests for pages that are not the session's, and
//! `set_position` telling the session where the reader scrolled to.

mod common;
use chapbook_core::Locator;
use chapbook_reader::{HostHighlight, Session};
use common::*;

fn open_long() -> Session {
    let mut s = open_isolated("scroll", &fixture("epub/long.epub"));
    s.set_metrics(metrics());
    s
}

#[test]
fn extents_describe_every_page_without_moving_the_reader() {
    let mut s = open_long();
    assert_eq!(s.position().spine, 0);
    assert!(!s.is_laid_out(3), "nothing is laid out until asked");

    let count = s.page_count_of(3).expect("chapter 4 lays out");
    assert!(
        count >= 2,
        "the long fixture's chapters run to pages: {count}"
    );
    assert!(s.is_laid_out(3));
    let extents = s.page_extents(3);
    assert_eq!(extents.len(), count);

    let content_h = metrics().content_height();
    for (i, e) in extents.iter().enumerate() {
        assert!(e.used_height > 0.0, "page {i} has content");
        assert!(
            e.used_height <= content_h + 30.0,
            "page {i} used {} of {content_h}",
            e.used_height
        );
        assert!(e.gap_before >= 0.0);
        assert!(e.gap_before <= content_h);
        assert_eq!(e.content.origin.y, 40.0, "the content box is the metrics'");
        assert_eq!(
            Some(*e),
            s.page_extent(3, i),
            "one page or all, same answer"
        );
    }
    assert_eq!(extents[0].gap_before, 0.0, "a chapter starts flush");
    assert!(
        extents
            .windows(2)
            .all(|w| w[0].start_offset <= w[1].start_offset),
        "start offsets are the char map"
    );
    // Every page but the last is filled to within a couple of lines.
    for (i, e) in extents.iter().enumerate().take(count - 1) {
        assert!(
            content_h - e.used_height < 60.0,
            "page {i} of chapter 4 is a full page: used {}",
            e.used_height
        );
    }
    assert_eq!(
        s.position().spine,
        0,
        "asking laid out chapter 4, the reader stayed"
    );
    assert!(s.page_extent(3, count).is_none(), "past the end");
    assert!(s.page_extent(99, 0).is_none(), "no such chapter");

    // A page's start offset is what the session reports standing on it
    // (not 0 for page 0: the markup's leading whitespace comes first).
    assert!(s.set_position(3, 0));
    assert_eq!(s.current_offset(), extents[0].start_offset);
    assert!(s.set_position(3, count - 1));
    assert_eq!(s.current_offset(), extents[count - 1].start_offset);
}

#[test]
fn any_page_renders_and_matches_the_paged_view() {
    let mut s = open_long();
    // What the paged reader paints for chapter 3 page 1 …
    assert!(s.goto(Locator::chapter_start(2)));
    let _ = s.frame(); // a jump lands on the next frame
    assert!(s.next_page());
    let paged = s.render().expect("renders");
    let at = s.position();
    assert_eq!((at.spine, at.page), (2, 1));

    // … is what the scroll surface paints for that page, asked from
    // anywhere.
    assert!(s.goto(Locator::chapter_start(0)));
    let _ = s.frame();
    let scrolled = s.render_page(2, 1).expect("any page renders");
    assert_eq!(scrolled.width(), paged.width());
    assert_eq!(scrolled.height(), paged.height());
    assert_eq!(scrolled.data(), paged.data(), "same pixels either way");
    assert_eq!(
        s.position().spine,
        0,
        "rendering another page moved nothing"
    );

    let list = s.page_frame(2, 1).expect("display list");
    assert!(list.ops.len() > 1, "a ground fill and some text");
    assert!(s.page_frame(2, 999).is_none());
}

#[test]
fn hit_tests_work_on_pages_that_are_not_current() {
    let mut s = open_long();
    let count = s.page_count_of(5).expect("chapter 6 lays out");
    let page = count / 2;
    // Sweep for text on that page, as the paged tests do on the current one.
    let mut hit = None;
    'outer: for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            if let Some(off) = s.offset_at_page(5, page, x as f32, y as f32) {
                hit = Some((x as f32, y as f32, off));
                break 'outer;
            }
        }
    }
    let (x, y, offset) = hit.expect("chapter 6 has text to hit");
    let extent = s.page_extent(5, page).unwrap();
    assert!(offset >= extent.start_offset, "an offset on that page");

    let (start, end) = s
        .word_at_page(5, page, x, y)
        .expect("a word under the point");
    assert!(
        start <= offset && offset < end,
        "the word contains the hit offset"
    );
    let rects = s.range_rects_on_page(5, page, start, end);
    assert!(!rects.is_empty(), "the word has geometry on its page");
    let under = |r: &chapbook_core::Rect| {
        r.min_x() - 2.0 <= x && x <= r.max_x() + 2.0 && r.min_y() - 2.0 <= y && y <= r.max_y() + 2.0
    };
    assert!(rects.iter().any(under), "one of them is under the point");
    assert!(
        s.range_rects_on_page(5, page + 1, start, end).is_empty(),
        "the range is not on the next page"
    );
    // The margin beside a line is not on it.
    assert!(s.word_at_page(5, page, 5.0, y).is_none());
    assert!(
        s.link_at_page(5, page, x, y).is_none(),
        "filler prose has no links"
    );
    assert_eq!(s.position().spine, 0, "hit-testing moved nothing");
}

#[test]
fn set_position_is_the_scroll_shells_page_turn() {
    let mut s = open_long();
    let _ = s.frame();
    assert!(s.set_position(2, 1), "moved");
    let at = s.position();
    assert_eq!((at.spine, at.page), (2, 1));
    let events = s.drain_events();
    let moved = |e: &chapbook_reader::SessionEvent| {
        matches!(
            e,
            chapbook_reader::SessionEvent::PositionChanged { spine: 2, page: 1 }
        )
    };
    assert!(
        events.iter().any(moved),
        "the shell hears the move it made, like any other: {events:?}"
    );
    // Position-derived answers follow.
    let extent = s.page_extent(2, 1).unwrap();
    assert_eq!(s.current_offset(), extent.start_offset);
    assert!(s.unit_fraction() > 0.0);
    let loc = s.layered_locator().expect("captures");
    assert_eq!(loc.spine_index, 2);
    assert_eq!(loc.char_offset, extent.start_offset);

    assert!(!s.set_position(2, 1), "same place is not a move");
    assert!(s.set_position(2, 999), "clamped to the last page");
    assert_eq!(s.position().page, s.page_count() - 1);
    assert!(!s.set_position(99, 0), "no such chapter");
    assert_eq!(s.position().spine, 2);

    // A jump's target, asked as a page, then scrolled to by the shell.
    let target = s.layered_locator().unwrap();
    assert!(s.set_position(0, 0));
    let page = s
        .page_of(Locator::new(2, target.char_offset))
        .expect("lays out");
    assert_eq!(page, s.page_count_of(2).unwrap() - 1);
    assert_eq!(
        s.page_of_anchor(2, "no-such-id"),
        Some(0),
        "unknown fragment: page 0"
    );
}

#[test]
fn a_selection_lives_on_the_page_it_was_made_on() {
    let mut s = open_long();
    let _ = s.frame();
    // Find a point *on* text on chapter 4 page 0 without making it
    // current first — the tap rule, so the tap at the end lands too.
    let mut hit = None;
    'outer: for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            if s.word_at_page(3, 0, x as f32, y as f32).is_some() {
                hit = Some((x as f32, y as f32));
                break 'outer;
            }
        }
    }
    let (x, y) = hit.expect("text on chapter 4 page 0");
    assert!(s.selection_begin_on_page(3, 0, x, y), "press hits text");
    assert_eq!(
        s.position().spine,
        3,
        "a selection moves the reader to its unit"
    );
    s.selection_drag_on_page(3, 0, x + 200.0, y);
    let (start, end) = s.selected_range().expect("non-empty");
    assert!(end > start);
    let text = s.selected_text().expect("text");
    assert!(!text.is_empty());

    // The selection paints on its page through the scroll surface …
    let plain_before = {
        s.selection_clear();
        let px = s.render_page(3, 0).unwrap();
        s.select_range(start, end);
        px
    };
    let with_selection = s.render_page(3, 0).unwrap();
    assert_ne!(
        plain_before.data(),
        with_selection.data(),
        "selection paints"
    );
    // … and not on a page of another chapter.
    let other_before = {
        s.selection_clear();
        let px = s.render_page(4, 0).unwrap();
        s.select_range(start, end);
        px
    };
    assert_eq!(other_before.data(), s.render_page(4, 0).unwrap().data());

    // A drag into another chapter is ignored, not applied.
    s.selection_drag_on_page(4, 0, x, y);
    assert_eq!(s.selected_range(), Some((start, end)));

    // Scrolling within the unit keeps the selection; leaving it clears.
    s.set_position(3, 1);
    assert_eq!(s.selected_range(), Some((start, end)));
    s.set_position(4, 0);
    assert_eq!(s.selected_range(), None);

    // A host highlight made from that selection is found by a tap on its
    // page, from anywhere.
    let row = HostHighlight {
        id: 7,
        start: {
            s.set_position(3, 0);
            s.layered_locator_at(start).unwrap()
        },
        end: s.layered_locator_at(end).unwrap(),
        color: None,
        text: Some(text),
    };
    s.show_host_highlight(row);
    s.set_position(0, 0);
    assert_eq!(s.host_highlight_at_page(3, 0, x, y), Some(7));
    assert_eq!(s.host_highlight_at_page(4, 0, x, y), None);
}

#[test]
fn settle_lands_a_jump_without_a_frame() {
    let mut s = open_long();
    // A restored position, the way Kalam hands one back.
    let stored = {
        s.set_position(4, 1);
        s.layered_locator().expect("captures")
    };
    let mut s = open_long();
    assert!(s.goto_layered(&stored, true));
    // The paged loop would land this on the next frame(); a scroll shell
    // never takes one, so without settle() the page is still the unit's
    // first.
    assert_eq!(s.position().spine, 4);
    let landed = s.settle();
    assert_eq!((landed.spine, landed.page), (4, 1), "settle lands the jump");
    assert_eq!(s.position(), landed);
    assert_eq!(s.settle(), landed, "nothing pending is a no-op");

    // A TOC fragment jump lands the same way.
    let mut s = open_isolated("scroll-toc", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    // The nested entry, as `navigation.rs` reads it.
    let entry = s.toc()[1].children[0].clone();
    assert_eq!(entry.fragment.as_deref(), Some("part2"));
    assert!(s.goto_toc(&entry));
    let landed = s.settle();
    assert_eq!(landed.spine, 1);
    assert_eq!(
        Some(landed.page),
        s.page_of_anchor(1, "part2"),
        "settled where page_of_anchor said it would"
    );
}

#[test]
fn the_layout_generation_moves_when_layouts_are_dropped() {
    let mut s = open_long();
    let g0 = s.layout_generation();
    s.page_extents(2);
    s.page_extents(3);
    assert_eq!(s.layout_generation(), g0, "measuring changes nothing");
    s.set_position(3, 1);
    assert_eq!(s.layout_generation(), g0, "nor does scrolling");

    s.adjust_font(2.0);
    let g1 = s.layout_generation();
    assert!(g1 > g0, "a font change drops every layout");
    assert!(!s.is_laid_out(2), "and the old extents are gone with them");

    let mut m = metrics();
    m.size.h += 100.0;
    s.set_metrics(m);
    assert!(s.layout_generation() > g1, "so does a resize");
    let g2 = s.layout_generation();

    s.set_metrics(m);
    assert_eq!(s.layout_generation(), g2, "the same metrics again: nothing");
    s.release_caches();
    assert!(s.layout_generation() > g2, "and a memory-pressure release");
}

#[test]
fn pinned_units_survive_a_budget_that_would_evict_them() {
    use chapbook_reader::SessionConfig;
    // A budget that holds roughly one laid-out chapter of the long
    // fixture, so a second one is normally evicted when a third arrives.
    let one_chapter = {
        let mut s = open_long();
        s.page_extents(3);
        s.cache_bytes()
    };
    let budget = one_chapter + one_chapter / 2;
    let open = || {
        let mut s = Session::open_with(
            fixture("epub/long.epub"),
            SessionConfig::new(fixture_fonts()).with_cache_budget(budget),
        )
        .unwrap();
        s.set_metrics(metrics());
        s
    };

    // Unpinned: the shell measures the chapter below the current one,
    // then something else lays out, and the neighbour is gone.
    let mut s = open();
    s.set_position(3, 0);
    s.page_extents(4);
    s.page_extents(5);
    assert!(
        !s.is_laid_out(4),
        "control: without pinning, the neighbour was evicted"
    );

    // Pinned as the visible band, the same sequence keeps it.
    let mut s = open();
    s.set_position(3, 0);
    s.pin_units(3..5);
    s.page_extents(4);
    s.page_extents(5);
    assert!(s.is_laid_out(4), "pinned, the neighbour survives");
    assert!(
        s.cache_bytes() > budget,
        "pinning is a floor: the budget was exceeded rather than the band broken"
    );

    // Unpinning lets the next eviction take it.
    s.pin_units(0..0);
    s.page_extents(6);
    assert!(!s.is_laid_out(4), "unpinned, it goes like any other");
}

#[test]
fn the_reading_line_names_a_line_and_finds_it_again() {
    let mut s = open_long();
    let extent = s.page_extent(2, 1).expect("chapter 3 has a page 1");
    let top = extent.content.origin.y;

    // On a line box, line_at_page is the line; between two, it is the
    // next one down; past the last, the last. Walking the page top to
    // bottom the answers never go backwards and start at the page's
    // first line.
    let first = s.line_at_page(2, 1, top).expect("a line at the top");
    assert_eq!(
        first, extent.start_offset,
        "the first line is where the page starts"
    );
    let mut last = first;
    let mut y = top;
    while y < top + extent.used_height + 50.0 {
        let here = s.line_at_page(2, 1, y).expect("always a line");
        assert!(here >= last, "monotonic at y={y}: {here} < {last}");
        last = here;
        y += 7.0;
    }
    assert!(last > first, "the page has more than one line");

    // The inverse: the rect of the line holding an offset contains the
    // y that named it, and is the box offset_at_page agrees with.
    let mid_y = top + extent.used_height / 2.0;
    let offset = s.line_at_page(2, 1, mid_y).unwrap();
    let rect = s.line_rect_at_page(2, 1, offset).expect("that line's box");
    assert!(
        rect.origin.y <= mid_y + rect.size.h && rect.max_y() >= mid_y - rect.size.h,
        "the line's box {rect:?} is about y={mid_y}"
    );
    let on_line = s
        .offset_at_page(2, 1, rect.origin.x + 1.0, rect.origin.y + rect.size.h / 2.0)
        .expect("text on the line");
    assert!(
        on_line >= offset,
        "a point on the line is at or after the line's start"
    );
    // An offset before the page's first line has no line here.
    assert_eq!(s.line_rect_at_page(2, 1, first.saturating_sub(1)), None);
    assert_eq!(s.position().spine, 0, "nothing here moved the reader");
}

#[test]
fn every_chapters_length_is_counted_once() {
    let s = open_long();
    let counts = s.chapter_char_counts().to_vec();
    assert_eq!(counts.len(), s.spine_len());
    assert!(counts.iter().all(|&n| n > 0), "every chapter has text");
    for (spine, &count) in counts.iter().enumerate() {
        assert_eq!(s.chapter_char_count(spine), Some(count));
    }
    let total: u64 = counts.iter().sum();
    // The same pass feeds the layered locator's whole-book progression.
    let l = s.layered_locator().expect("a locator at the start");
    assert!(l.book_progression < 0.01, "at the start of {total} chars");
}
