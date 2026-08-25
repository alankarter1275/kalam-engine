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

    /// The format is one chapbook implements, but this build left it out.
    ///
    /// Distinct from a malformed or unrecognised book: the file is fine and
    /// a differently-configured build would open it. Device builds drop
    /// whole format stacks (see chapbook-reader's `cbz`/`pdf`/`opds`
    /// features), and a reader that silently fell through to the EPUB path
    /// would report a parse failure for a perfectly good PDF.
    #[error("{0} support is not compiled into this build")]
    FormatNotBuilt(&'static str),

    #[error("spine index {0} out of range")]
    SpineOutOfRange(usize),

    #[error("failed to parse content document: {0}")]
    Parse(String),

    #[error("stylesheet error: {0}")]
    Style(String),

    #[error("layout error: {0}")]
    Layout(String),

    /// The font source a session was given cannot produce a usable font
    /// system — in practice, it found no faces at all. Its own error rather
    /// than a `Layout` one because it is a construction-time
    /// misconfiguration on the caller's side, and because the failure it
    /// replaces was silent: a fontless session lays out, renders, paints
    /// and conforms, one blank page per book.
    #[error("font error: {0}")]
    Font(String),

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

    /// A host credential store refused or failed. Distinct from
    /// [`Self::Opds`]: the server was never asked, or asked and answered
    /// 401 with nothing to retry with. See `chapbook_core::credential`.
    #[error("credential error: {0}")]
    Credential(String),

    /// A display panel rejected an update, or could not be reached.
    #[error("panel error: {0}")]
    Panel(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
