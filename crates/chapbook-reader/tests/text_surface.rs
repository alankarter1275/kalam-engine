//! The text surface: page text runs with geometry, the material an
//! accessibility tree, TTS, or a selection loupe is built from.

use std::path::PathBuf;

use chapbook_core::{BookKind, EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::{Session, SessionConfig};

fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// The vendored fixture faces, all three axes pinned, so this suite means
/// the same thing on Linux, on a Mac and on a device.
fn fixture_fonts() -> chapbook_core::FontSource {
    chapbook_core::FontSource::embedded(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts"),
        "Crimson Text",
    )
}

/// Open a session against a per-test library dir (see tests/session.rs).
fn open_isolated(name: &str, source: &str) -> Session {
    let dir = std::env::temp_dir().join(format!(
        "chapbook-text-surface-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Session::open_with(
        source,
        SessionConfig::new(fixture_fonts()).with_library_dir(dir),
    )
    .unwrap()
}

/// Drive the async load path to completion: render (queues the load),
/// then poll until the unit lands. Panics after ~5s.
#[cfg(feature = "cbz")]
fn render_loaded(s: &mut Session) {
    for _ in 0..200 {
        s.render().expect("render");
        if !s.has_pending_loads() {
            s.poll_loaded();
            s.render().expect("render");
            return;
        }
        s.poll_loaded();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("unit never finished loading");
}

fn metrics() -> PageMetrics {
    PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(40.0),
        dpi_scale: 1.0,
        rotation: Rotation::None,
    }
}

/// The one locator range where `needle` occurs in the open unit — offsets
/// from search, never written down (see tests/session.rs).
fn only_hit(s: &mut Session, needle: &str) -> (u32, u32) {
    let spine = s.spine();
    let hits = s.search_unit(spine, needle);
    assert_eq!(hits.len(), 1, "{needle:?} should occur exactly once");
    (hits[0].locator.char_offset, hits[0].end)
}

/// Every run's rect sits on the page, its range is well-formed, and run
/// starts never decrease — reading order, the invariant a screen reader
/// walks the list by.
fn assert_runs_sane(runs: &[chapbook_reader::TextRun], page: &PageMetrics) {
    assert!(!runs.is_empty(), "a text page has runs");
    let mut last_start = 0;
    for run in runs {
        assert!(!run.text.is_empty(), "empty runs are skipped");
        assert!(
            run.locator_end >= run.locator_start,
            "range is [start, end): {run:?}"
        );
        assert!(
            run.locator_start >= last_start,
            "runs are in reading order: {run:?}"
        );
        last_start = run.locator_start;
        assert!(
            run.rect.min_x() >= -0.5
                && run.rect.min_y() >= -0.5
                && run.rect.max_x() <= page.size.w + 0.5
                && run.rect.max_y() <= page.size.h + 0.5,
            "rect is on the page: {run:?}"
        );
    }
}

#[test]
fn epub_page_runs_carry_the_page_text() {
    let mut s = open_isolated("epub-runs", &fixture("epub/illustrated.epub"));
    assert_eq!(s.kind(), BookKind::Epub);
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let runs = s.page_text_runs().expect("page is laid out");
    assert_runs_sane(&runs, &metrics());

    // The heading is on the first page: its locator range lies inside the
    // page's run coverage, and its words appear in the run text.
    let (start, end) = only_hit(&mut s, "Illustrated Chapter");
    let runs = s.page_text_runs().expect("still laid out");
    let first = runs.first().unwrap().locator_start;
    let last = runs.iter().map(|r| r.locator_end).max().unwrap();
    assert!(
        first <= start && end <= last,
        "heading range {start}..{end} inside page coverage {first}..{last}"
    );
    let page_text = runs
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        page_text.contains("Illustrated Chapter"),
        "heading text present in runs: {page_text:?}"
    );
}

#[test]
fn runs_are_none_before_layout() {
    let s = open_isolated("epub-runs-early", &fixture("epub/illustrated.epub"));
    // No metrics, nothing laid out: not-laid-out is distinguishable from
    // "laid out with nothing to speak".
    assert_eq!(s.page_text_runs(), None);
    assert!(s.range_rects(0, 10).is_empty());
}

#[test]
fn range_rects_agrees_with_selection_geometry() {
    let mut s = open_isolated("epub-runs-rects", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let (start, end) = only_hit(&mut s, "Illustrated Chapter");
    let rects = s.range_rects(start, end);
    assert!(!rects.is_empty(), "the heading has geometry");

    // Every rect lies inside a run that overlaps the range.
    let runs = s.page_text_runs().expect("laid out");
    for rect in &rects {
        assert!(
            runs.iter()
                .filter(|r| r.locator_start < end && start < r.locator_end)
                .any(|r| r.rect.min_y() <= rect.min_y() + 0.5
                    && rect.max_y() <= r.rect.max_y() + 0.5),
            "rect {rect:?} sits inside an overlapping run"
        );
    }

    // An empty range has no geometry.
    assert!(s.range_rects(start, start).is_empty());
}

#[test]
#[cfg(feature = "pdf")]
fn pdf_hidden_text_becomes_runs() {
    let mut s = open_isolated("pdf-runs", &fixture("pdf/minimal.pdf"));
    assert_eq!(s.kind(), BookKind::Pdf);
    s.set_metrics(metrics());

    // Pages 1 and 2 are rect drawings with no text layer: laid out, but
    // nothing to speak.
    render_loaded(&mut s);
    assert_eq!(s.page_text_runs(), Some(vec![]));

    // Page 3 carries the Helvetica text lines as hidden text.
    s.next_page();
    s.next_page();
    assert_eq!(s.spine(), 2);
    render_loaded(&mut s);
    let runs = s.page_text_runs().expect("pdf page is laid out");
    assert_runs_sane(&runs, &metrics());
    for run in &runs {
        // PDF extracted lines are contiguous in locator space, so the
        // range width is exactly the text's char count.
        assert_eq!(
            run.locator_end - run.locator_start,
            run.text.chars().count() as u32,
            "contiguous hidden text: {run:?}"
        );
    }
    let (start, end) = (runs[0].locator_start, runs[0].locator_end);
    assert!(!s.range_rects(start, end).is_empty());
}

#[test]
#[cfg(feature = "cbz")]
fn comic_page_has_no_runs() {
    let mut s = open_isolated("cbz-runs", &fixture("cbz/minimal.cbz"));
    assert_eq!(s.kind(), BookKind::Comic);
    s.set_metrics(metrics());
    render_loaded(&mut s);
    // Laid out, nothing to speak — not the same answer as "not laid out".
    assert_eq!(s.page_text_runs(), Some(vec![]));
}

/// A span's slice of the speakable text.
fn span_text(page: &chapbook_reader::SpeakablePage, span: &chapbook_reader::WordSpan) -> String {
    page.text
        .chars()
        .skip(span.text_start as usize)
        .take((span.text_end - span.text_start) as usize)
        .collect()
}

/// Find a point that hit-tests onto text, then put the selection back.
fn sweep_for_text(s: &mut Session) -> (f32, f32) {
    for y in (60..760).step_by(8) {
        for x in (60..560).step_by(8) {
            if s.selection_begin(x as f32, y as f32) {
                s.selection_clear();
                return (x as f32, y as f32);
            }
        }
    }
    panic!("no hit-testable text on this page");
}

#[test]
fn speakable_words_index_the_text() {
    let mut s = open_isolated("epub-speak", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let page = s.speakable_page().expect("page is laid out");
    assert!(!page.words.is_empty(), "a text page has words");
    assert!(!page.text.contains('\u{AD}'), "soft hyphens are not spoken");
    assert!(
        !page.text.contains("  "),
        "whitespace collapses to one space"
    );
    assert!(!page.text.starts_with(' ') && !page.text.ends_with(' '));

    let mut last_start = 0;
    for span in &page.words {
        let word = span_text(&page, span);
        assert!(!word.is_empty(), "a span names text: {span:?}");
        assert!(
            word.chars().any(char::is_alphanumeric),
            "words carry alphanumerics, punctuation is nobody's word: {word:?}"
        );
        assert!(span.locator_end > span.locator_start, "{span:?}");
        assert!(span.text_start >= last_start, "reading order: {span:?}");
        last_start = span.text_start;
    }
}

#[test]
fn words_agree_with_search() {
    let mut s = open_isolated("epub-speak-search", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    // The heading's locator range, from search; the word table must cover
    // it with spans whose text is the heading's words.
    let (start, end) = only_hit(&mut s, "Illustrated Chapter");
    let page = s.speakable_page().expect("laid out");
    let covering: Vec<_> = page
        .words
        .iter()
        .filter(|w| w.locator_start >= start && w.locator_end <= end)
        .collect();
    assert_eq!(covering.len(), 2, "two words inside the heading range");
    assert_eq!(span_text(&page, covering[0]), "Illustrated");
    assert_eq!(span_text(&page, covering[1]), "Chapter");

    // A span's locator range reproduces its text through selection — the
    // two surfaces agree on what the offsets name.
    let word = *covering[0];
    s.select_range(word.locator_start, word.locator_end);
    assert_eq!(s.selected_text().as_deref(), Some("Illustrated"));
}

#[test]
fn word_at_finds_the_word_under_a_point() {
    let mut s = open_isolated("epub-word-at", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let (x, y) = sweep_for_text(&mut s);
    let (start, end) = s.word_at(x, y).expect("a word sits under text");
    assert!(end > start);
    assert!(
        !s.range_rects(start, end).is_empty(),
        "the word has geometry for a highlight"
    );

    // The word is in the table, and it is a word.
    let page = s.speakable_page().expect("laid out");
    let span = page
        .words
        .iter()
        .find(|w| (w.locator_start, w.locator_end) == (start, end))
        .expect("word_at answers from the word table");
    assert!(span_text(&page, span).chars().any(char::is_alphanumeric));
}

#[test]
fn select_word_at_sets_a_selection() {
    let mut s = open_isolated("epub-word-select", &fixture("epub/illustrated.epub"));
    s.set_metrics(metrics());
    s.render().expect("page renders");

    let (x, y) = sweep_for_text(&mut s);
    assert!(s.select_word_at(x, y), "a word was there");
    let (start, end) = s.selected_range().expect("the word is selected");
    assert_eq!(s.word_at(x, y), Some((start, end)));
    let text = s.selected_text().expect("the selection carries text");
    assert!(text.chars().any(char::is_alphanumeric), "{text:?}");
}

#[test]
#[cfg(feature = "pdf")]
fn pdf_speakable_page_has_words() {
    let mut s = open_isolated("pdf-speak", &fixture("pdf/minimal.pdf"));
    s.set_metrics(metrics());
    s.next_page();
    s.next_page();
    assert_eq!(s.spine(), 2);
    render_loaded(&mut s);

    let page = s.speakable_page().expect("pdf page is laid out");
    assert!(!page.words.is_empty(), "the text page has words");
    let text_chars = page.text.chars().count() as u32;
    for span in &page.words {
        assert!(span.text_end <= text_chars, "{span:?}");
        assert!(!span_text(&page, span).is_empty());
    }
}

#[test]
#[cfg(feature = "cbz")]
fn comic_speakable_page_is_empty() {
    let mut s = open_isolated("cbz-speak", &fixture("cbz/minimal.cbz"));
    s.set_metrics(metrics());
    render_loaded(&mut s);
    let page = s.speakable_page().expect("comic page is laid out");
    assert!(page.text.is_empty() && page.words.is_empty());
}

#[test]
fn speakable_page_none_before_layout() {
    let s = open_isolated("epub-speak-early", &fixture("epub/illustrated.epub"));
    assert_eq!(s.speakable_page(), None);
}
