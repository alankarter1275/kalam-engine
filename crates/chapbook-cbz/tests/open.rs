//! Opening the fixture archive: page order, junk filtering, media types,
//! and the Publication surface.

use std::path::PathBuf;

use chapbook_core::{BookKind, Publication};

fn fixture() -> chapbook_cbz::ComicBook {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cbz/minimal.cbz");
    chapbook_cbz::ComicBook::open(&path).unwrap()
}

#[test]
fn pages_in_natural_order_junk_skipped() {
    let book = fixture();
    assert_eq!(book.kind(), BookKind::Comic);
    let hrefs: Vec<&str> = book.spine().iter().map(|s| s.href.as_str()).collect();
    assert_eq!(hrefs, ["page1.png", "page2.png", "page10.png"]);
    assert!(book.spine().iter().all(|s| s.media_type == "image/png"));
    assert_eq!(book.metadata().title.as_deref(), Some("minimal"));
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
