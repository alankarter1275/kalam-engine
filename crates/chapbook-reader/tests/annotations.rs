//! Stored annotations over the fixture books: highlights, notes and
//! bookmarks persisting, resolving, and answering taps.

mod common;
use common::*;

#[test]
fn epub_highlight_persists_across_sessions() {
    let source = fixture("epub/illustrated.epub");
    let (start, end, text) = {
        let mut s = open_isolated("epub-highlight", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");
        assert!(s.selection_begin(100.0, 70.0));
        s.selection_drag(400.0, 140.0);
        let (start, end) = s.selected_range().expect("non-empty selection");
        let text = s.selected_text().expect("selection carries text");
        let id = s.add_highlight().expect("highlight is stored");
        assert!(id > 0);
        assert_eq!(s.highlights(0).len(), 1, "visible without a reload");
        (start, end, text)
    };

    let mut s = reopen_isolated("epub-highlight", &source);
    s.set_metrics(metrics());
    let stored = s.highlights(0).to_vec();
    assert_eq!(stored.len(), 1, "highlight survives the session");
    assert_eq!(
        (stored[0].start, stored[0].end),
        (start, end),
        "same edition resolves the exact offsets"
    );
    assert_eq!(stored[0].text.as_deref(), Some(text.as_str()));

    let painted = s.render().expect("page renders");
    // Locator space doesn't move under relayout, so the highlight is
    // unchanged at a different font size.
    s.adjust_font(4.0);
    assert_eq!(s.highlights(0), &stored[..]);
    s.adjust_font(-4.0);

    s.remove_annotation(stored[0].id);
    assert!(s.highlights(0).is_empty(), "delete clears the cache too");
    let plain = s.render().expect("page renders");
    assert_ne!(painted.data(), plain.data(), "the highlight was painted");

    let mut s = reopen_isolated("epub-highlight", &source);
    s.set_metrics(metrics());
    assert!(s.highlights(0).is_empty(), "delete survives the session");
}

#[test]
fn pdf_highlight_resolves_against_the_page_text_layer() {
    let source = fixture("pdf/minimal.pdf");
    let (start, end) = {
        let mut s = open_isolated("pdf-highlight", &source);
        s.set_metrics(metrics());
        // Page 3 carries the text lines.
        s.next_page();
        s.next_page();
        render_loaded(&mut s);
        let (ax, ay) = sweep_for_text(&mut s);
        s.selection_drag(ax + 150.0, ay);
        let range = s.selected_range().expect("selection over PDF text");
        assert!(s.add_highlight().expect("PDF highlight is stored") > 0);
        range
    };

    let mut s = reopen_isolated("pdf-highlight", &source);
    s.set_metrics(metrics());
    s.next_page();
    s.next_page();
    assert_eq!(s.spine(), 2);
    // The page's text layer is what the endpoints resolve against, so
    // nothing resolves until the page has loaded.
    assert!(
        s.highlights(2).is_empty(),
        "unresolved before the page loads"
    );
    render_loaded(&mut s);
    let stored = s.highlights(2).to_vec();
    assert_eq!(stored.len(), 1);
    assert_eq!((stored[0].start, stored[0].end), (start, end));
}

#[test]
fn comic_pages_take_no_highlights() {
    let mut s = open_isolated("cbz-highlight", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    assert_eq!(s.add_highlight(), None, "no text layer to anchor to");
    assert!(s.highlights(0).is_empty());
}

#[test]
fn taps_find_highlights_recolor_them_and_list_every_mark() {
    use chapbook_library::AnnotationKind;

    let source = fixture("epub/illustrated.epub");
    let (id, point) = {
        let mut s = open_isolated("epub-annotations", &source);
        s.set_metrics(metrics());
        s.render().expect("page renders");

        assert!(s.selection_begin(100.0, 70.0), "press must hit the heading");
        s.selection_drag(400.0, 70.0);
        let id = s.add_highlight().expect("highlight is stored");
        s.selection_clear();

        // A tap inside the marked words finds it; the margin beside them
        // does not.
        assert_eq!(s.highlight_at(110.0, 70.0), Some(id));
        assert_eq!(s.highlight_at(2.0, 70.0), None);
        (id, (110.0, 70.0))
    };

    let mut s = reopen_isolated("epub-annotations", &source);
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert_eq!(
        s.highlight_at(point.0, point.1),
        Some(id),
        "found after a reload"
    );

    // Recoloring paints differently and survives the session.
    let themed = s.render().expect("page renders").data().to_vec();
    s.set_highlight_color(id, Some("#ff0000"));
    assert_ne!(
        s.render().expect("page renders").data(),
        &themed[..],
        "the stored color reached the page"
    );
    drop(s);

    let mut s = reopen_isolated("epub-annotations", &source);
    s.set_metrics(metrics());
    s.render().expect("page renders");
    assert_eq!(
        s.highlights(0)[0].color.as_deref(),
        Some("#ff0000"),
        "the color persisted"
    );

    // Notes and bookmarks are marks too, and all three list together.
    assert!(s.selection_begin(100.0, 70.0));
    s.selection_drag(300.0, 70.0);
    let note = s.add_note("worth revisiting").expect("note is stored");
    s.selection_clear();
    let bookmark = s.add_bookmark().expect("bookmark is stored");

    let all = s.annotations();
    assert_eq!(all.len(), 3, "{all:?}");
    assert_eq!(
        all.iter()
            .filter(|a| a.kind == AnnotationKind::Bookmark)
            .count(),
        1
    );
    let note_summary = all.iter().find(|a| a.id == note).expect("note listed");
    assert_eq!(note_summary.kind, AnnotationKind::Note);
    assert_eq!(note_summary.text.as_deref(), Some("worth revisiting"));
    assert!(all.windows(2).all(|w| w[0].progression <= w[1].progression));

    // A bookmark is a point: it paints nothing, while the ranged marks do.
    assert_eq!(s.highlights(0).len(), 2, "the bookmark isn't painted");

    // Every mark can be jumped to and taken away again.
    assert!(s.goto_annotation(bookmark));
    s.render().expect("page renders");
    assert_eq!(s.spine(), 0);
    s.remove_annotation(note);
    assert_eq!(s.annotations().len(), 2);
    assert_eq!(s.highlights(0).len(), 1);
}
