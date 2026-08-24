//! Shared vocabulary for the chapbook ereader components.
//!
//! This crate is deliberately dependency-light and paint-neutral: geometry is
//! hand-rolled `f32` (CSS px) rather than leaking `euclid`/`kurbo`/`tiny-skia`
//! types across crate boundaries. Nothing here knows about stylo, cosmic-text,
//! or any renderer.

mod book;
mod cfi;
mod error;
mod geometry;
mod locator;
mod page;

pub use book::{BookKind, BookMetadata, Publication, Resource, SpineItem, TocEntry};
pub use cfi::{Cfi, CfiStep};
pub use error::ChapbookError;
pub use geometry::{EdgeSizes, Point, Rect, Rgba, Size};
pub use locator::{
    book_progression, find_quote_nearest, resolve_in_text, LayeredLocator, Locator, Quote,
    ResolvedOffset, LOCATOR_VERSION, QUOTE_CONTEXT_CHARS,
};
pub use page::{PageMetrics, PixelFormat, ReadingSettings, Rotation, Theme};

/// Convenience result type used across chapbook crates.
pub type Result<T> = std::result::Result<T, ChapbookError>;
