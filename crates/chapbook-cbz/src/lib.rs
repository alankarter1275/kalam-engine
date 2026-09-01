//! CBZ comic-book archives as [`Publication`]s.
//!
//! A CBZ is a zip of page images with no manifest: reading order is the
//! natural sort of member names (the ComicRack convention — numeric runs
//! compare as numbers, so `page2` < `page10`), the media type comes from
//! the extension or, failing that, from the first bytes of the member, and
//! non-image members (macOS resource forks, hidden files) are skipped. Each image is one spine item; there is no
//! within-page text, so locators for comics carry `char_offset = 0` and
//! live in `spine_index`/progression (see `chapbook_core::locator` on
//! per-format progression units).
//!
//! The one member that is not a page and not junk is `ComicInfo.xml`: when
//! an archive carries one, it supplies the metadata and the bookmarks a
//! bare zip cannot (see [`comicinfo`]). Without it a comic still opens —
//! titled after its filename, with an empty table of contents.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chapbook_core::{
    BookKind, BookMetadata, ChapbookError, Publication, Resource, Result, SpineItem, TocEntry,
};

mod comicinfo;

/// A local CBZ archive, opened and indexed.
pub struct ComicBook {
    metadata: BookMetadata,
    spine: Vec<SpineItem>,
    toc: Vec<TocEntry>,
    /// zip reads need `&mut`; the trait surface is `&self`.
    ///
    /// Boxed rather than generic: a comic opened from a `content://` file
    /// descriptor and one opened from a path are the same publication, and
    /// making `ComicBook` generic would push that difference into every
    /// type that holds one — including the `dyn Publication` the session
    /// already stores it as.
    archive: Mutex<Archive>,
}

/// Extensions accepted as pages, matching the formats the render pipeline
/// decodes. Comic pages in the wild are jpeg/png/webp/gif.
const IMAGE_EXTENSIONS: [(&str, &str); 5] = [
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

fn page_media_type(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_EXTENSIONS
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, mt)| *mt)
}

/// Magic numbers for the same formats, for members the extension cannot
/// classify. Some archives name their pages `001` with nothing after it,
/// and a few give an image the wrong extension outright — a zip has no
/// manifest to correct either, which is why `SpineItem::media_type` has
/// always said an archive format may have to sniff the bytes.
fn sniff_media_type(head: &[u8]) -> Option<&'static str> {
    match head {
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        // RIFF container, four bytes of length, then the form type.
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// The longest prefix any signature above needs.
const SNIFF_BYTES: usize = 12;

fn is_junk(name: &str) -> bool {
    name.split('/')
        .any(|part| part.starts_with('.') || part.eq_ignore_ascii_case("__MACOSX"))
}

fn is_comic_info(name: &str) -> bool {
    name.rsplit('/')
        .next()
        .is_some_and(|file| file.eq_ignore_ascii_case("ComicInfo.xml"))
}

/// How deep a member sits. `ComicInfo.xml` belongs at the archive root, but
/// archives zipped from a directory bury it a level down, and a few carry
/// more than one — take the shallowest and leave the rest alone.
fn depth(name: &str) -> usize {
    name.matches('/').count()
}

/// Read one member. Free rather than a method because `open` needs it
/// before the archive goes behind the mutex.
fn read_entry(archive: &mut Archive, name: &str) -> Result<Vec<u8>> {
    let mut entry = archive
        .by_name(name)
        .map_err(|e| ChapbookError::ResourceNotFound(format!("{name}: {e}")))?;
    let mut data = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut data)
        .map_err(|e| ChapbookError::BookMalformed(format!("reading {name}: {e}")))?;
    Ok(data)
}

/// The first [`SNIFF_BYTES`] of a member, decompressed. Cheap even for a
/// deflated entry: the reader stops as soon as it has them.
fn read_head(archive: &mut Archive, name: &str) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(name).ok()?;
    let mut head = vec![0u8; SNIFF_BYTES];
    let mut filled = 0;
    while filled < SNIFF_BYTES {
        match entry.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    head.truncate(filled);
    Some(head)
}

/// The zip, over whatever the caller handed us.
type Archive = zip::ZipArchive<Box<dyn chapbook_core::ReadSeek>>;

impl ComicBook {
    pub fn open(path: &Path) -> Result<ComicBook> {
        let file = File::open(path).map_err(|e| ChapbookError::BookOpen {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        Self::read_at(Box::new(BufReader::new(file)), path)
    }

    /// Open from bytes already in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<ComicBook> {
        Self::read(std::io::Cursor::new(bytes))
    }

    /// Open from any seekable handle. Seekable because a zip's central
    /// directory is at the end: there is no reading an archive forwards.
    pub fn read(reader: impl chapbook_core::ReadSeek + 'static) -> Result<ComicBook> {
        Self::read_at(Box::new(reader), Path::new(""))
    }

    /// The real constructor. `path` is only ever for error messages — it is
    /// empty for a handle, which is the honest thing to say when there is
    /// no file to name.
    fn read_at(reader: Box<dyn chapbook_core::ReadSeek>, path: &Path) -> Result<ComicBook> {
        let open_err = |reason: String| ChapbookError::BookOpen {
            path: PathBuf::from(path),
            reason,
        };
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| open_err(format!("not a zip archive: {e}")))?;

        let mut pages: Vec<(String, &'static str)> = Vec::new();
        let mut sidecar: Option<String> = None;
        // Members the extension could not classify. Sniffing them needs a
        // second pass: the name comes from a borrow of the archive, and
        // reading the bytes needs another.
        let mut unclassified: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            let entry = archive
                .by_index_raw(i)
                .map_err(|e| open_err(format!("bad zip entry: {e}")))?;
            let name = entry.name().to_string();
            if entry.is_dir() || is_junk(&name) || entry.size() == 0 {
                continue;
            }
            match page_media_type(&name) {
                Some(media_type) => pages.push((name, media_type)),
                None if is_comic_info(&name) => {
                    if depth(&name) < sidecar.as_deref().map_or(usize::MAX, depth) {
                        sidecar = Some(name);
                    }
                }
                None => unclassified.push(name),
            }
        }
        for name in unclassified {
            if let Some(media_type) = read_head(&mut archive, &name)
                .as_deref()
                .and_then(sniff_media_type)
            {
                pages.push((name, media_type));
            }
        }
        if pages.is_empty() {
            return Err(open_err("archive contains no page images".into()));
        }
        pages.sort_by(|(a, _), (b, _)| natural_cmp(a, b));

        let spine: Vec<SpineItem> = pages
            .iter()
            .map(|(name, media_type)| SpineItem {
                id: name.clone(),
                href: name.clone(),
                media_type: (*media_type).into(),
                linear: true,
            })
            .collect();

        let mut metadata = BookMetadata {
            title: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty()),
            format_version: "cbz".into(),
            ..BookMetadata::default()
        };
        // An unreadable or malformed sidecar is not an unreadable comic:
        // the pages are already indexed, so fall through to the filename.
        let mut toc = Vec::new();
        if let Some(info) = sidecar
            .and_then(|name| read_entry(&mut archive, &name).ok())
            .and_then(|bytes| comicinfo::parse(&bytes))
        {
            if let Some(title) = info.display_title() {
                metadata.title = Some(title);
            }
            // The same two fields the display title is built from, kept
            // apart as well: a shelf groups by series and sorts by number,
            // and it cannot get either back out of "Series #3: Title".
            // `Number` is a string in the schema and taggers put
            // `Annual 1` and `HS` in it, so an unparseable one is simply
            // no position, the way a series with no number is.
            metadata.series = info.series;
            metadata.series_index = info.number.as_deref().and_then(|n| n.trim().parse().ok());
            metadata.authors = info.credits;
            metadata.description = info.summary;
            metadata.language = info.language;
            // `Image` indexes the reading-ordered pages, which is what a
            // spine index is — but it is author-supplied, so bound it.
            toc = info
                .bookmarks
                .into_iter()
                .filter(|(image, _)| *image < spine.len())
                .map(|(image, label)| TocEntry {
                    label,
                    href: Some(spine[image].href.clone()),
                    fragment: None,
                    spine_index: Some(image),
                    children: Vec::new(),
                })
                .collect();
        }

        Ok(ComicBook {
            metadata,
            spine,
            toc,
            archive: Mutex::new(archive),
        })
    }

    fn entry_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let mut archive = self.archive.lock().expect("archive lock");
        read_entry(&mut archive, name)
    }
}

impl Publication for ComicBook {
    fn kind(&self) -> BookKind {
        BookKind::Comic
    }

    fn metadata(&self) -> &BookMetadata {
        &self.metadata
    }

    fn spine(&self) -> &[SpineItem] {
        &self.spine
    }

    fn toc(&self) -> &[TocEntry] {
        &self.toc
    }

    fn unit_bytes(&self, spine_index: usize) -> Result<Vec<u8>> {
        let item = self.spine_item(spine_index)?;
        self.entry_bytes(&item.href.clone())
    }

    fn cover(&self) -> Result<Option<Resource>> {
        let first = &self.spine[0];
        Ok(Some(Resource {
            media_type: first.media_type.clone(),
            data: self.entry_bytes(&first.href.clone())?,
        }))
    }
}

/// Case-insensitive natural ordering: digit runs compare numerically,
/// everything else lexicographically. `Page2.jpg` < `Page10.jpg`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ac = a.chars().peekable();
    let mut bc = b.chars().peekable();
    loop {
        match (ac.peek().copied(), bc.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut xn = 0u64;
                let mut yn = 0u64;
                while let Some(d) = ac.peek().and_then(|c| c.to_digit(10)) {
                    xn = xn.saturating_mul(10).saturating_add(d as u64);
                    ac.next();
                }
                while let Some(d) = bc.peek().and_then(|c| c.to_digit(10)) {
                    yn = yn.saturating_mul(10).saturating_add(d as u64);
                    bc.next();
                }
                match xn.cmp(&yn) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(x), Some(y)) => {
                let (xl, yl) = (
                    x.to_lowercase().next().unwrap_or(x),
                    y.to_lowercase().next().unwrap_or(y),
                );
                match xl.cmp(&yl) {
                    Ordering::Equal => {
                        ac.next();
                        bc.next();
                    }
                    other => return other,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{natural_cmp, sniff_media_type};
    use std::cmp::Ordering;

    #[test]
    fn natural_ordering() {
        assert_eq!(natural_cmp("page2.png", "page10.png"), Ordering::Less);
        assert_eq!(natural_cmp("Page2.png", "page2.PNG"), Ordering::Equal); // case-folded
        assert_eq!(natural_cmp("ch1/p9.jpg", "ch1/p10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("a10b2", "a10b10"), Ordering::Less);
        assert_eq!(natural_cmp("cover.png", "page1.png"), Ordering::Less);
    }

    #[test]
    fn signatures_classify_the_formats_we_decode() {
        assert_eq!(
            sniff_media_type(b"\x89PNG\r\n\x1a\n\0\0\0\r"),
            Some("image/png")
        );
        assert_eq!(
            sniff_media_type(b"\xff\xd8\xff\xe0JFIF"),
            Some("image/jpeg")
        );
        assert_eq!(sniff_media_type(b"GIF89a..."), Some("image/gif"));
        assert_eq!(
            sniff_media_type(b"RIFF\x24\0\0\0WEBPVP8 "),
            Some("image/webp")
        );
    }

    #[test]
    fn signatures_reject_what_is_not_a_page() {
        assert_eq!(sniff_media_type(b"just some text"), None);
        assert_eq!(sniff_media_type(b"<ComicInfo/>"), None);
        // A RIFF that is not a WebP — a .wav, say — is not a page either.
        assert_eq!(sniff_media_type(b"RIFF\x24\0\0\0WAVEfmt "), None);
        // Short reads must not panic or match on a prefix.
        assert_eq!(sniff_media_type(b""), None);
        assert_eq!(sniff_media_type(b"\x89PNG"), None);
    }
}
