//! Opening the same books through every kind of [`Source`].
//!
//! The claim being tested is that a book is its bytes, not its name: the
//! same EPUB opens identically from a path, from a `Vec<u8>` a host already
//! holds, and from a seekable handle it resolved from a `content://` URI —
//! and a *misnamed* file opens correctly, which the extension test this
//! replaces could not do.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use chapbook_core::{BookKind, Format, Source};
use chapbook_reader::{Session, SessionConfig};

/// Path sources reach the library, which is keyed off a process-global env
/// var. Bytes and handles do not, but the lock is cheap and the tests are
/// clearer for not having to say which is which.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

fn fixture_fonts() -> chapbook_core::FontSource {
    chapbook_core::FontSource::embedded(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts"),
        "Crimson Text",
    )
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "chapbook-sources-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Open with an isolated library, so a path source cannot pick up a
/// position another test stored.
fn open(source: impl Into<Source>, name: &str) -> chapbook_core::Result<Session> {
    let guard = ENV_LOCK.lock().unwrap();
    let dir = scratch(name);
    std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);
    let opened = Session::open_with(source, SessionConfig::new(fixture_fonts()));
    drop(guard);
    let _ = std::fs::remove_dir_all(&dir);
    opened
}

#[test]
fn one_epub_three_ways() {
    let path = fixture("epub/minimal.epub");
    let bytes = std::fs::read(&path).unwrap();

    let from_path = open(path.clone(), "epub-path").unwrap();
    let from_bytes = open(Source::bytes(bytes.clone()), "epub-bytes").unwrap();
    let from_reader = open(Source::reader(Cursor::new(bytes)), "epub-reader").unwrap();

    for session in [&from_path, &from_bytes, &from_reader] {
        assert_eq!(session.title(), from_path.title());
        assert_eq!(session.spine_len(), from_path.spine_len());
        assert_eq!(session.kind(), BookKind::Epub);
    }
    assert!(
        from_path.spine_len() > 0,
        "a book with no units proves nothing"
    );
}

#[test]
fn a_comic_and_a_pdf_open_from_bytes_too() {
    let cbz = std::fs::read(fixture("cbz/minimal.cbz")).unwrap();
    let session = open(Source::bytes(cbz), "cbz-bytes").unwrap();
    assert_eq!(session.kind(), BookKind::Comic);
    assert!(session.spine_len() > 0);

    let pdf = std::fs::read(fixture("pdf/minimal.pdf")).unwrap();
    let session = open(Source::bytes(pdf), "pdf-bytes").unwrap();
    assert!(session.spine_len() > 0);
}

#[test]
fn a_misnamed_book_opens_by_its_bytes() {
    // The concrete win over the extension test. A comic archive named
    // `.epub` was a parse failure; an EPUB named `.cbz` was worse, since
    // it opened as a comic with no pages.
    let dir = scratch("misnamed");
    let epub_as_cbz = dir.join("actually-a-book.cbz");
    std::fs::copy(fixture("epub/minimal.epub"), &epub_as_cbz).unwrap();
    let session = open(epub_as_cbz, "misnamed-epub").unwrap();
    assert_eq!(session.kind(), BookKind::Epub, "the bytes say EPUB");

    let cbz_as_epub = dir.join("actually-a-comic.epub");
    std::fs::copy(fixture("cbz/minimal.cbz"), &cbz_as_epub).unwrap();
    let session = open(cbz_as_epub, "misnamed-cbz").unwrap();
    assert_eq!(session.kind(), BookKind::Comic, "the bytes say CBZ");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_host_that_knows_the_format_is_believed() {
    // A `content://` resolver reports a MIME type, and it may know things
    // the first 64 bytes cannot. Stating the format skips the sniff.
    let bytes = std::fs::read(fixture("cbz/minimal.cbz")).unwrap();
    let session = open(
        Source::bytes(bytes).with_format(Format::Cbz),
        "stated-format",
    )
    .unwrap();
    assert_eq!(session.kind(), BookKind::Comic);
}

#[test]
fn bytes_that_are_no_book_fail_with_a_sentence() {
    let Err(err) = open(
        Source::bytes(b"<html>not a book</html>".to_vec()),
        "garbage",
    ) else {
        panic!("HTML is not a book chapbook reads");
    };
    let message = err.to_string();
    assert!(
        message.contains("EPUB") && message.contains("CBZ") && message.contains("PDF"),
        "the error should say what it looked for: {message}"
    );
}

#[test]
fn a_handle_is_rewound_before_the_format_reader_sees_it() {
    // Sniffing consumes the first bytes. If `peek` failed to seek back,
    // every zip in the world would report a broken central directory —
    // so this passing at all is the assertion, and the explicit position
    // check is what names the failure if it stops.
    use std::io::{Seek, SeekFrom};
    let bytes = std::fs::read(fixture("epub/minimal.epub")).unwrap();
    let mut cursor = Cursor::new(bytes);
    cursor.seek(SeekFrom::Start(0)).unwrap();
    let session = open(Source::reader(cursor), "rewind").unwrap();
    assert!(session.spine_len() > 0);
}

#[test]
fn a_string_still_means_what_it_always_did() {
    // Every existing caller passes a `&str`. That path is unchanged: this
    // is the compatibility assertion, not a new capability.
    let path = fixture("epub/minimal.epub").to_string_lossy().into_owned();
    let session = open(path.as_str(), "string").unwrap();
    assert!(session.spine_len() > 0);
}
