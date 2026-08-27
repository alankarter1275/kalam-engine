//! Shared vocabulary for the chapbook ereader components.
//!
//! This crate is deliberately dependency-light and paint-neutral: geometry is
//! hand-rolled `f32` (CSS px) rather than leaking `euclid`/`kurbo`/`tiny-skia`
//! types across crate boundaries. Nothing here knows about stylo, cosmic-text,
//! or any renderer.

mod book;
mod cfi;
mod credential;
mod diagnostics;
mod error;
mod font;
mod geometry;
mod input;
mod locator;
mod page;
mod panel;
mod source;

pub use book::{
    BookKind, BookMetadata, Publication, Resource, SpineItem, TextGlyph, TextLine, TocEntry,
};
pub use cfi::{Cfi, CfiStep};
pub use credential::{
    basic_authorization, Credential, CredentialKey, CredentialLookup, CredentialStore,
    EnvCredentials, Freshness, MemoryCredentials, NoCredentials,
};
pub use diagnostics::{log_to_stderr, log_to_stderr_at};
pub use error::ChapbookError;
pub use font::{
    Faces, FallbackFamilies, Fallbacks, FontReport, FontSource, GenericFamilies, Generics,
    ScriptTag,
};
pub use geometry::{EdgeSizes, Point, Rect, Rgba, Size};
pub use input::{Action, ActionOutcome, Key, KeyMap, ReadingDirection, TapZones};
pub use locator::{
    book_progression, find_quote_nearest, resolve_in_text, LayeredLocator, Locator, Quote,
    ResolvedOffset, LOCATOR_VERSION, QUOTE_CONTEXT_CHARS,
};
pub use page::{PageMetrics, PixelFormat, ReadingSettings, Rotation, Theme};
pub use panel::{
    Panel, PanelDriver, PanelInfo, PanelRect, RecordingPanel, RefreshPolicy, UpdateClass,
    UpdateToken,
};
pub use source::{Format, ReadSeek, Source, FORMAT_SNIFF_BYTES};

/// Convenience result type used across chapbook crates.
pub type Result<T> = std::result::Result<T, ChapbookError>;
