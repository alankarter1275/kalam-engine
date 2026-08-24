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
    assert_eq!(book.spine().len(), 3);
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
    assert!(book.unit_bytes(3).is_err());
}

#[test]
fn text_layer_extracts_lines_with_geometry() {
    let book = fixture();
    assert_eq!(book.spine().len(), 3);
    // Pages 0/1 are pure rectangles: no text.
    assert!(book.text_page(0).unwrap().is_empty());
    let lines = book.text_page(2).unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0].text, "Hello selection");
    assert_eq!(lines[1].text, "Second line here");
    // Geometry: 24pt text at x=40; baseline y=340pt from bottom of a
    // 400pt page -> top-left y ≈ 60 - ascent. Line 1 above line 2.
    let l0 = &lines[0];
    assert!(l0.top < lines[1].top);
    assert!(
        (l0.glyphs[0].x - 40.0).abs() < 3.0,
        "first glyph x: {}",
        l0.glyphs[0].x
    );
    assert!(
        l0.height > 15.0 && l0.height < 40.0,
        "line height: {}",
        l0.height
    );
    // Offsets are continuous across the page in reading order.
    assert_eq!(l0.glyphs[0].offset, 0);
    assert_eq!(lines[1].glyphs[0].offset, l0.text.chars().count() as u32);
    // Glyph x positions increase along the line.
    assert!(l0.glyphs.windows(2).all(|w| w[0].x < w[1].x));
}
