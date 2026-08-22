//! The format-neutral book model.
//!
//! Chapbook is EPUB-first, but these types deliberately live here rather than
//! in `chapbook-epub` so image-per-page formats (CBZ, and eventually others)
//! can join later without consumers — the library, the viewer, the CLI —
//! growing an EPUB dependency. A future `chapbook-cbz` produces the same
//! model: each archive image is one spine item whose content is the page.

use crate::error::ChapbookError;
use crate::Result;

/// The publication format behind a [`Publication`].
///
/// Deliberately exhaustive: all consumers live in this workspace, and the
/// compile error on every `match` when a variant is added is exactly the
/// audit we want.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BookKind {
    Epub,
    /// Comic book archive (planned; no implementation yet).
    Cbz,
}

/// Owned metadata extracted from a publication at open time.
#[derive(Debug, Clone, Default)]
pub struct BookMetadata {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub description: Option<String>,
    /// Format-specific version string (e.g. `"3.0"` for an EPUB 3 package).
    pub format_version: String,
}

/// One entry of the spine: the canonical reading-order sequence. For EPUB an
/// XHTML content document; for an image-per-page format, one page image.
#[derive(Debug, Clone)]
pub struct SpineItem {
    /// Format-level identifier (EPUB manifest idref; archive member name for
    /// page-image formats).
    pub id: String,
    /// Container-root path of the unit's content. Empty when the spine
    /// references a missing manifest entry (malformed book): the item is kept
    /// anyway so spine indices stay stable — persisted locators are keyed by
    /// them — and `unit_bytes` for it fails with a clear error instead.
    pub href: String,
    /// Advisory content type. Provenance varies by format: EPUB declares it
    /// in the package manifest; archive formats (CBZ) have no manifest and
    /// must guess from the filename extension or sniff bytes — and comic
    /// pages in the wild include `image/webp`. Don't assume it's uniform or
    /// authoritative across producers.
    pub media_type: String,
    /// Non-linear items are auxiliary content (notes, answers) skipped in
    /// sequential reading. Always `true` for page-image formats.
    pub linear: bool,
}

/// A table-of-contents node; `children` nest arbitrarily deep.
#[derive(Debug, Clone)]
pub struct TocEntry {
    pub label: String,
    /// Container-root path of the target unit, if the entry links anywhere.
    pub href: Option<String>,
    /// Fragment identifier within the target, if any (text formats only).
    pub fragment: Option<String>,
    /// Spine index of the target, if it is a spine item.
    pub spine_index: Option<usize>,
    pub children: Vec<TocEntry>,
}

/// A resolved resource from within a publication's container.
pub struct Resource {
    pub media_type: String,
    pub data: Vec<u8>,
}

/// The format-neutral surface consumers program against.
///
/// Format-specific capability (EPUB's relative-href resource resolution,
/// stylesheets, fonts) stays on the concrete types; this trait is only what
/// a bookshelf, a position store, or a shell UI needs from *any* book.
pub trait Publication {
    fn kind(&self) -> BookKind;
    fn metadata(&self) -> &BookMetadata;
    fn spine(&self) -> &[SpineItem];
    fn toc(&self) -> &[TocEntry];

    /// Raw bytes of one reading unit — an XHTML chapter, a page image.
    ///
    /// # Blocking contract
    ///
    /// This call may **block for seconds and fail with
    /// [`ChapbookError::Network`]**. Local containers return in microseconds,
    /// but the same trait will back remote publications (e.g. OPDS page
    /// streaming à la OPDS-PSE, where each page is an HTTP fetch); the model
    /// is deliberately "blocking fetch + cache", consistent with the blocking
    /// OPDS client. UI code must never call this on the UI thread — hand it
    /// to a loader thread and treat the result as arriving asynchronously.
    fn unit_bytes(&self, spine_index: usize) -> Result<Vec<u8>>;

    /// Cover image. `Ok(None)` means the publication declares no cover;
    /// a declared-but-unreadable cover is an `Err`, not `None` — implementors
    /// must not collapse corruption into absence.
    fn cover(&self) -> Result<Option<Resource>> {
        Ok(None)
    }

    /// Convenience: the spine item or a range error.
    fn spine_item(&self, spine_index: usize) -> Result<&SpineItem> {
        self.spine()
            .get(spine_index)
            .ok_or(ChapbookError::SpineOutOfRange(spine_index))
    }
}
