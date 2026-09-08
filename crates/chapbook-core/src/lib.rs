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
pub use page::{PageMetrics, ReadingSettings, Rotation, Theme};
pub use source::{Format, ReadSeek, Source, FORMAT_SNIFF_BYTES};

/// The panel vocabulary, from [mezzotint].
///
/// These are the three panel types the page model itself speaks:
/// `FrameIntent::update_class` names an [`UpdateClass`],
/// `DisplayList::dither_regions` produces [`PanelRect`]s, and a session
/// rasterizes for a [`PixelFormat`]. They are re-exported so those types
/// still come from one place.
///
/// The machinery *underneath* the seam — `Panel`, `PanelDriver`,
/// `RecordingPanel`, `Source`, `Update` — is deliberately not re-exported.
/// A shell driving a panel is a consumer of mezzotint and should say so;
/// and `mezzotint::Source` is a window onto a pixel buffer, which is not
/// what [`Source`] means here.
pub use mezzotint::{PanelRect, PixelFormat, UpdateClass};

/// Convenience result type used across chapbook crates.
pub type Result<T> = std::result::Result<T, ChapbookError>;
