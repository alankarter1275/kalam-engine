//! Opening the fixture archives: page order, junk filtering, media types,
//! the ComicInfo.xml sidecar, and the Publication surface.

use std::path::PathBuf;

use chapbook_core::{BookKind, Publication};

fn open(name: &str) -> chapbook_cbz::ComicBook {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/cbz")
        .join(name);
    chapbook_cbz::ComicBook::open(&path).unwrap()
}

/// The tagged archive: pages, junk, and a ComicInfo.xml.
fn fixture() -> chapbook_cbz::ComicBook {
    open("minimal.cbz")
}

#[test]
fn pages_in_natural_order_junk_skipped() {
    let book = fixture();
    assert_eq!(book.kind(), BookKind::Comic);
    let hrefs: Vec<&str> = book.spine().iter().map(|s| s.href.as_str()).collect();
    assert_eq!(hrefs, ["page1.png", "page2.png", "page10.png"]);
    assert!(book.spine().iter().all(|s| s.media_type == "image/png"));
}

#[test]
fn comicinfo_supplies_the_metadata() {
    let book = fixture();
    let md = book.metadata();
    // Series and number compose the display title; the ampersand is an
    // entity in the file and has to survive being one.
    assert_eq!(md.title.as_deref(), Some("Cogs & Levers #3"));
    assert_eq!(md.authors, ["Ada Lovelace", "Grace Hopper"]);
    assert_eq!(md.language.as_deref(), Some("en"));
    assert_eq!(
        md.description.as_deref(),
        Some("Three plates in primary colors.")
    );
    // And they survive separately, because a shelf that groups by series
    // cannot get one back out of the composed title.
    assert_eq!(md.series.as_deref(), Some("Cogs & Levers"));
    assert_eq!(md.series_index, Some(3.0));
}

#[test]
fn page_bookmarks_become_the_toc() {
    let book = fixture();
    let toc = book.toc();
    assert_eq!(toc.len(), 2, "{toc:#?}");
    assert_eq!(toc[0].label, "The Escapement");
    assert_eq!(toc[0].spine_index, Some(1));
    assert_eq!(toc[0].href.as_deref(), Some("page2.png"));
    // Image indexes the *sorted* pages, so 2 is page10.png, not page2.
    assert_eq!(toc[1].label, "Afterword");
    assert_eq!(toc[1].href.as_deref(), Some("page10.png"));
    assert!(toc.iter().all(|e| e.children.is_empty()));
}

#[test]
fn without_a_sidecar_a_comic_still_opens() {
    let book = open("bare.cbz");
    assert_eq!(book.spine().len(), 3);
    // Falls back to the filename stem, and has nothing to build a toc from.
    assert_eq!(book.metadata().title.as_deref(), Some("bare"));
    assert!(book.metadata().authors.is_empty());
    assert!(book.toc().is_empty());
}

#[test]
fn unit_bytes_returns_decodable_pages() {
    let book = fixture();
    for i in 0..book.spine().len() {
        let bytes = book.unit_bytes(i).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (120, 180));
    }
    assert!(book.unit_bytes(3).is_err());
}

#[test]
fn cover_is_first_page() {
    let book = fixture();
    let cover = book.cover().unwrap().unwrap();
    assert_eq!(cover.media_type, "image/png");
    assert_eq!(cover.data, book.unit_bytes(0).unwrap());
}

/// Pages named `001` with nothing after them: the extension classifies
/// nothing, so the media type has to come from the member's own bytes.
/// Before this the archive opened as "contains no page images" — every
/// page was a valid PNG and every one was discarded.
#[test]
fn pages_without_an_extension_are_found_by_their_bytes() {
    let book = open("unnamed.cbz");
    let hrefs: Vec<&str> = book.spine().iter().map(|s| s.href.as_str()).collect();
    assert_eq!(hrefs, ["001", "002", "003"]);
    assert!(book.spine().iter().all(|s| s.media_type == "image/png"));
    // `notes` sniffs to nothing and stays out of the spine.
    assert_eq!(book.spine().len(), 3);
    assert!(book.unit_bytes(0).is_ok());
}
