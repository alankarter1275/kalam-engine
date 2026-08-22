//! EPUB container reading for chapbook.
//!
//! Wraps [`rbook`] for OCF/OPF/spine/TOC handling and adds what a reading
//! system needs on top: an owned, simple view of the book structure (rbook's
//! API is borrow-heavy), resource resolution relative to a chapter, and
//! fixed-layout detection (rejected — chapbook is reflowable-only).
//!
//! IDPF/Adobe font de-obfuscation lands in M5.

mod book;
mod href;

pub use book::Book;
pub use href::resolve_href;

// The format-neutral book model (Publication, BookMetadata, SpineItem,
// TocEntry, Resource) lives in chapbook-core and is deliberately NOT
// re-exported here: a single import path keeps `grep chapbook_epub` an honest
// map of who actually depends on EPUB.
