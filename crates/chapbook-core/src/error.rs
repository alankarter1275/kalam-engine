use std::path::PathBuf;

/// Unified error type across chapbook crates.
#[derive(Debug, thiserror::Error)]
pub enum ChapbookError {
    #[error("failed to open book {path}: {reason}")]
    BookOpen { path: PathBuf, reason: String },

    #[error("book is malformed: {0}")]
    BookMalformed(String),

    #[error("resource not found in book: {0}")]
    ResourceNotFound(String),

    #[error("fixed-layout EPUBs are not supported (this is a reflowable-text reading system)")]
    FixedLayoutUnsupported,

    #[error("spine index {0} out of range")]
    SpineOutOfRange(usize),

    #[error("failed to parse content document: {0}")]
    Parse(String),

    #[error("stylesheet error: {0}")]
    Style(String),

    #[error("layout error: {0}")]
    Layout(String),

    #[error("invalid CFI: {0}")]
    Cfi(String),

    /// Transport-level failure fetching remote content (a page stream, an
    /// acquisition download). Distinct from [`Self::Opds`], which is for
    /// protocol/feed-shape errors.
    #[error("network error: {0}")]
    Network(String),

    #[error("OPDS error: {0}")]
    Opds(String),

    #[error("library database error: {0}")]
    Library(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
