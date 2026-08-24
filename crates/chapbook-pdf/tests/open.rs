//! Fixture PDF: page indexing, metadata, and rasterization to decodable
//! PNGs with the expected colors.

use std::path::PathBuf;

use chapbook_core::{BookKind, Publication};

fn fixture() -> chapbook_pdf::PdfBook {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pdf/minimal.pdf");
    chapbook_pdf::PdfBook::open(&path).unwrap()
}

#[test]
fn pages_and_metadata() {
    let book = fixture();
    assert_eq!(book.kind(), BookKind::Pdf);
    assert_eq!(book.spine().len(), 2);
    assert_eq!(book.metadata().title.as_deref(), Some("Minimal Fixture"));
    assert_eq!(book.metadata().authors, ["Chapbook Tests"]);
    assert!(book.spine().iter().all(|s| s.media_type == "image/png"));
}

#[test]
fn pages_rasterize_at_scale_with_expected_colors() {
    let book = fixture();
    // 300x400pt at 2x -> 600x800 px.
    let png = book.unit_bytes(0).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!((img.width(), img.height()), (600, 800));
    let p = img.get_pixel(300, 400);
    assert!(p[0] > 150 && p[2] < 100, "page 1 must be red: {p:?}");
    let img2 = image::load_from_memory(&book.unit_bytes(1).unwrap())
        .unwrap()
        .to_rgba8();
    let p2 = img2.get_pixel(300, 400);
    assert!(p2[2] > 120 && p2[0] < 100, "page 2 must be blue: {p2:?}");
    assert!(book.unit_bytes(2).is_err());
}
