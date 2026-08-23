use std::path::Path;

use chapbook_core::{
    BookKind, BookMetadata, ChapbookError, Publication, Resource, Result, SpineItem, TocEntry,
};
use rbook::epub::manifest::EpubManifestEntry;
use rbook::epub::toc::EpubTocEntry;

use crate::href::{percent_decode, resolve_href};
use crate::obfuscation::{self, Obfuscation};

/// An open EPUB, presenting an owned, reading-system-shaped view over rbook.
///
/// Structure (metadata, spine, TOC) is extracted eagerly at open — books are
/// small; resource payloads stay lazy in the zip until asked for. The
/// format-neutral surface is [`Publication`]; EPUB-specific capability
/// (relative-href resource resolution for stylesheets/images/fonts,
/// fixed-layout detection) lives on the inherent methods.
pub struct Book {
    epub: rbook::Epub,
    metadata: BookMetadata,
    spine: Vec<SpineItem>,
    toc: Vec<TocEntry>,
    fixed_layout: bool,
    /// Container paths whose leading bytes are obfuscated (fonts), from
    /// META-INF/encryption.xml. Reads de-obfuscate transparently.
    obfuscated: std::collections::HashMap<String, Obfuscation>,
}

impl Book {
    pub fn open(path: &Path) -> Result<Self> {
        let epub = rbook::Epub::open(path).map_err(|e| ChapbookError::BookOpen {
            path: path.to_owned(),
            reason: e.to_string(),
        })?;
        Self::from_epub(epub)
    }

    fn from_epub(epub: rbook::Epub) -> Result<Self> {
        // Everything below borrows from `epub`; scope the borrows so the
        // handle can move into the returned Book.
        let (metadata, fixed_layout, spine, toc) = Self::extract(&epub);
        let obfuscated = epub
            .read_resource_bytes("/META-INF/encryption.xml")
            .map(|xml| obfuscation::parse_encryption_xml(&xml))
            .unwrap_or_default();
        Ok(Book {
            epub,
            metadata,
            spine,
            toc,
            fixed_layout,
            obfuscated,
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
            format_version: md.version_str().to_string(),
        };

        let mut layout_entries = md.by_property("rendition:layout");
        let fixed_layout = layout_entries.any(|e| e.value() == "pre-paginated");

        // Every spine entry becomes a SpineItem, even when its idref has no
        // manifest entry (malformed book): dropping entries would silently
        // compact the spine indices persisted locators are keyed by. Broken
        // entries get an empty href and fail loudly in unit_bytes instead.
        let spine: Vec<SpineItem> = epub
            .spine()
            .iter()
            .map(|entry| match entry.manifest_entry() {
                Some(manifest) => SpineItem {
                    id: entry.idref().to_string(),
                    href: manifest.href().as_str().trim_start_matches('/').to_string(),
                    media_type: manifest.kind().as_str().to_string(),
                    linear: entry.is_linear(),
                },
                None => SpineItem {
                    id: entry.idref().to_string(),
                    href: String::new(),
                    media_type: String::new(),
                    linear: entry.is_linear(),
                },
            })
            .collect();

        let toc = match epub.toc().contents() {
            Some(root) => root.iter().map(|e| convert_toc(&e, &spine)).collect(),
            None => Vec::new(),
        };

        (metadata, fixed_layout, spine, toc)
    }

    pub fn is_fixed_layout(&self) -> bool {
        self.fixed_layout
    }

    /// Resolve `href` relative to the container-root path `base` (the
    /// referencing document) and read the resource it points to.
    pub fn resource(&self, base: &str, href: &str) -> Result<Resource> {
        let path = resolve_href(base, href);
        let entry = self
            .lookup(&path)
            .ok_or_else(|| ChapbookError::ResourceNotFound(path.clone()))?;
        let mut resource = entry_resource(&entry)?;
        self.maybe_deobfuscate(&path, &mut resource.data);
        Ok(resource)
    }

    fn read_by_path(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .lookup(path)
            .ok_or_else(|| ChapbookError::ResourceNotFound(path.to_string()))?;
        let mut data = entry
            .read_bytes()
            .map_err(|e| ChapbookError::BookMalformed(e.to_string()))?;
        self.maybe_deobfuscate(path, &mut data);
        Ok(data)
    }

    /// Undo IDPF/Adobe font obfuscation when `path` is listed in
    /// META-INF/encryption.xml. Paths are compared raw and percent-decoded,
    /// mirroring the manifest lookup's tolerance.
    fn maybe_deobfuscate(&self, path: &str, data: &mut [u8]) {
        if self.obfuscated.is_empty() {
            return;
        }
        let algo = self
            .obfuscated
            .get(path)
            .or_else(|| self.obfuscated.get(percent_decode(path).as_str()))
            .copied();
        if let Some(algo) = algo {
            let identifier = self.metadata.identifier.as_deref().unwrap_or("");
            obfuscation::deobfuscate(algo, identifier, data);
        }
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

impl Publication for Book {
    fn kind(&self) -> BookKind {
        BookKind::Epub
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

    /// Raw XHTML bytes of a spine item.
    ///
    /// Fixed-layout EPUBs are rejected here — the content gate — so metadata
    /// and TOC inspection still work on them while nothing downstream ever
    /// sees pre-paginated content.
    fn unit_bytes(&self, spine_index: usize) -> Result<Vec<u8>> {
        if self.fixed_layout {
            return Err(ChapbookError::FixedLayoutUnsupported);
        }
        let item = self.spine_item(spine_index)?;
        if item.href.is_empty() {
            return Err(ChapbookError::BookMalformed(format!(
                "spine item \"{}\" references no manifest entry",
                item.id
            )));
        }
        let href = item.href.clone();
        self.read_by_path(&href)
    }

    fn cover(&self) -> Result<Option<Resource>> {
        match self.epub.manifest().cover_image() {
            Some(entry) => entry_resource(&entry).map(Some),
            None => Ok(None),
        }
    }
}

/// Read a manifest entry into an owned [`Resource`].
fn entry_resource(entry: &EpubManifestEntry<'_>) -> Result<Resource> {
    let media_type = entry.kind().as_str().to_string();
    let data = entry
        .read_bytes()
        .map_err(|e| ChapbookError::BookMalformed(e.to_string()))?;
    Ok(Resource { media_type, data })
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
