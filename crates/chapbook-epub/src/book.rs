use std::path::Path;

use chapbook_core::{ChapbookError, Result};
use rbook::epub::manifest::EpubManifestEntry;
use rbook::epub::toc::EpubTocEntry;

use crate::href::{percent_decode, resolve_href};

/// Owned metadata extracted from the package document at open time.
#[derive(Debug, Clone, Default)]
pub struct BookMetadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub description: Option<String>,
    pub epub_version: String,
}

/// One entry of the spine, in canonical reading order.
#[derive(Debug, Clone)]
pub struct SpineItem {
    pub idref: String,
    /// Normalized container-root path of the content document.
    pub href: String,
    pub media_type: String,
    /// Non-linear items are auxiliary content (notes, answers) skipped in
    /// sequential reading.
    pub linear: bool,
}

/// A table-of-contents node; `children` nest arbitrarily deep.
#[derive(Debug, Clone)]
pub struct TocEntry {
    pub label: String,
    /// Container-root path of the target document, if the entry links anywhere.
    pub href: Option<String>,
    /// Fragment identifier within the target document, if any.
    pub fragment: Option<String>,
    /// Spine index of the target document, if it is a linear spine item.
    pub spine_index: Option<usize>,
    pub children: Vec<TocEntry>,
}

/// A resolved resource from within the container.
pub struct Resource {
    pub media_type: String,
    pub data: Vec<u8>,
}

/// An open EPUB, presenting an owned, reading-system-shaped view over rbook.
///
/// Structure (metadata, spine, TOC) is extracted eagerly at open — books are
/// small; resource payloads stay lazy in the zip until asked for.
pub struct Book {
    epub: rbook::Epub,
    metadata: BookMetadata,
    spine: Vec<SpineItem>,
    toc: Vec<TocEntry>,
    fixed_layout: bool,
}

impl Book {
    pub fn open(path: &Path) -> Result<Self> {
        let epub = rbook::Epub::open(path).map_err(|e| ChapbookError::EpubOpen {
            path: path.to_owned(),
            reason: e.to_string(),
        })?;
        Self::from_epub(epub)
    }

    fn from_epub(epub: rbook::Epub) -> Result<Self> {
        // Everything below borrows from `epub`; scope the borrows so the
        // handle can move into the returned Book.
        let (metadata, fixed_layout, spine, toc) = Self::extract(&epub);
        Ok(Book {
            epub,
            metadata,
            spine,
            toc,
            fixed_layout,
        })
    }

    #[allow(clippy::type_complexity)]
    fn extract(epub: &rbook::Epub) -> (BookMetadata, bool, Vec<SpineItem>, Vec<TocEntry>) {
        let md = epub.metadata();
        let metadata = BookMetadata {
            title: md.title().map(|t| t.value().to_string()),
            authors: md.creators().map(|c| c.value().to_string()).collect(),
            language: md.language().map(|l| l.value().to_string()),
            identifier: md.identifier().map(|i| i.value().to_string()),
            description: md.description().map(|d| d.value().to_string()),
            epub_version: md.version_str().to_string(),
        };

        let mut layout_entries = md.by_property("rendition:layout");
        let fixed_layout = layout_entries.any(|e| e.value() == "pre-paginated");

        let spine: Vec<SpineItem> = epub
            .spine()
            .iter()
            .filter_map(|entry| {
                let manifest = entry.manifest_entry()?;
                Some(SpineItem {
                    idref: entry.idref().to_string(),
                    href: manifest.href().as_str().trim_start_matches('/').to_string(),
                    media_type: manifest.kind().as_str().to_string(),
                    linear: entry.is_linear(),
                })
            })
            .collect();

        let toc = match epub.toc().contents() {
            Some(root) => root.iter().map(|e| convert_toc(&e, &spine)).collect(),
            None => Vec::new(),
        };

        (metadata, fixed_layout, spine, toc)
    }

    pub fn metadata(&self) -> &BookMetadata {
        &self.metadata
    }

    pub fn spine(&self) -> &[SpineItem] {
        &self.spine
    }

    pub fn toc(&self) -> &[TocEntry] {
        &self.toc
    }

    pub fn is_fixed_layout(&self) -> bool {
        self.fixed_layout
    }

    /// Raw XHTML bytes of a spine item.
    pub fn chapter_xhtml(&self, spine_index: usize) -> Result<Vec<u8>> {
        let item = self
            .spine
            .get(spine_index)
            .ok_or(ChapbookError::SpineOutOfRange(spine_index))?;
        let href = item.href.clone();
        self.read_by_path(&href)
    }

    /// Resolve `href` relative to the container-root path `base` (the
    /// referencing document) and read the resource it points to.
    pub fn resource(&self, base: &str, href: &str) -> Result<Resource> {
        let path = resolve_href(base, href);
        let entry = self
            .lookup(&path)
            .ok_or_else(|| ChapbookError::ResourceNotFound(path.clone()))?;
        let media_type = entry.kind().as_str().to_string();
        let data = entry
            .read_bytes()
            .map_err(|e| ChapbookError::EpubMalformed(e.to_string()))?;
        Ok(Resource { media_type, data })
    }

    fn read_by_path(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .lookup(path)
            .ok_or_else(|| ChapbookError::ResourceNotFound(path.to_string()))?;
        entry
            .read_bytes()
            .map_err(|e| ChapbookError::EpubMalformed(e.to_string()))
    }

    /// Manifest lookup by container-root path (no leading slash). rbook keys
    /// hrefs container-absolute (leading `/`), unnormalized; try the raw form
    /// first, then percent-decoded for packages that list hrefs decoded.
    fn lookup(&self, path: &str) -> Option<EpubManifestEntry<'_>> {
        let manifest = self.epub.manifest();
        let absolute = format!("/{path}");
        manifest
            .by_href(&absolute)
            .or_else(|| manifest.by_href(&percent_decode(&absolute)))
    }
}

fn convert_toc(entry: &EpubTocEntry<'_>, spine: &[SpineItem]) -> TocEntry {
    let (href, fragment) = match entry.href() {
        Some(h) => (
            Some(h.path().as_str().trim_start_matches('/').to_string()).filter(|p| !p.is_empty()),
            h.fragment().map(str::to_string),
        ),
        None => (None, None),
    };
    let spine_index = href
        .as_deref()
        .and_then(|h| spine.iter().position(|s| s.href == h));
    TocEntry {
        label: entry.label().to_string(),
        href,
        fragment,
        spine_index,
        children: entry.iter().map(|c| convert_toc(&c, spine)).collect(),
    }
}
