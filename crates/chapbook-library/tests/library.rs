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

/// A publication that is nothing but its metadata and, optionally, a
/// cover. `import` takes the whole publication now — it needs `cover()` —
/// so the tests need something that is one.
struct FakeBook {
    metadata: BookMetadata,
    cover: Option<chapbook_core::Resource>,
}

impl FakeBook {
    fn new(title: &str) -> FakeBook {
        FakeBook {
            metadata: metadata(title),
            cover: None,
        }
    }

    fn with_cover(mut self, media_type: &str, data: &[u8]) -> FakeBook {
        self.cover = Some(chapbook_core::Resource {
            media_type: media_type.to_string(),
            data: data.to_vec(),
        });
        self
    }
}

impl chapbook_core::Publication for FakeBook {
    fn kind(&self) -> chapbook_core::BookKind {
        chapbook_core::BookKind::Epub
    }
    fn metadata(&self) -> &BookMetadata {
        &self.metadata
    }
    fn spine(&self) -> &[SpineItem] {
        &[]
    }
    fn toc(&self) -> &[chapbook_core::TocEntry] {
        &[]
    }
    fn unit_bytes(&self, index: usize) -> chapbook_core::Result<Vec<u8>> {
        Err(chapbook_core::ChapbookError::SpineOutOfRange(index))
    }
    fn cover(&self) -> chapbook_core::Result<Option<chapbook_core::Resource>> {
        Ok(self.cover.as_ref().map(|c| chapbook_core::Resource {
            media_type: c.media_type.clone(),
            data: c.data.clone(),
        }))
    }
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
    let id = lib.import(&source, &FakeBook::new("Book One")).unwrap();

    // Managed copy exists and the original path is not it.
    let books = lib.books(None).unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, "Book One");
    assert_eq!(books[0].authors, vec!["Ada Fixture", "Co Author"]);
    assert!(books[0].file_path.exists());
    assert_ne!(books[0].file_path, source);

    // Same bytes re-imported: same book, no duplicate.
    let again = lib
        .import(&source, &FakeBook::new("Book One Again"))
        .unwrap();
    assert_eq!(id, again);
    assert_eq!(lib.books(None).unwrap().len(), 1);

    // Different bytes: a new book.
    let other = sample_book(&dir, "other.epub", b"fake epub bytes two");
    let other_id = lib.import(&other, &FakeBook::new("Book Two")).unwrap();
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
    let id = lib.import(&source, &FakeBook::new("B")).unwrap();

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
    let id = lib.import(&source, &FakeBook::new("B")).unwrap();

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
fn opds_sources_roundtrip_and_hold_no_secret() {
    let (mut lib, dir) = temp_library();
    let id = lib
        .add_opds_source(
            "https://cat.example.com/opds/abc123secret/",
            Some("Example"),
            Some("user"),
        )
        .unwrap();
    let sources = lib.opds_sources().unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].title.as_deref(), Some("Example"));
    assert_eq!(sources[0].auth_user.as_deref(), Some("user"));

    // The id is the credential key, and it must not be secret-bearing.
    // (That the secret column is gone from the schema is db.rs's test.)
    let key = chapbook_core::CredentialKey::opds_source(id);
    assert!(!key.as_str().contains("abc123secret"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reopen_persists() {
    let (mut lib, dir) = temp_library();
    let source = sample_book(&dir, "b.epub", b"bytes");
    let id = lib.import(&source, &FakeBook::new("Persistent")).unwrap();
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

#[test]
fn reading_settings_resolve_book_then_default_then_builtin() {
    use chapbook_core::{ReadingSettings, Theme};

    let (mut lib, dir) = temp_library();
    let path = sample_book(&dir, "settings.epub", b"settings fixture");
    let id = lib.import(&path, &FakeBook::new("Settings")).unwrap();

    // Nothing stored: the built-in defaults.
    assert_eq!(lib.effective_settings(Some(id)), ReadingSettings::default());
    assert_eq!(lib.reading_settings(None).unwrap(), None);

    let global = ReadingSettings {
        base_font_px: 21.0,
        theme: Theme::Dark,
        ..Default::default()
    };
    lib.set_reading_settings(None, &global).unwrap();
    assert_eq!(
        lib.effective_settings(Some(id)),
        global,
        "book follows the default"
    );

    let mut mine = global.clone();
    mine.base_font_px = 15.0;
    mine.justify = true;
    lib.set_reading_settings(Some(id), &mine).unwrap();
    assert_eq!(lib.effective_settings(Some(id)), mine);
    assert_eq!(
        lib.effective_settings(None),
        global,
        "the default is untouched"
    );

    // A later change to the default leaves the override alone.
    let mut moved = global.clone();
    moved.base_font_px = 30.0;
    lib.set_reading_settings(None, &moved).unwrap();
    assert_eq!(lib.effective_settings(Some(id)).base_font_px, 15.0);

    lib.clear_reading_settings(id).unwrap();
    assert_eq!(
        lib.effective_settings(Some(id)),
        moved,
        "back to the default"
    );
}

/// A 1x1 PNG, so the cover bytes are a real image and not a marker.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[test]
fn a_cover_is_kept_at_import_so_a_shelf_has_something_to_draw() {
    let (mut lib, dir) = temp_library();
    let source = sample_book(&dir, "with-cover.epub", b"bytes");
    let id = lib
        .import(
            &source,
            &FakeBook::new("Illustrated").with_cover("image/png", PNG),
        )
        .unwrap();

    let record = lib.book(id).unwrap().unwrap();
    let cover = record.cover_path.expect("a cover was offered and kept");
    assert_eq!(cover.extension().unwrap(), "png", "named for its type");
    assert_eq!(std::fs::read(&cover).unwrap(), PNG, "bytes land intact");

    // A book with no cover says so rather than pointing at nothing.
    let bare = sample_book(&dir, "bare.epub", b"other bytes");
    let bare_id = lib.import(&bare, &FakeBook::new("Bare")).unwrap();
    assert!(lib.book(bare_id).unwrap().unwrap().cover_path.is_none());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn recent_puts_what_you_were_reading_first() {
    let (mut lib, dir) = temp_library();
    let first = lib
        .import(
            &sample_book(&dir, "first.epub", b"one"),
            &FakeBook::new("Added First"),
        )
        .unwrap();
    let second = lib
        .import(
            &sample_book(&dir, "second.epub", b"two"),
            &FakeBook::new("Added Second"),
        )
        .unwrap();

    // Nothing read yet: newest addition leads, same as a plain listing.
    let shelf = lib.recent(None).unwrap();
    assert_eq!(shelf[0].id, second);
    assert!(shelf.iter().all(|b| b.last_read.is_none()));
    assert!(shelf.iter().all(|b| b.progress.is_none()));

    // Open the older one. It goes to the front, and brings its progress.
    let mut locator = locator_at(10);
    locator.book_progression = 0.42;
    lib.set_position(first, &locator).unwrap();

    let shelf = lib.recent(None).unwrap();
    assert_eq!(shelf[0].id, first, "the book you are in the middle of");
    assert!(shelf[0].last_read.is_some());
    assert!((shelf[0].progress.unwrap() - 0.42).abs() < 1e-9);
    assert_eq!(shelf[1].id, second, "unread still sorts by when it arrived");

    // And a plain listing is unmoved: the two orders answer different
    // questions and must not have become the same method.
    assert_eq!(lib.books(None).unwrap()[0].id, second);

    // The limit is a limit.
    assert_eq!(lib.recent(Some(1)).unwrap().len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_removed_book_leaves_the_shelf_and_keeps_its_annotations() {
    let (mut lib, dir) = temp_library();
    let id = lib
        .import(
            &sample_book(&dir, "removable.epub", b"bytes"),
            &FakeBook::new("Removable"),
        )
        .unwrap();
    let start = locator_at(10);
    lib.add_annotation(id, AnnotationKind::Bookmark, &start, None, None, None)
        .unwrap();

    lib.delete_book(id).unwrap();
    assert!(lib.recent(None).unwrap().is_empty(), "gone from the shelf");
    assert!(lib.books(None).unwrap().is_empty(), "and from the listing");
    assert!(lib.book(id).unwrap().is_none());

    // Soft, so re-importing the same file finds its highlights again —
    // which is why the column was in the schema before anything set it.
    assert_eq!(lib.annotations(id).unwrap().len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}
