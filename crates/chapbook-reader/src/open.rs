//! Opening a book: source and format resolution and session construction.
//!
//! kalam: upstream's library handshake (match/import/adopt, position
//! restore, stored annotations) lived here too. It is gone with the
//! library crate; a host restores its own position with
//! [`Session::goto_layered`] and paints its own marks with
//! [`Session::set_host_highlights`].

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[cfg(feature = "opds")]
use chapbook_core::CredentialStore;
use chapbook_core::{Format, PixelFormat, Publication, Result, Source};
use chapbook_paint::{FrameIntent, ImageStore};

use crate::frame::PendingDamage;
#[cfg(feature = "_image-book")]
use crate::loader::{LoadSource, Loader};
use crate::ReadingSettings;
use crate::{FontSource, Session, SessionConfig, WakerCell, DEFAULT_CACHE_BUDGET};
#[cfg(feature = "opds")]
use chapbook_opds::http::HttpClient;

/// The open publication. Text units need the concrete EPUB surface
/// (relative resource resolution, stylesheets) that deliberately isn't on
/// the `Publication` trait, so the session keeps the concrete type.
pub(crate) enum OpenBook {
    Epub(Box<chapbook_epub::Book>),
    /// Any image-per-page publication: a local CBZ, or a streamed PSE
    /// feed. Both arrive as a trait object, so neither format needs a
    /// variant of its own.
    #[cfg(feature = "_comic")]
    Comic(Arc<dyn Publication + Send + Sync>),
    /// PDFs keep their concrete type: the loader renders straight to RGBA
    /// and extracts the text layer through it.
    #[cfg(feature = "pdf")]
    Pdf(Arc<chapbook_pdf::PdfBook>),
}

impl OpenBook {
    pub(crate) fn publication(&self) -> &dyn Publication {
        match self {
            OpenBook::Epub(book) => book.as_ref(),
            #[cfg(feature = "_comic")]
            OpenBook::Comic(comic) => comic.as_ref(),
            #[cfg(feature = "pdf")]
            OpenBook::Pdf(pdf) => pdf.as_ref(),
        }
    }

    #[cfg(feature = "_image-book")]
    fn load_source(&self) -> Option<LoadSource> {
        match self {
            OpenBook::Epub(_) => None,
            #[cfg(feature = "_comic")]
            OpenBook::Comic(comic) => Some(LoadSource::Comic(comic.clone())),
            #[cfg(feature = "pdf")]
            OpenBook::Pdf(pdf) => Some(LoadSource::Pdf(pdf.clone())),
        }
    }
}

/// Ask the host's store for this catalog's credential and apply it.
///
/// Returns whether the client came away with one. Nothing here learns the
/// scheme: the store hands back a complete `Authorization` header value and
/// `opds-client` sends it verbatim, which is what lets a bearer token
/// arrive later without touching this code or the C ABI that will wrap it.
#[cfg(feature = "opds")]
fn authorize(
    client: &mut chapbook_opds::OpdsClient,
    store: &dyn CredentialStore,
    key: Option<&chapbook_core::CredentialKey>,
    freshness: chapbook_core::Freshness,
) -> bool {
    use chapbook_core::CredentialLookup;
    let Some(key) = key else {
        return false;
    };
    // The key is safe to print: it is an origin or a row id, never the
    // secret-bearing path a catalog URL may carry.
    match store.get(key, freshness) {
        CredentialLookup::Found(credential) => {
            client.set_authorization(credential.authorization);
            true
        }
        CredentialLookup::Missing => false,
        CredentialLookup::Locked => {
            log::warn!("credentials for {key} are stored but locked right now");
            false
        }
        CredentialLookup::Failed(reason) => {
            log::warn!("credential store failed for {key}: {reason}");
            false
        }
    }
}

/// Decide a format from the bytes, falling back to the name and then to
/// EPUB.
///
/// Bytes first is the point: an extension is a claim by whoever named the
/// file, and a `.epub` that is really a comic archive is a parse error
/// today. The name is consulted only when the bytes say nothing, and the
/// final fallback to EPUB preserves an existing behaviour worth keeping —
/// rbook opens an *unzipped* EPUB directory, which has no leading bytes to
/// read at all.
fn format_of_path(path: &Path) -> Format {
    let sniffed = std::fs::File::open(path).ok().and_then(|mut file| {
        let mut head = vec![0u8; chapbook_core::FORMAT_SNIFF_BYTES];
        let read = std::io::Read::read(&mut file, &mut head).ok()?;
        head.truncate(read);
        Format::sniff(&head)
    });
    sniffed
        .or_else(|| Format::from_extension(path))
        .unwrap_or(Format::Epub)
}

/// Honour a format the host stated; sniff only when it said `Guess`.
///
/// A host that resolved a `content://` URI already asked the resolver for a
/// MIME type, and that answer is better than ours — it may know things the
/// first 64 bytes cannot say.
fn resolve_format(stated: Format, head: &[u8]) -> Result<Format> {
    match stated {
        Format::Guess => Format::sniff(head).ok_or_else(|| {
            chapbook_core::ChapbookError::BookMalformed(
                "not an EPUB, CBZ or PDF: the leading bytes match no format chapbook reads".into(),
            )
        }),
        stated => Ok(stated),
    }
}

/// The first bytes of a handle, rewound afterwards so the format reader
/// gets an untouched stream.
fn peek(reader: &mut dyn chapbook_core::ReadSeek) -> Result<Vec<u8>> {
    let mut head = vec![0u8; chapbook_core::FORMAT_SNIFF_BYTES];
    let read = reader.read(&mut head)?;
    head.truncate(read);
    reader.seek(std::io::SeekFrom::Start(0))?;
    Ok(head)
}

/// The format is not compiled in. Its own function so every arm reports it
/// the same way — and `cfg`'d, because with every format built there is no
/// arm left to call it.
#[cfg(any(not(feature = "cbz"), not(feature = "pdf")))]
fn not_built(format: Format) -> chapbook_core::ChapbookError {
    chapbook_core::ChapbookError::FormatNotBuilt(format.as_str())
}

fn book_from_bytes(format: Format, bytes: Vec<u8>) -> Result<OpenBook> {
    match format {
        Format::Epub | Format::Guess => Ok(OpenBook::Epub(Box::new(
            chapbook_epub::Book::from_bytes(bytes)?,
        ))),
        #[cfg(feature = "cbz")]
        Format::Cbz => Ok(OpenBook::Comic(Arc::new(
            chapbook_cbz::ComicBook::from_bytes(bytes)?,
        ))),
        #[cfg(feature = "pdf")]
        Format::Pdf => Ok(OpenBook::Pdf(Arc::new(chapbook_pdf::PdfBook::from_bytes(
            bytes,
        )?))),
        #[cfg(not(feature = "cbz"))]
        Format::Cbz => Err(not_built(format)),
        #[cfg(not(feature = "pdf"))]
        Format::Pdf => Err(not_built(format)),
    }
}

fn book_from_reader(format: Format, reader: Box<dyn chapbook_core::ReadSeek>) -> Result<OpenBook> {
    match format {
        Format::Epub | Format::Guess => {
            Ok(OpenBook::Epub(Box::new(chapbook_epub::Book::read(reader)?)))
        }
        #[cfg(feature = "cbz")]
        Format::Cbz => Ok(OpenBook::Comic(Arc::new(chapbook_cbz::ComicBook::read(
            reader,
        )?))),
        #[cfg(feature = "pdf")]
        Format::Pdf => Ok(OpenBook::Pdf(Arc::new(chapbook_pdf::PdfBook::read(
            reader,
        )?))),
        #[cfg(not(feature = "cbz"))]
        Format::Cbz => Err(not_built(format)),
        #[cfg(not(feature = "pdf"))]
        Format::Pdf => Err(not_built(format)),
    }
}

fn book_at_path(format: Format, path: &Path) -> Result<OpenBook> {
    match format {
        Format::Epub | Format::Guess => {
            Ok(OpenBook::Epub(Box::new(chapbook_epub::Book::open(path)?)))
        }
        #[cfg(feature = "cbz")]
        Format::Cbz => Ok(OpenBook::Comic(Arc::new(chapbook_cbz::ComicBook::open(
            path,
        )?))),
        #[cfg(feature = "pdf")]
        Format::Pdf => Ok(OpenBook::Pdf(Arc::new(chapbook_pdf::PdfBook::open(path)?))),
        #[cfg(not(feature = "cbz"))]
        Format::Cbz => Err(not_built(format)),
        #[cfg(not(feature = "pdf"))]
        Format::Pdf => Err(not_built(format)),
    }
}

impl Session {
    /// Open a book from a path. (kalam: upstream also took an `http(s)://`
    /// OPDS URL here and matched local books into its library; both are
    /// gone. A book opens at its beginning, and the host hands back the
    /// place it stored with [`Session::goto_layered`].)
    ///
    /// The format comes from the *bytes*, not the extension — see
    /// [`chapbook_core::Format::sniff`]. A book whose name lies about it
    /// opens correctly. A host with no path at all passes a
    /// [`Source`] to [`Session::open_with`] instead.
    ///
    /// `fonts` is required rather than defaulted. A session that finds no
    /// faces lays out, renders, paints and *conforms* — it just paginates
    /// every book to a single blank page, taking navigation, search and
    /// the table of contents down with it — and fontdb has no Android, iOS
    /// or wasm branch, so "no faces" is the ordinary case on three
    /// platforms. Desktop shells want [`FontSource::host`]; anything whose
    /// output is compared against a golden wants
    /// [`FontSource::embedded`]. See `chapbook_core::font`.
    ///
    /// Credentials default to none; a shell that can reach a host store —
    /// or just an environment — passes one through
    /// [`Session::open_with`].
    pub fn open(source: &str, fonts: FontSource) -> Result<Session> {
        Session::open_with(source, SessionConfig::new(fonts))
    }

    /// [`Session::open`], with the source typed and the host's capabilities
    /// supplied explicitly.
    ///
    /// The form every non-desktop shell wants, and the one the FFI will
    /// wrap: nothing in here is reached for behind the caller's back.
    ///
    /// `source` accepts anything that becomes a [`Source`] — a `&str` or
    /// `String` is a path, a `PathBuf` is a file, and [`Source::bytes`] /
    /// [`Source::reader`] are for hosts that have no path to give. An
    /// `http(s)://` string is refused: the catalog support is gone.
    ///
    /// kalam: nothing is written anywhere. The session remembers nothing
    /// between runs; the host keeps positions, marks and settings.
    pub fn open_with(source: impl Into<Source>, config: SessionConfig) -> Result<Session> {
        // kalam: timed, reported at `info` — see `layout_text_unit`.
        let clock = std::time::Instant::now();
        let source = source.into();
        let SessionConfig {
            fonts,
            // Only the OPDS path has anything to authenticate; a build
            // without it still takes the store, so a shell's construction
            // code does not change with the feature set.
            #[cfg_attr(not(feature = "opds"), allow(unused_variables))]
            credentials,
            #[cfg(feature = "opds")]
            transport,
            cache_budget,
        } = config;

        let book: OpenBook = match source {
            Source::Url(_) => {
                // kalam: the OPDS page stream needed the library's cache
                // directory; both are gone.
                return Err(chapbook_core::ChapbookError::FormatNotBuilt("OPDS"));
            }
            // A source with no file behind it: bytes a host already holds,
            // or a handle it resolved from a `content://` URI or a
            // security-scoped bookmark.
            Source::Bytes { format, bytes } => {
                let format = resolve_format(format, &bytes)?;
                book_from_bytes(format, bytes)?
            }
            Source::Reader { format, mut reader } => {
                let format = resolve_format(format, &peek(reader.as_mut())?)?;
                book_from_reader(format, reader)?
            }
            Source::Path(ref source_path) => {
                let path = source_path.as_path();
                book_at_path(format_of_path(path), path)?
            }
        };

        let title = book
            .publication()
            .metadata()
            .title
            .clone()
            .unwrap_or_else(|| "chapbook".to_string());
        let waker: WakerCell = Arc::new(Mutex::new(None));
        #[cfg(feature = "_image-book")]
        let loader = book.load_source().map(|source| {
            let cell = waker.clone();
            Loader::spawn(
                source,
                Arc::new(move || {
                    if let Some(wake) = &*cell.lock().unwrap() {
                        wake();
                    }
                }),
            )
        });
        // kalam: the built-in defaults; a host applies its own settings
        // with `set_settings` before the first layout.
        let settings = ReadingSettings::default();

        let before_fonts = clock.elapsed().as_millis();
        let (fonts, font_report) = chapbook_layout::build_font_system(&fonts)?;
        let total = clock.elapsed().as_millis();
        log::info!(
            "opened {title:?} in {total} ms (fonts: {} faces, {} ms)",
            font_report.faces,
            total - before_fonts
        );

        Ok(Session {
            book,
            title,
            fonts,
            font_report,
            renderer: chapbook_render_tinyskia::Renderer::new(),
            settings,
            metrics: None,
            view: None,
            units: HashMap::new(),
            cache_budget: cache_budget.unwrap_or(DEFAULT_CACHE_BUDGET),
            use_clock: 0,
            registered_fonts: HashSet::new(),
            spine: 0,
            page: 0,
            pending_offset: None,
            #[cfg(feature = "_image-book")]
            loader,
            #[cfg(feature = "_image-book")]
            load_errors: HashMap::new(),
            waker,
            events: Vec::new(),
            // Seeded with where the book actually opens, so the first
            // drain reports a move only if one happened. A restored
            // position resolves later, in `frame`, and is a real move.
            reported_position: crate::Position { spine: 0, page: 0 },
            reported_finished: false,
            selection: None,
            empty_images: ImageStore::default(),
            pending: FrameIntent::default(),
            pending_damage: PendingDamage::default(),
            painted_selection: None,
            pixel_format: PixelFormat::default(),
            back_stack: Vec::new(),
            pending_anchor: None,
            char_counts: std::cell::OnceCell::new(),
            unit_text_cache: std::cell::RefCell::new(None),
            host_highlights: Vec::new(),
        })
    }
}
