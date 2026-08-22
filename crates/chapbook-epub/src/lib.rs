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

pub use book::{Book, BookMetadata, Resource, SpineItem, TocEntry};
pub use href::resolve_href;
