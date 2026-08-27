//! `Format::sniff` against every book in `fixtures/`.
//!
//! The unit tests in `src/source.rs` build zip headers by hand, which
//! proves the arithmetic and not much else — a hand-built header is a
//! restatement of the code that reads it. This runs the same function over
//! real archives — the checked-in fixtures always, and the downloaded
//! corpus (Standard Ebooks, the EPUB 3 samples) under `--ignored` — and is
//! the test that would actually catch a wrong offset.

use std::path::{Path, PathBuf};

use chapbook_core::{Format, FORMAT_SNIFF_BYTES};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Every file under `dir` with `ext`, sorted so a failure names the same
/// file every run.
fn books(dir: &str, ext: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(fixtures().join(dir))
        .unwrap_or_else(|e| panic!("fixtures/{dir}: {e}"))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    found.sort();
    found
}

fn head_of(path: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).unwrap();
    let mut head = vec![0u8; FORMAT_SNIFF_BYTES];
    let read = file.read(&mut head).unwrap();
    head.truncate(read);
    head
}

fn assert_all(paths: &[PathBuf], expected: Format) {
    assert!(!paths.is_empty(), "no fixtures to check for {expected:?}");
    for path in paths {
        assert_eq!(
            Format::sniff(&head_of(path)),
            Some(expected),
            "{}",
            path.display()
        );
    }
}

#[test]
fn every_epub_fixture_is_recognised_as_one() {
    assert_all(&books("epub", "epub"), Format::Epub);
}

/// The same question asked of books nobody here produced. Separate and
/// ignored because `fixtures/corpus/` is a download, not a checked-in
/// fixture — folded into the test above it made a clean checkout fail.
#[test]
#[ignore = "requires fixtures/fetch-corpus.sh"]
fn every_corpus_book_is_recognised_as_an_epub() {
    assert_all(&books("corpus", "epub"), Format::Epub);
}

#[test]
fn every_cbz_fixture_is_recognised_as_one() {
    assert_all(&books("cbz", "cbz"), Format::Cbz);
}

#[test]
fn every_pdf_fixture_is_recognised_as_one() {
    assert_all(&books("pdf", "pdf"), Format::Pdf);
}

#[test]
fn sniffing_beats_the_extension_it_replaces() {
    // The point of doing this from bytes: a misnamed book opens correctly
    // instead of failing to parse as whatever its name claimed.
    let epub = head_of(&fixtures().join("epub/minimal.epub"));
    assert_eq!(Format::sniff(&epub), Some(Format::Epub));
    assert_eq!(
        Format::from_extension(Path::new("mislabelled.cbz")),
        Some(Format::Cbz),
        "the name says comic"
    );

    let cbz = head_of(&fixtures().join("cbz/minimal.cbz"));
    assert_eq!(Format::sniff(&cbz), Some(Format::Cbz));
    assert_eq!(
        Format::from_extension(Path::new("mislabelled.epub")),
        Some(Format::Epub),
        "and here the name says book — bytes win in both directions"
    );
}

#[test]
fn a_font_is_not_a_book() {
    let fonts = books("fonts", "ttf");
    for path in &fonts {
        assert_eq!(Format::sniff(&head_of(path)), None, "{}", path.display());
    }
}
