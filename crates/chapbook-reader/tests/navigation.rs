//! Links, the table of contents, the back trail, and search.

mod common;
use common::*;

#[test]
fn links_and_the_toc_both_navigate_and_the_trail_comes_back() {
    let mut s = open_isolated("epub-nav", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);

    let (lx, ly, href) = sweep_for_link(&mut s).expect("chapter one links to chapter two");
    assert_eq!(href, "chapter2.xhtml");
    // A tap in the margin beside the link is not a tap on the link.
    assert_eq!(s.link_at(2.0, ly), None);
    assert!(s.link_at(lx, ly).is_some());

    assert!(s.follow_link(&href));
    assert_eq!(s.spine(), 1, "the link crossed to chapter two");
    assert!(s.can_go_back());
    assert!(s.back());
    assert_eq!(s.spine(), 0, "and back again");
    assert!(!s.can_go_back(), "the trail is spent");

    // The TOC's nested entry carries a fragment.
    let entry = s.toc()[1].children[0].clone();
    assert_eq!(entry.fragment.as_deref(), Some("part2"));
    assert!(s.goto_toc(&entry));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 1);
    let anchor_page = s.page();

    // A fragment the unit doesn't have lands at its start rather than
    // failing outright.
    assert!(s.goto_anchor(1, "not-in-this-chapter"));
    s.render().expect("page renders");
    assert_eq!(s.page(), 0);
    assert!(anchor_page >= s.page());
}

#[test]
fn external_links_are_not_a_reading_position() {
    let mut s = open_isolated("epub-nav-external", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert!(!s.follow_link("https://example.com/"));
    assert!(!s.follow_link("mailto:nobody@example.com"));
    assert!(!s.follow_link("chapter9.xhtml"), "no such unit");
    assert_eq!(s.spine(), 0);
    assert!(!s.can_go_back(), "a refused link leaves no trail");
}

#[test]
fn a_jump_remembers_the_offset_it_left() {
    let mut s = open_isolated("epub-nav-offset", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    // Read into chapter two, past its forced page break, so the position
    // being remembered is an offset and not just a unit.
    assert!(s.goto(chapbook_core::Locator::chapter_start(1)));
    s.render().expect("page renders");
    s.next_page();
    s.render().expect("page renders");
    let before = s.locator();
    assert_eq!(before.spine_index, 1);
    assert!(before.char_offset > 0, "reading past the first page");

    assert!(s.goto(chapbook_core::Locator::chapter_start(0)));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);

    assert!(s.back());
    s.render().expect("page renders");
    assert_eq!(s.locator().spine_index, before.spine_index);
    assert_eq!(
        s.locator().char_offset,
        before.char_offset,
        "back lands on the offset it left, not the chapter start"
    );
}

#[test]
fn search_finds_hits_that_navigate_and_select() {
    let mut s = open_isolated("epub-search", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let hits = s.search("universally acknowledged", 10);
    assert_eq!(hits.len(), 1, "one hit in chapter one: {hits:?}");
    let hit = hits[0].clone();
    assert_eq!(hit.locator.spine_index, 0);
    assert_eq!(hit.end - hit.locator.char_offset, 24);

    // The context is list-ready: the raw locator text\'s newlines and
    // XHTML indentation are collapsed out of it.
    assert!(!hit.context.contains('\n'));
    assert!(!hit.context.contains("  "));
    let matched: String = hit
        .context
        .chars()
        .skip(hit.match_range.0 as usize)
        .take((hit.match_range.1 - hit.match_range.0) as usize)
        .collect();
    assert_eq!(matched, "universally acknowledged", "{:?}", hit.context);

    // A hit is a locator: it navigates, and selecting its range on the
    // page it lands on reads back the words that were searched for.
    assert!(s.goto(hit.locator));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);
    s.select_range(hit.locator.char_offset, hit.end);
    assert_eq!(
        s.selected_text().as_deref(),
        Some("universally acknowledged")
    );
}

#[test]
fn search_is_case_insensitive_and_spans_the_spine() {
    let mut s = open_isolated("epub-search-case", &fixture("epub/minimal.epub"));
    s.set_metrics(metrics());

    let hits = s.search("CHAPTER", 50);
    assert!(hits.len() >= 2, "chapters one and two both match: {hits:?}");
    assert!(
        hits.iter().any(|h| h.locator.spine_index == 0)
            && hits.iter().any(|h| h.locator.spine_index == 1),
        "hits come from both units"
    );
    // Hits arrive in reading order.
    let mut ordered = hits.clone();
    ordered.sort_by_key(|h| (h.locator.spine_index, h.locator.char_offset));
    assert_eq!(hits, ordered);

    assert_eq!(s.search("chapter", 2).len(), 2, "the limit is honored");
    assert!(
        s.search("", 10).is_empty(),
        "an empty query matches nothing"
    );
    assert!(s.search("no such phrase anywhere", 10).is_empty());
}

#[test]
#[cfg(feature = "cbz")]
fn comics_have_nothing_to_search() {
    let mut s = open_isolated("cbz-search", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert!(s.search("anything", 10).is_empty());
}

#[test]
#[cfg(feature = "pdf")]
fn a_pdf_page_becomes_searchable_once_it_loads() {
    let mut s = open_isolated("pdf-search", &fixture("pdf/minimal.pdf"));
    s.set_metrics(metrics());
    // Page three carries the text; nothing is searchable before it loads.
    assert!(s.search_unit(2, "Hello").is_empty());

    s.next_unit();
    s.next_unit();
    render_loaded(&mut s);
    let hits = s.search_unit(2, "hello");
    assert_eq!(hits.len(), 1, "the text layer is searchable: {hits:?}");
    assert_eq!(hits[0].locator.spine_index, 2);
    assert!(hits[0].context.starts_with("Hello"));
}
