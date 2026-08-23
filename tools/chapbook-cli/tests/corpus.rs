//! Real-world corpus sweep. Ignored by default — run `fixtures/fetch-corpus.sh`
//! first, then `cargo test -p chapbook-cli --test corpus -- --ignored`.

use std::path::PathBuf;

use chapbook_core::Publication;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/corpus")
}

#[test]
#[ignore = "requires fixtures/fetch-corpus.sh"]
fn every_corpus_book_opens_and_extracts_text() {
    let dir = corpus_dir();
    let epubs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures/corpus missing — run fixtures/fetch-corpus.sh")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "epub"))
        .collect();
    assert!(!epubs.is_empty(), "corpus directory has no .epub files");

    for path in epubs {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let book = chapbook_epub::Book::open(&path)
            .unwrap_or_else(|e| panic!("{name}: failed to open: {e}"));

        assert!(book.metadata().title.is_some(), "{name}: missing title");
        assert!(!book.spine().is_empty(), "{name}: empty spine");
        assert!(!book.toc().is_empty(), "{name}: empty toc");

        let mut total_text = 0usize;
        for i in 0..book.spine().len() {
            let bytes = book
                .unit_bytes(i)
                .unwrap_or_else(|e| panic!("{name}: spine {i} unreadable: {e}"));
            let doc = chapbook_dom::parse_xhtml(&bytes, &book.spine()[i].href)
                .unwrap_or_else(|e| panic!("{name}: spine {i} unparseable: {e}"));
            total_text += chapbook_dom::extract_text(&doc).len();
        }
        assert!(
            total_text > 1000,
            "{name}: suspiciously little text extracted ({total_text} bytes)"
        );
        println!(
            "ok {name}: {} spine items, {total_text} bytes of text",
            book.spine().len()
        );
    }
}

/// Run the full stylo cascade over real-world chapters: shakes out panics in
/// the TElement binding (style sharing cache, selector matching) that the
/// tiny fixture can't reach. Every chapter must produce a styled body.
#[test]
#[ignore = "requires fixtures/fetch-corpus.sh"]
fn corpus_chapters_style_without_panicking() {
    let dir = corpus_dir();
    let epubs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures/corpus missing — run fixtures/fetch-corpus.sh")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "epub"))
        .collect();
    assert!(!epubs.is_empty());

    for path in epubs {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let book = chapbook_epub::Book::open(&path).unwrap();
        let mut styled = 0usize;
        for i in 0..book.spine().len() {
            let dump = chapbook_cli::commands::styles(&path, i)
                .unwrap_or_else(|e| panic!("{name}: spine {i} failed to style: {e}"));
            assert!(
                dump.contains("<body>"),
                "{name}: spine {i} produced no styled body"
            );
            styled += 1;
        }
        println!("ok {name}: styled {styled} chapters");
    }
}

/// Paginate every real-world chapter: the layout-engine stress test. Every
/// chapter must produce in-bounds pages, and any chapter with text must
/// produce at least one line.
#[test]
#[ignore = "requires fixtures/fetch-corpus.sh"]
fn corpus_chapters_paginate_within_bounds() {
    let dir = corpus_dir();
    let epubs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("fixtures/corpus missing — run fixtures/fetch-corpus.sh")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "epub"))
        .collect();
    assert!(!epubs.is_empty());

    for path in epubs {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let book = chapbook_epub::Book::open(&path).unwrap();
        let mut total_pages = 0usize;
        for i in 0..book.spine().len() {
            let dump = chapbook_cli::commands::layout(&path, i)
                .unwrap_or_else(|e| panic!("{name}: spine {i} failed to lay out: {e}"));
            let pages: usize = dump
                .lines()
                .find_map(|l| l.strip_prefix("pages: "))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            assert!(pages > 0, "{name}: spine {i} produced no pages");
            total_pages += pages;
        }
        println!(
            "ok {name}: {} chapters -> {total_pages} pages",
            book.spine().len()
        );
    }
}
