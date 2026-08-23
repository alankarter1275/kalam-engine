//! Library persistence round-trips and the position restore chain.

use chapbook_core::{BookMetadata, LayeredLocator, SpineItem};
use chapbook_library::{restore_position, AnnotationKind, Library, RestoreTier};

fn temp_library() -> (Library, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "chapbook-lib-test-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    (Library::open(&dir).unwrap(), dir)
}

fn rand_suffix() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

fn sample_book(dir: &std::path::Path, name: &str, contents: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn metadata(title: &str) -> BookMetadata {
    BookMetadata {
        title: Some(title.to_string()),
        authors: vec!["Ada Fixture".into(), "Co Author".into()],
        language: Some("en".into()),
        identifier: Some("urn:uuid:test".into()),
        description: None,
        format_version: "3.0".into(),
    }
}

const TEXT: &str = "It was a truth universally acknowledged that a reader in \
                    possession of a position must be in want of restoring it.";

fn locator_at(offset: u32) -> LayeredLocator {
    LayeredLocator::capture("OEBPS/ch2.xhtml", 1, TEXT, offset, 100, 1000)
}

#[test]
fn import_ls_and_fingerprint_dedup() {
    let (mut lib, dir) = temp_library();
    let source = sample_book(&dir, "src.epub", b"fake epub bytes one");
    let id = lib.import(&source, &metadata("Book One")).unwrap();

    // Managed copy exists and the original path is not it.
    let books = lib.books(None).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, "Book One");
    assert_eq!(books[0].authors, vec!["Ada Fixture", "Co Author"]);
    assert!(books[0].file_path.exists());
    assert_ne!(books[0].file_path, source);

    // Same bytes re-imported: same book, no duplicate.
    let again = lib.import(&source, &metadata("Book One Again")).unwrap();
    assert_eq!(id, again);
    assert_eq!(lib.books(None).unwrap().len(), 1);

    // Different bytes: a new book.
    let other = sample_book(&dir, "other.epub", b"fake epub bytes two");
    let other_id = lib.import(&other, &metadata("Book Two")).unwrap();
    assert_ne!(id, other_id);
    assert_eq!(lib.books(None).unwrap().len(), 2);

    // Filter by author substring.
    assert_eq!(lib.books(Some("Fixture")).unwrap().len(), 2);
    assert_eq!(lib.books(Some("Book Two")).unwrap().len(), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn position_roundtrip_and_update() {
    let (mut lib, dir) = temp_library();
    let source = sample_book(&dir, "b.epub", b"bytes");
    let id = lib.import(&source, &metadata("B")).unwrap();

    assert!(lib.position(id).unwrap().is_none());
    let loc = locator_at(40);
    lib.set_position(id, &loc).unwrap();
    let stored = lib.position(id).unwrap().unwrap();
    assert_eq!(stored.locator, loc);

    // Upsert replaces.
    let later = locator_at(80);
    lib.set_position(id, &later).unwrap();
    assert_eq!(lib.position(id).unwrap().unwrap().locator, later);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn annotations_roundtrip_and_soft_delete() {
    let (mut lib, dir) = temp_library();
    let source = sample_book(&dir, "b.epub", b"bytes");
    let id = lib.import(&source, &metadata("B")).unwrap();

    let start = locator_at(10);
    let end = locator_at(30);
    let highlight = lib
        .add_annotation(
            id,
            AnnotationKind::Highlight,
            &start,
            Some(&end),
            Some("a highlighted passage"),
            Some("#ffff00"),
        )
        .unwrap();
    lib.add_annotation(
        id,
        AnnotationKind::Bookmark,
        &locator_at(90),
        None,
        None,
        None,
    )
    .unwrap();

    let annotations = lib.annotations(id).unwrap();
    assert_eq!(annotations.len(), 2);
    assert_eq!(annotations[0].kind, AnnotationKind::Highlight);
    assert_eq!(annotations[0].start, start);
    assert_eq!(annotations[0].end.as_ref(), Some(&end));
    assert_eq!(annotations[1].kind, AnnotationKind::Bookmark);
    assert!(annotations[1].end.is_none());

    lib.delete_annotation(highlight).unwrap();
    let after = lib.annotations(id).unwrap();
    assert_eq!(after.len(), 1, "soft delete hides but keeps the row");
    assert_eq!(after[0].kind, AnnotationKind::Bookmark);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn opds_sources_roundtrip() {
    let (mut lib, dir) = temp_library();
    lib.add_opds_source(
        "https://cat.example.com/opds/abc123secret/",
        Some("Example"),
        Some("user"),
        Some("pw"),
    )
    .unwrap();
    let sources = lib.opds_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].title.as_deref(), Some("Example"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reopen_persists() {
    let (mut lib, dir) = temp_library();
    let source = sample_book(&dir, "b.epub", b"bytes");
    let id = lib.import(&source, &metadata("Persistent")).unwrap();
    lib.set_position(id, &locator_at(55)).unwrap();
    drop(lib);

    let lib = Library::open(&dir).unwrap();
    assert_eq!(lib.books(None).unwrap()[0].title, "Persistent");
    assert_eq!(lib.position(id).unwrap().unwrap().locator.char_offset, 55);
    std::fs::remove_dir_all(&dir).ok();
}

// ---- Restore chain ----

fn spine() -> Vec<SpineItem> {
    ["OEBPS/ch1.xhtml", "OEBPS/ch2.xhtml", "OEBPS/ch3.xhtml"]
        .iter()
        .map(|href| SpineItem {
            id: href.to_string(),
            href: href.to_string(),
            media_type: "application/xhtml+xml".into(),
            linear: true,
        })
        .collect()
}

#[test]
fn restore_same_edition_is_exact() {
    let stored = locator_at(40);
    let (locator, tier) = restore_position(&stored, true, &spine(), |i| {
        (i == 1).then(|| TEXT.to_string())
    });
    assert_eq!(tier, RestoreTier::Exact);
    assert_eq!(locator.spine_index, 1);
    assert_eq!(locator.char_offset, 40);
}

#[test]
fn restore_new_edition_reanchors_via_quote() {
    let stored = locator_at(40);
    // New edition prepends a translator's note to chapter 2.
    let drifted = format!("A note from the translator. {TEXT}");
    let (locator, tier) = restore_position(&stored, false, &spine(), move |i| {
        (i == 1).then(|| drifted.clone())
    });
    assert_eq!(tier, RestoreTier::Quote);
    assert_eq!(locator.spine_index, 1);
    assert_eq!(
        locator.char_offset,
        40 + "A note from the translator. ".chars().count() as u32
    );
}

#[test]
fn restore_finds_content_moved_to_neighbor_chapter() {
    let stored = locator_at(40);
    // The new edition re-split chapters: the passage now lives in ch3.
    let (locator, tier) = restore_position(&stored, false, &spine(), |i| match i {
        1 => Some("Completely different content in chapter two now.".to_string()),
        2 => Some(TEXT.to_string()),
        _ => Some("Front matter.".to_string()),
    });
    assert_eq!(tier, RestoreTier::Quote);
    assert_eq!(
        locator.spine_index, 2,
        "quote found in the neighbor chapter"
    );
    assert_eq!(locator.char_offset, 40);
}

#[test]
fn restore_spine_reorder_follows_href() {
    let stored = locator_at(40);
    // Same edition claim, but spine order changed: href wins over index.
    let mut reordered = spine();
    reordered.swap(0, 1); // ch2 is now index 0
    let (locator, tier) = restore_position(&stored, true, &reordered, |i| {
        (i == 0).then(|| TEXT.to_string())
    });
    assert_eq!(tier, RestoreTier::Exact);
    assert_eq!(locator.spine_index, 0);
}

#[test]
fn restore_degrades_to_chapter_start() {
    let stored = locator_at(40);
    let (locator, tier) = restore_position(&stored, false, &spine(), |_| None);
    assert_eq!(tier, RestoreTier::ChapterStart);
    assert!(locator.spine_index < 3);
    assert_eq!(locator.char_offset, 0);
}
