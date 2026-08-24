//! CBZ comic-book archives as [`Publication`]s.
//!
//! A CBZ is a zip of page images with no manifest: reading order is the
//! natural sort of member names (the ComicRack convention — numeric runs
//! compare as numbers, so `page2` < `page10`), the media type is guessed
//! from the extension, and non-image members (`ComicInfo.xml`, macOS
//! resource forks, hidden files) are skipped. Each image is one spine
//! item; there is no within-page text, so locators for comics carry
//! `char_offset = 0` and live in `spine_index`/progression (see
//! `chapbook_core::locator` on per-format progression units).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::Mutex;

use chapbook_core::{
    BookKind, BookMetadata, ChapbookError, Publication, Resource, Result, SpineItem, TocEntry,
};

/// A local CBZ archive, opened and indexed.
pub struct ComicBook {
    metadata: BookMetadata,
    spine: Vec<SpineItem>,
    toc: Vec<TocEntry>,
    /// zip reads need `&mut`; the trait surface is `&self`.
    archive: Mutex<zip::ZipArchive<BufReader<File>>>,
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

fn is_junk(name: &str) -> bool {
    name.split('/')
        .any(|part| part.starts_with('.') || part.eq_ignore_ascii_case("__MACOSX"))
}

impl ComicBook {
    pub fn open(path: &Path) -> Result<ComicBook> {
        let open_err = |reason: String| ChapbookError::BookOpen {
            path: path.to_path_buf(),
            reason,
        };
        let file = File::open(path).map_err(|e| open_err(e.to_string()))?;
        let mut archive = zip::ZipArchive::new(BufReader::new(file))
            .map_err(|e| open_err(format!("not a zip archive: {e}")))?;

        let mut pages: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            let entry = archive
                .by_index_raw(i)
                .map_err(|e| open_err(format!("bad zip entry: {e}")))?;
            let name = entry.name().to_string();
            if entry.is_dir() || is_junk(&name) || entry.size() == 0 {
                continue;
            }
            if page_media_type(&name).is_some() {
                pages.push(name);
            }
        }
        if pages.is_empty() {
            return Err(open_err("archive contains no page images".into()));
        }
        pages.sort_by(|a, b| natural_cmp(a, b));

        let spine: Vec<SpineItem> = pages
            .iter()
            .map(|name| SpineItem {
                id: name.clone(),
                href: name.clone(),
                media_type: page_media_type(name)
                    .unwrap_or("application/octet-stream")
                    .into(),
                linear: true,
            })
            .collect();

        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty());
        Ok(ComicBook {
            metadata: BookMetadata {
                title,
                format_version: "cbz".into(),
                ..BookMetadata::default()
            },
            spine,
            toc: Vec::new(),
            archive: Mutex::new(archive),
        })
    }

    fn entry_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let mut archive = self.archive.lock().expect("archive lock");
        let mut entry = archive
            .by_name(name)
            .map_err(|e| ChapbookError::ResourceNotFound(format!("{name}: {e}")))?;
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut data)
            .map_err(|e| ChapbookError::BookMalformed(format!("reading {name}: {e}")))?;
        Ok(data)
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
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn natural_ordering() {
        assert_eq!(natural_cmp("page2.png", "page10.png"), Ordering::Less);
        assert_eq!(natural_cmp("Page2.png", "page2.PNG"), Ordering::Equal); // case-folded
        assert_eq!(natural_cmp("ch1/p9.jpg", "ch1/p10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("a10b2", "a10b10"), Ordering::Less);
        assert_eq!(natural_cmp("cover.png", "page1.png"), Ordering::Less);
    }
}
