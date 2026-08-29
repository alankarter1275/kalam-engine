//! The shared reading session: everything a viewer shell needs that isn't
//! windowing.
//!
//! [`Session`] owns the open publication (EPUB, local CBZ, or an OPDS-PSE
//! stream — dispatched by [`Session::open`]), the font system, per-chapter
//! layout and image caches, reading settings, the selection, and the
//! library glue (import/match on open, layered-locator persistence on
//! save). Shells — winit, GTK, anything with a keyboard and a pixel
//! buffer — translate input events into `Session` calls and blit the
//! [`Session::render`] result. A shell that rasterizes for itself takes
//! [`Session::frame`] instead and never touches tiny-skia.
//!
//! Text units run the full dom→stylo→layout pipeline; comic units fabricate
//! a one-page [`ChapterLayout`] around a single scaled image fragment, so
//! navigation, the char map, and position persistence are one code path.
//!
//! `docs/SHELLS.md` is the contract from the shell's side — the loop, the
//! loader rule, metrics, position, panel policy — and
//! [`conformance`] is that contract as runnable assertions.
//!
//! Blocking caveat, inherited from `Publication::unit_bytes`: layout of a
//! not-yet-cached unit may block on I/O (seconds, for a cold PSE page).
//! These shells are dev harnesses and call it on the UI thread anyway —
//! the documented debt; a loader thread slots in here, not in the shells.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub mod conformance;

#[cfg(feature = "_image-book")]
mod loader;
#[cfg(feature = "_image-book")]
use loader::{DecodedUnit, LoadSource, Loader};

use chapbook_core::{
    Action, ActionOutcome, BookKind, CredentialStore, FontReport, FontSource, Format, Locator,
    NoCredentials, PageMetrics, PixelFormat, Point, Publication, ReadingDirection, ReadingSettings,
    Rect, Result, Rotation, Source, TocEntry,
};
// Annotations are the only thing that captures a locator, resolves one
// against unit text, or paints a stored colour.
#[cfg(feature = "library")]
use chapbook_core::{resolve_in_text, LayeredLocator, Rgba, SpineItem};
use chapbook_layout::{cascade, dom, ChapterLayout};
#[cfg(feature = "library")]
use chapbook_library::AnnotationKind;
use chapbook_paint::{Frame, FrameIntent, ImageStore, Selection};

// Everything a shell needs to consume what the session produces, so it
// depends on chapbook-reader alone and can't skew versions with it: the
// display-list vocabulary, the font database its glyph runs name faces
// in, and the bundled CPU backend.
pub use chapbook_core;
// Annotation kinds and library records surface in this crate's own API.
#[cfg(feature = "library")]
pub use chapbook_library;
pub use chapbook_paint;
pub use chapbook_render_tinyskia;
pub use chapbook_render_tinyskia::tiny_skia;
pub use cosmic_text;
// A shell that owns its networking implements this, so it has to be
// nameable from here — the same rule as the display-list vocabulary above.
#[cfg(feature = "opds")]
pub use chapbook_opds::http::{HttpClient, HttpError, HttpRequest, HttpResponse};

/// The open publication. Text units need the concrete EPUB surface
/// (relative resource resolution, stylesheets) that deliberately isn't on
/// the `Publication` trait, so the session keeps the concrete type.
enum OpenBook {
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
    fn publication(&self) -> &dyn Publication {
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

/// Waker slot shared with the loader thread; the shell installs its wakeup
/// (an event-loop proxy, a main-context poke) after the session exists.
type WakerCell = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

/// Metrics-independent record of a loaded image-book unit (the pixels are
/// in the session's image store).
struct LoadedUnit {
    width: u32,
    height: u32,
    text: Vec<chapbook_core::TextLine>,
    natural: (f32, f32),
}

/// Where a settings change should stick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsScope {
    /// The reader's default, for every book without an override.
    Global,
    /// This book only. It keeps these settings when the default changes.
    ThisBook,
}

/// One search hit, in the same locator space positions and annotations
/// live in — so a hit feeds straight into [`Session::goto`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Unit and offset of the match's first character.
    pub locator: Locator,
    /// Offset just past the match, within the same unit.
    pub end: u32,
    /// The match with a little text on either side, whitespace collapsed,
    /// for a results list.
    pub context: String,
    /// Char range of the match within `context`.
    pub match_range: (u32, u32),
}

// Annotations are the library's: without a place to store them there
// is nothing for these to describe. See the `library` feature.
#[cfg(feature = "library")]
/// A stored highlight resolved into the open book's locator space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// Library annotation id — the handle for [`Session::remove_highlight`].
    pub id: i64,
    pub spine: usize,
    /// Locator offsets within the unit, `[start, end)`.
    pub start: u32,
    pub end: u32,
    /// The text as captured, for a highlight list.
    pub text: Option<String>,
    /// Stored color as written (`#rrggbb`); `None` follows the theme.
    pub color: Option<String>,
}

#[cfg(feature = "library")]
/// One stored annotation as recorded — enough to list every mark in a book
/// without resolving any of them against unit text.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationSummary {
    pub id: i64,
    pub kind: AnnotationKind,
    /// Unit the annotation resolves against in this book.
    pub spine_index: usize,
    /// Whole-book progression of its start, for ordering and for showing
    /// where in the book it sits.
    pub progression: f64,
    /// The quoted text for a highlight, the body for a note.
    pub text: Option<String>,
    pub color: Option<String>,
}

#[cfg(feature = "library")]
/// An annotation as the library stores it, plus where it lands in this
/// book's spine. Resolution into offsets waits for the unit's text.
struct StoredAnnotation {
    id: i64,
    kind: AnnotationKind,
    /// Spine item the endpoints resolve against: by href where the book
    /// still has that item, else the stored index.
    target: usize,
    /// The stored href matched a spine item — only then, and only in the
    /// same edition, are the exact offsets trustworthy.
    href_matched: bool,
    start: LayeredLocator,
    /// Ranged kinds (highlights, notes) have an end; a bookmark is a point.
    end: Option<LayeredLocator>,
    text: Option<String>,
    color: Option<String>,
}

/// Book-wide char counts around the current unit — what
/// `LayeredLocator::capture` needs beyond the offset itself.
#[cfg(feature = "library")]
struct UnitCharContext {
    /// The current unit's locator text.
    text: String,
    /// Chars in the spine items before it.
    prior: u64,
    /// Chars across the whole book.
    total: u64,
}

/// Where changes have landed since the last frame was taken.
///
/// Kept apart from the pending [`FrameIntent`] on purpose. Intent is
/// ordered by how disturbing a change is and collapses to the strongest
/// one; damage is a union and collapses to "everywhere" the moment any
/// change cannot say where it went. Folding the two together made damage
/// hostage to that ordering, so a highlight discarded the region a live
/// selection had already named.
#[derive(Debug, Clone, Copy, Default)]
struct PendingDamage {
    /// Some change since the last frame could not name its region.
    unstated: bool,
    /// Union of the regions that could, in page coordinates.
    region: Option<Rect>,
}

/// Where the reader is: which spine unit, and which page inside it.
///
/// The pair, never the page alone. `next_page` crosses into the next unit
/// by resetting the page to 0, so a shell comparing `page()` across a turn
/// reads a successful move as "did not move" — and since most books open on
/// a single-page cover, it reads that on the very first turn. That is not
/// hypothetical: it is what stopped the fbdev shell dead on its first run
/// against real hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub spine: usize,
    pub page: usize,
}

/// One open book and everything needed to read it.
pub struct Session {
    book: OpenBook,
    title: String,
    fonts: cosmic_text::FontSystem,
    /// What the font source actually produced, kept so a shell can say so.
    font_report: FontReport,
    renderer: chapbook_render_tinyskia::Renderer,
    settings: ReadingSettings,
    metrics: Option<PageMetrics>,
    layouts: HashMap<usize, ChapterLayout>,
    images: HashMap<usize, ImageStore>,
    /// Bytes the two caches above may hold between them.
    cache_budget: usize,
    /// Last time each unit was used, for eviction order. A counter rather
    /// than a clock: monotonic, cheap, and it cannot go backwards when the
    /// host's clock does.
    used_at: HashMap<usize, u64>,
    /// Ticks the counter above.
    use_clock: u64,
    registered_fonts: HashSet<String>,
    spine: usize,
    page: usize,
    /// Restored char offset, turned into a page once the unit lays out.
    ///
    /// Paired with the unit it was captured in, and *only* applied while
    /// the reader is still there. A restore lands in `frame()`, because
    /// the offset cannot become a page until the unit has laid out, and
    /// nothing obliges a shell to paint before it navigates: a batched
    /// turn, or a harness, can walk the whole book first. Unpaired, the
    /// offset then resolved against whatever unit the reader had reached
    /// and moved them backwards inside it — see
    /// `a_restore_does_not_follow_the_reader_into_another_unit`.
    pending_offset: Option<(usize, u32)>,
    /// Worker for image-book units (comics, PDFs); `None` for EPUBs.
    #[cfg(feature = "_image-book")]
    loader: Option<Loader>,
    /// Metrics-independent metadata of loaded units (pixels live in
    /// `images`, which image books never clear on relayout).
    loaded_units: HashMap<usize, LoadedUnit>,
    /// Units the loader failed on, so a retry isn't queued every frame.
    #[cfg(feature = "_image-book")]
    load_errors: HashMap<usize, String>,
    /// Units currently showing a placeholder page (relaid once loaded).
    placeholders: HashSet<usize>,
    waker: WakerCell,
    /// Selection anchor and cursor as locator offsets (unordered).
    selection: Option<(u32, u32)>,
    #[cfg(feature = "library")]
    library: Option<chapbook_library::Library>,
    /// Where the library lives, so [`Session::suspend`] can close it and a
    /// later access can open it again.
    #[cfg(feature = "library")]
    library_dir: Option<std::path::PathBuf>,
    /// Set by [`Session::suspend`]: the database is deliberately closed and
    /// the next access should reopen rather than treat `None` as "this
    /// platform has no library".
    #[cfg(feature = "library")]
    suspended: bool,
    #[cfg(feature = "library")]
    book_id: OpenedBookId,
    /// Annotations as stored, awaiting resolution against unit text.
    #[cfg(feature = "library")]
    stored: Vec<StoredAnnotation>,
    /// Resolved per unit, cached: the locator space of a unit doesn't move
    /// under relayout, so this survives font-size and theme changes.
    #[cfg(feature = "library")]
    resolved_highlights: HashMap<usize, Vec<Highlight>>,
    /// The open file is the edition the positions were captured against.
    #[cfg(feature = "library")]
    same_edition: bool,
    /// Handed out for units with no images of their own, so
    /// [`Session::image_store`] can return a reference either way.
    empty_images: ImageStore,
    /// What has changed since the last frame was taken.
    pending: FrameIntent,
    pending_damage: PendingDamage,
    /// The selection as of the last frame — the other half of a selection
    /// change, needed to damage what it used to cover.
    painted_selection: Option<(u32, u32)>,
    /// What the target panel can show; applied to rendered pixels.
    pixel_format: PixelFormat,
    /// Per-unit hyperlinks in locator space, filled as units lay out.
    links: HashMap<usize, Vec<dom::Link>>,
    /// Where jumps came from, so a footnote can be returned from. Only
    /// jumps push; ordinary page turns don't.
    back_stack: Vec<Locator>,
    /// Fragment to land on once the target unit has laid out — the
    /// anchor-flavored sibling of `pending_offset`, unit-paired for the
    /// same reason.
    pending_anchor: Option<(usize, String)>,
}

/// The library's handle on the open book.
///
/// `Option<Infallible>` without the `library` feature: always `None`, zero
/// sized, and impossible to construct — so the open path keeps one shape
/// instead of growing a `cfg` at every step that merely passes it along.
#[cfg(feature = "library")]
type OpenedBookId = Option<chapbook_library::BookId>;
#[cfg(not(feature = "library"))]
type OpenedBookId = Option<std::convert::Infallible>;

/// What a session keeps cached when the host does not say.
///
/// Chosen to hold a comfortable working set of comic pages — a 1600x2400
/// page is 15.4 MB decoded, so this is about twelve of them — while being
/// far below what any target would be killed for. It is a backstop, not a
/// recommendation: a phone should pass its own number, and before this
/// existed the answer was "everything, forever", which measured at 676 MB
/// after forty pages of a comic with no way to give any of it back.
pub const DEFAULT_CACHE_BUDGET: usize = 192 * 1024 * 1024;

/// How far one [`Action::FontUp`] or [`Action::FontDown`] moves the base
/// font size, in CSS px.
///
/// The magnitude is the engine's rather than the shell's on purpose: the
/// action carries a direction and nothing else, so every shell steps by
/// the same amount and a reader who changes device finds the same ladder.
/// Two px over the 10–40 range `adjust_font` clamps to is sixteen stops,
/// which is fine-grained enough to land on a comfortable size and coarse
/// enough that reaching either end is a few presses.
pub const FONT_STEP_PX: f32 = 2.0;

/// What a session needs from its host, instead of assuming a desktop.
///
/// Every field here replaces something the session used to reach for on its
/// own — installed fonts, environment variables — and each of those
/// assumptions holds on exactly one of the platforms chapbook targets.
/// Adding to this struct is how the next one arrives (`docs/FFI.md` lists
/// the transport, the library directory and a cache budget as the ones
/// still outstanding), which is the point of it being a struct: the
/// alternative is changing `open`'s signature once per capability.
pub struct SessionConfig {
    /// Where fonts come from. Required, and required for the reasons in
    /// [`Session::open`].
    pub fonts: FontSource,
    /// Where secrets come from. Defaults to [`NoCredentials`], because the
    /// engine has no business guessing that a machine has an environment,
    /// a Keychain, or anything at all — a shell says. Desktop shells and
    /// the CLI pass `EnvCredentials`; see `chapbook_core::credential`.
    pub credentials: Arc<dyn CredentialStore>,
    /// How bytes are fetched. `None` uses `opds-client`'s bundled `ureq`
    /// transport, which is right for a desktop process and wrong
    /// everywhere else: a host that reaches the network outside
    /// `URLSession` gives up background transfer, the system trust store
    /// and App Transport Security, and a browser has no sockets at all.
    ///
    /// Shared rather than owned because a host has *one* of these — a
    /// single background `URLSession` whose value is that transfers
    /// outlive the process — and the session builds a client per
    /// authentication attempt.
    ///
    /// Present only with the `opds` feature: without it there is nothing
    /// to fetch, and the whole TLS stack is out of the build.
    #[cfg(feature = "opds")]
    pub transport: Option<Arc<dyn HttpClient>>,
    /// Where the library, the managed book copies, the covers and the PSE
    /// page cache live. `None` asks
    /// [`Library::default_dir`](chapbook_library::Library::default_dir),
    /// which knows the convention for each desktop platform and refuses to
    /// guess anywhere else.
    ///
    /// A sandboxed host knows its own answer and nothing else can: Android
    /// hands an app `context.getFilesDir()`, iOS wants
    /// `Library/Application Support`, and a browser has no filesystem at
    /// all. None of those are reachable through an environment variable.
    pub library_dir: Option<std::path::PathBuf>,
    /// How many bytes of laid-out chapters and decoded page images a
    /// session may keep. `None` takes [`DEFAULT_CACHE_BUDGET`].
    ///
    /// A host that knows its own limits should say. Android kills a
    /// process for exceeding them and hands `onTrimMemory` no argument
    /// about it; the number a device can afford is not one the engine can
    /// guess from inside.
    pub cache_budget: Option<usize>,
}

impl SessionConfig {
    pub fn new(fonts: FontSource) -> SessionConfig {
        SessionConfig {
            fonts,
            credentials: Arc::new(NoCredentials),
            #[cfg(feature = "opds")]
            transport: None,
            library_dir: None,
            cache_budget: None,
        }
    }

    pub fn with_credentials(mut self, credentials: Arc<dyn CredentialStore>) -> SessionConfig {
        self.credentials = credentials;
        self
    }

    /// Fetch through the host's networking instead of the bundled `ureq`.
    #[cfg(feature = "opds")]
    pub fn with_transport(mut self, transport: Arc<dyn HttpClient>) -> SessionConfig {
        self.transport = Some(transport);
        self
    }

    /// Keep the library somewhere this host chose.
    pub fn with_library_dir(mut self, dir: impl Into<std::path::PathBuf>) -> SessionConfig {
        self.library_dir = Some(dir.into());
        self
    }

    /// Cap what the session's caches may hold, in bytes.
    pub fn with_cache_budget(mut self, bytes: usize) -> SessionConfig {
        self.cache_budget = Some(bytes);
        self
    }
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("SessionConfig");
        out.field("fonts", &self.fonts)
            .field("credentials", &"<dyn CredentialStore>")
            .field("library_dir", &self.library_dir)
            .field("cache_budget", &self.cache_budget);
        #[cfg(feature = "opds")]
        out.field(
            "transport",
            match &self.transport {
                Some(_) => &"<dyn HttpClient>",
                None => &"bundled ureq",
            },
        );
        out.finish()
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
    /// Open a book from a path or an `http(s)://` OPDS URL (resolved to a
    /// PSE page stream). Local books are matched into the library
    /// (fingerprint, then identifier for replaced editions, else imported)
    /// and their stored position restored; streams skip the library (no
    /// local file to fingerprint) but honor `pse:lastRead`.
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
    /// `String` still means what it always did (`http(s)://` is a catalog,
    /// anything else a path), a `PathBuf` is a file, and
    /// [`Source::bytes`] / [`Source::reader`] are for hosts that have no
    /// path to give: an Android `content://` URI resolved to a file
    /// descriptor, an iOS security-scoped file, a WASM `ArrayBuffer`.
    ///
    /// Only path sources reach the library. Bytes and handles have no file
    /// to record and no stable identity to key a position on, so they open
    /// at the beginning every time — the custody question in
    /// `docs/PLATFORM.md`, not something a source type settles.
    pub fn open_with(source: impl Into<Source>, config: SessionConfig) -> Result<Session> {
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
            library_dir,
            cache_budget,
        } = config;

        // Resolved once, and used for the library and the page cache both.
        // A platform with no default is not a failure to open a book: the
        // session reads on without a library, exactly as it does when the
        // database itself cannot be opened.
        #[cfg(feature = "library")]
        let library_dir = match library_dir {
            Some(dir) => Some(dir),
            None => chapbook_library::Library::default_dir()
                .map_err(|e| log::warn!("reading without a library: {e}"))
                .ok(),
        };
        #[cfg(not(feature = "library"))]
        let _ = library_dir;
        #[cfg(feature = "library")]
        let mut library = library_dir.as_ref().and_then(|dir| {
            chapbook_library::Library::open(dir)
                .map_err(|e| log::warn!("library unavailable: {e}"))
                .ok()
        });

        #[cfg_attr(not(feature = "library"), allow(unused_variables))]
        let (book, book_id, start_spine, pending_offset, same_edition): (
            OpenBook,
            OpenedBookId,
            usize,
            Option<u32>,
            bool,
        ) = match source {
            Source::Url(_source) => {
                #[cfg(not(feature = "opds"))]
                return Err(chapbook_core::ChapbookError::FormatNotBuilt("OPDS"));
                #[cfg(feature = "opds")]
                {
                    let source = _source.as_str();
                    use chapbook_core::Freshness;

                    // Keyed by origin, not by the URL: the path may carry a
                    // per-user API key, and a catalog that moves its path must
                    // not lose its login. See `chapbook_core::credential`.
                    let key = chapbook_core::CredentialKey::http_origin(source);
                    // Unlike the library, a page stream cannot do without
                    // this: every page is a fetch that has to land somewhere.
                    let Some(cache) = library_dir.as_ref().map(|dir| dir.join("pse-cache")) else {
                        return Err(chapbook_core::ChapbookError::Library(
                            "no library location for the page cache; pass \
                         SessionConfig::with_library_dir"
                                .into(),
                        ));
                    };
                    let store = credentials.as_ref();

                    // One transport, however many clients the auth flow needs.
                    let http: Arc<dyn HttpClient> = match transport {
                        Some(host) => host,
                        #[cfg(feature = "ureq")]
                        None => Arc::new(chapbook_opds::UreqHttp::new()),
                        #[cfg(not(feature = "ureq"))]
                        None => {
                            return Err(chapbook_core::ChapbookError::Network(
                                "this build has no bundled HTTP transport; pass one with \
                             SessionConfig::with_transport"
                                    .into(),
                            ))
                        }
                    };

                    let mut client = chapbook_opds::OpdsClient::new(http.clone());
                    authorize(&mut client, store, key.as_ref(), Freshness::Cached);
                    let opened = chapbook_opds::StreamedComic::open(client, source, &cache);

                    // One retry, and only on a 401. `Freshness::Renewed` is
                    // what makes an expiring secret work: a store backed by a
                    // refreshable token can produce a new one here, and a
                    // store that cannot says so by returning nothing, which
                    // costs exactly one skipped retry. Prompting is not our
                    // job — the error carries the server's Authentication
                    // Document so the shell can do it.
                    let comic = match opened {
                        Err(chapbook_opds::OpdsError::AuthRequired(doc)) => {
                            let mut retry = chapbook_opds::OpdsClient::new(http.clone());
                            if !authorize(&mut retry, store, key.as_ref(), Freshness::Renewed) {
                                return Err(chapbook_opds::to_chapbook_error(
                                    chapbook_opds::OpdsError::AuthRequired(doc),
                                ));
                            }
                            chapbook_opds::StreamedComic::open(retry, source, &cache)
                                .map_err(chapbook_opds::to_chapbook_error)?
                        }
                        other => other.map_err(chapbook_opds::to_chapbook_error)?,
                    };
                    let resume = comic.resume_page().unwrap_or(0);
                    (OpenBook::Comic(Arc::new(comic)), None, resume, None, true)
                }
            }

            // A source with no file behind it: bytes a host already holds,
            // or a handle it resolved from a `content://` URI. Neither
            // reaches the library — there is no path to record and no
            // stable identity to key a position on. Closing that is the
            // custody question in `docs/PLATFORM.md` (persist a bookmark,
            // not a copy), which a source type cannot answer alone.
            Source::Bytes { format, bytes } => {
                let format = resolve_format(format, &bytes)?;
                (book_from_bytes(format, bytes)?, None, 0, None, true)
            }
            Source::Reader { format, mut reader } => {
                let format = resolve_format(format, &peek(reader.as_mut())?)?;
                (book_from_reader(format, reader)?, None, 0, None, true)
            }

            Source::Path(ref source_path) => {
                let path = source_path.as_path();
                let book = book_at_path(format_of_path(path), path)?;

                // Matching a book into the library, restoring where the
                // reader was, and importing it if it is new: all of it is
                // the library's, and without one the book simply opens at
                // the beginning.
                #[cfg(not(feature = "library"))]
                let (book_id, start_spine, pending_offset, same_edition) = (None, 0, None, true);

                #[cfg(feature = "library")]
                let mut same_edition = true;
                #[cfg(feature = "library")]
                let book_id = library.as_mut().and_then(|lib| {
                    let fingerprint = chapbook_library::Library::fingerprint_of_file(path).ok()?;
                    if let Ok(Some(id)) = lib.find_by_fingerprint(&fingerprint) {
                        return Some(id);
                    }
                    if let Some(identifier) = &book.publication().metadata().identifier {
                        if let Ok(Some(id)) = lib.find_by_identifier(identifier) {
                            same_edition = false;
                            let _ = lib.update_edition(id, path);
                            return Some(id);
                        }
                    }
                    lib.import(path, book.publication()).ok()
                });

                #[cfg(feature = "library")]
                let (start_spine, pending_offset) = match (&library, book_id) {
                    (Some(lib), Some(id)) => match lib.position(id) {
                        Ok(Some(stored)) => {
                            let (locator, tier) = chapbook_library::restore_position(
                                &stored.locator,
                                same_edition,
                                book.publication().spine(),
                                |i| unit_locator_text(book.publication(), i),
                            );
                            log::info!("resuming at unit {} ({tier:?})", locator.spine_index + 1);
                            (locator.spine_index, Some(locator.char_offset))
                        }
                        _ => (0, None),
                    },
                    _ => (0, None),
                };
                (book, book_id, start_spine, pending_offset, same_edition)
            }
        };

        // Highlights load with the book; endpoints resolve lazily, per
        // unit, once that unit's locator text is available.
        #[cfg(feature = "library")]
        let stored = match (&library, book_id) {
            (Some(lib), Some(id)) => lib
                .annotations(id)
                .map_err(|e| log::warn!("failed to read annotations: {e}"))
                .unwrap_or_default()
                .into_iter()
                .map(|a| {
                    let by_href = book
                        .publication()
                        .spine()
                        .iter()
                        .position(|s: &SpineItem| s.href == a.start.spine_href);
                    let len = book.publication().spine().len();
                    StoredAnnotation {
                        id: a.id,
                        kind: a.kind,
                        target: by_href
                            .unwrap_or(a.start.spine_index)
                            .min(len.saturating_sub(1)),
                        href_matched: by_href.is_some(),
                        start: a.start,
                        end: a.end,
                        text: a.text,
                        color: a.color,
                    }
                })
                .collect(),
            _ => Vec::new(),
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
        // The book's override if it has one, else the reader's default,
        // else the built-in defaults. Without a library there is nowhere
        // for an override to have been stored, so the defaults it is.
        #[cfg(feature = "library")]
        let settings = library
            .as_ref()
            .map(|lib| lib.effective_settings(book_id))
            .unwrap_or_default();
        #[cfg(not(feature = "library"))]
        let settings = ReadingSettings::default();

        let (fonts, font_report) = chapbook_layout::build_font_system(&fonts)?;

        Ok(Session {
            book,
            title,
            fonts,
            font_report,
            renderer: chapbook_render_tinyskia::Renderer::new(),
            settings,
            metrics: None,
            layouts: HashMap::new(),
            images: HashMap::new(),
            cache_budget: cache_budget.unwrap_or(DEFAULT_CACHE_BUDGET),
            used_at: HashMap::new(),
            use_clock: 0,
            registered_fonts: HashSet::new(),
            spine: start_spine,
            page: 0,
            pending_offset: pending_offset.map(|offset| (start_spine, offset)),
            #[cfg(feature = "_image-book")]
            loader,
            loaded_units: HashMap::new(),
            #[cfg(feature = "_image-book")]
            load_errors: HashMap::new(),
            placeholders: HashSet::new(),
            waker,
            selection: None,
            #[cfg(feature = "library")]
            library,
            #[cfg(feature = "library")]
            library_dir,
            #[cfg(feature = "library")]
            suspended: false,
            #[cfg(feature = "library")]
            book_id,
            #[cfg(feature = "library")]
            stored,
            #[cfg(feature = "library")]
            resolved_highlights: HashMap::new(),
            #[cfg(feature = "library")]
            same_edition,
            empty_images: ImageStore::default(),
            pending: FrameIntent::default(),
            pending_damage: PendingDamage::default(),
            painted_selection: None,
            pixel_format: PixelFormat::default(),
            links: HashMap::new(),
            back_stack: Vec::new(),
            pending_anchor: None,
        })
    }

    /// Install the shell's wakeup: invoked from the loader thread whenever
    /// a unit finishes, so the shell can call [`Session::poll_loaded`] and
    /// redraw. Shells without a thread-safe wakeup may poll instead while
    /// [`Session::has_pending_loads`] is true.
    pub fn set_waker(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        *self.waker.lock().unwrap() = Some(Box::new(wake));
    }

    /// Drain finished background loads into the caches; returns true when
    /// the page on screen changed and the shell should redraw.
    ///
    /// Always false in a build with no format that loads units in the
    /// background — the signature stays put so shells compile unchanged.
    #[cfg(not(feature = "_image-book"))]
    pub fn poll_loaded(&mut self) -> bool {
        false
    }

    /// Drain finished background loads into the caches; returns true when
    /// the page on screen changed and the shell should redraw.
    ///
    /// A prefetched unit landing is *not* that. It changes nothing the
    /// reader can see, and saying otherwise cost a full-page panel update
    /// per prefetch — on e-ink, a visible flash of a page that did not
    /// move.
    #[cfg(feature = "_image-book")]
    pub fn poll_loaded(&mut self) -> bool {
        let Some(loader) = self.loader.as_mut() else {
            return false;
        };
        let results = loader.drain();
        let mut visible = false;
        for (spine, result) in results {
            visible |= spine == self.spine;
            match result {
                Ok(unit) => {
                    let DecodedUnit {
                        width,
                        height,
                        rgba,
                        text,
                        natural,
                    } = unit;
                    self.images.entry(spine).or_default().insert(
                        spine as u64 + 1,
                        width,
                        height,
                        rgba,
                    );
                    self.loaded_units.insert(
                        spine,
                        LoadedUnit {
                            width,
                            height,
                            text,
                            natural,
                        },
                    );
                }
                Err(message) => {
                    log::error!("page {} failed to load: {message}", spine + 1);
                    self.load_errors.insert(spine, message);
                }
            }
            if self.placeholders.remove(&spine) {
                self.layouts.remove(&spine);
            }
            // A page that just landed is 15 MB of decoded RGBA for a comic;
            // this is the moment the cache grows, so it is the moment to
            // check it still fits. Sparing the arrival keeps a load that
            // was asked for from being thrown away before it is drawn.
            self.touch(spine);
            self.evict_keeping(Some(spine));
        }
        if visible {
            // The region is exactly where the image was placed: a page
            // image is letterboxed into the content box, so the ground
            // around it is the same ground the placeholder painted. A unit
            // that failed to load has no fragment to point at, and takes
            // the whole page.
            let spine = self.spine;
            let placed = self
                .layout_unit(spine)
                .and_then(|layout| layout.pages.first())
                .and_then(|page| page.fragments.first())
                .map(|fragment| fragment.rect);
            match placed {
                Some(rect) => self.mark_rect(FrameIntent::ContentArrived, rect),
                None => self.mark(FrameIntent::ContentArrived),
            }
        }
        visible
    }

    /// Whether background loads are in flight (placeholder pages showing).
    pub fn has_pending_loads(&self) -> bool {
        #[cfg(not(feature = "_image-book"))]
        {
            false
        }
        #[cfg(feature = "_image-book")]
        {
            self.loader.as_ref().is_some_and(Loader::has_pending)
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn kind(&self) -> BookKind {
        self.book.publication().kind()
    }

    pub fn spine(&self) -> usize {
        self.spine
    }

    pub fn spine_len(&self) -> usize {
        self.book.publication().spine().len()
    }

    pub fn page(&self) -> usize {
        self.page
    }

    /// Where the reader is, as the pair that actually identifies a place.
    /// Prefer this to `spine()`/`page()` for "did anything move" — see
    /// [`Position`] for the turn that made the difference matter.
    pub fn position(&self) -> Position {
        Position {
            spine: self.spine,
            page: self.page,
        }
    }

    pub fn settings(&self) -> &ReadingSettings {
        &self.settings
    }

    /// Page count of the current unit (`0` until metrics are known).
    pub fn page_count(&mut self) -> usize {
        let spine = self.spine;
        self.layout_unit(spine).map_or(0, |l| l.pages.len())
    }

    /// The page geometry in force, or `None` before a shell has set any.
    ///
    /// A shell needs it back to ask [`chapbook_core::TapZones`] what a tap
    /// means: the zones are fractions of a page, and the rotation they
    /// undo is here rather than in the shell's own bookkeeping.
    pub fn metrics(&self) -> Option<PageMetrics> {
        self.metrics
    }

    /// Set page geometry (window size in CSS px + margins + dpi scale).
    /// A change relayouts, keeping the reading position via the char map.
    pub fn set_metrics(&mut self, metrics: PageMetrics) {
        if self.metrics == Some(metrics) {
            return;
        }
        // A turn alone changes the panel, not the layout: keep the cached
        // pages and just repaint through the new orientation.
        if self
            .metrics
            .is_some_and(|current| current.same_layout(&metrics))
        {
            self.metrics = Some(metrics);
            self.mark(FrameIntent::Relayout);
            return;
        }
        let had = self.metrics.is_some();
        let locator = self.current_offset();
        self.metrics = Some(metrics);
        self.layouts.clear();
        self.placeholders.clear();
        // Image-book pixels are metrics-independent; only text chapters
        // rebuild their per-layout stores.
        if matches!(self.book, OpenBook::Epub(_)) {
            self.images.clear();
        }
        if had {
            let spine = self.spine;
            if let Some(layout) = self.layout_unit(spine) {
                self.page = layout.page_of(locator);
            }
        }
        self.mark(FrameIntent::Relayout);
    }

    // ---- Navigation ----

    /// Turn forward one page, crossing into the next unit at the end of
    /// this one. Returns whether the position moved, which is the answer a
    /// shell's loop wants and the one it cannot reliably derive from
    /// `page()` — see [`Position`].
    pub fn next_page(&mut self) -> bool {
        let before = self.position();
        let count = self.page_count();
        if self.page + 1 < count {
            self.page += 1;
            self.mark(FrameIntent::PageTurn);
        } else if self.spine + 1 < self.spine_len() {
            self.spine += 1;
            self.page = 0;
            self.mark(FrameIntent::UnitChange);
        }
        self.selection = None;
        self.position() != before
    }

    /// Turn back one page. Returns whether the position moved.
    pub fn prev_page(&mut self) -> bool {
        let before = self.position();
        if self.page > 0 {
            self.page -= 1;
            self.mark(FrameIntent::PageTurn);
        } else if self.spine > 0 {
            self.spine -= 1;
            let spine = self.spine;
            self.page = self
                .layout_unit(spine)
                .map_or(0, |l| l.pages.len())
                .saturating_sub(1);
            self.mark(FrameIntent::UnitChange);
        }
        self.selection = None;
        self.position() != before
    }

    /// Skip to the start of the next unit. Returns whether it moved.
    pub fn next_unit(&mut self) -> bool {
        let before = self.position();
        if self.spine + 1 < self.spine_len() {
            self.spine += 1;
            self.page = 0;
            self.selection = None;
            self.mark(FrameIntent::UnitChange);
        }
        self.position() != before
    }

    /// Skip to the start of the previous unit. Returns whether it moved.
    pub fn prev_unit(&mut self) -> bool {
        let before = self.position();
        if self.spine > 0 {
            self.spine -= 1;
            self.page = 0;
            self.selection = None;
            self.mark(FrameIntent::UnitChange);
        }
        self.position() != before
    }

    // ---- Settings ----

    /// Apply settings and remember them. Covers every field, including the
    /// three no shell could reach before: line height, justification, and
    /// whether publisher styles apply.
    ///
    /// Settings persist to the library, so font size survives a restart.
    /// `Global` is the reader's default from here on; `ThisBook` is an
    /// override that outlives later changes to the default.
    pub fn set_settings(&mut self, settings: ReadingSettings, scope: SettingsScope) {
        let changed = self.settings != settings;
        self.settings = settings;
        if changed {
            self.relayout_keeping_position();
        }
        self.persist_settings(scope);
    }

    /// Drop this book's override so it follows the reader's default again,
    /// applying that default now.
    #[cfg(feature = "library")]
    pub fn clear_book_settings(&mut self) {
        let book_id = self.book_id;
        let (Some(library), Some(id)) = (self.library_mut(), book_id) else {
            return;
        };
        if let Err(e) = library.clear_reading_settings(id) {
            log::error!("failed to clear book settings: {e}");
            return;
        }
        let settings = library.effective_settings(None);
        self.set_settings(settings, SettingsScope::Global);
    }

    /// Step the base font size, keeping the reader's place. A convenience
    /// over [`Session::set_settings`]; persists globally.
    pub fn adjust_font(&mut self, delta: f32) {
        let mut settings = self.settings.clone();
        settings.base_font_px = (settings.base_font_px + delta).clamp(10.0, 40.0);
        self.set_settings(settings, SettingsScope::Global);
    }

    /// Cycle light → sepia → dark. A convenience over
    /// [`Session::set_settings`]; persists globally.
    pub fn cycle_theme(&mut self) {
        let mut settings = self.settings.clone();
        settings.theme = settings.theme.cycle();
        self.set_settings(settings, SettingsScope::Global);
    }

    fn persist_settings(&mut self, scope: SettingsScope) {
        // Settings still apply; there is simply nowhere to write them
        // down, so they last as long as the session does.
        #[cfg(not(feature = "library"))]
        let _ = scope;
        #[cfg(feature = "library")]
        {
            let book_id = self.book_id;
            let settings = self.settings.clone();
            let Some(library) = self.library_mut() else {
                return;
            };
            let target = match scope {
                SettingsScope::Global => None,
                // No library record, nothing to hang an override on.
                SettingsScope::ThisBook => match book_id {
                    Some(id) => Some(id),
                    None => return,
                },
            };
            if let Err(e) = library.set_reading_settings(target, &settings) {
                log::error!("failed to save settings: {e}");
            }
        }
    }

    // ---- Input ----

    /// Which edge this book reads from, for [`TapZones`].
    ///
    /// Ask the session rather than reaching for [`TapZones::default`],
    /// which is `Ltr` and has no way to know better. The book declares
    /// this — EPUB's `page-progression-direction` — so a shell that
    /// picks for itself picks `Ltr` on every platform it ships to, and
    /// an RTL book paged the wrong way is unreadable rather than merely
    /// unfamiliar.
    ///
    /// ```no_run
    /// # use chapbook_core::TapZones;
    /// # fn f(session: &chapbook_reader::Session) {
    /// let zones = TapZones::new(session.reading_direction());
    /// # }
    /// ```
    ///
    /// [`TapZones`]: chapbook_core::TapZones
    /// [`TapZones::default`]: chapbook_core::TapZones::default
    pub fn reading_direction(&self) -> ReadingDirection {
        self.book.publication().reading_direction()
    }

    /// Apply a reader intent from [`chapbook_core::input`].
    ///
    /// This is the second half of the input seam: shells translate native
    /// events into an [`Action`] and the engine applies it, so a tap zone
    /// or a key binding is written once instead of once per platform.
    ///
    /// The [`ActionOutcome`] answers two questions rather than one,
    /// because a shell needs both and can derive neither from the other.
    /// See its own documentation for what guessing costs.
    pub fn apply(&mut self, action: Action) -> ActionOutcome {
        let moved = |did: bool| {
            if did {
                ActionOutcome::Changed
            } else {
                ActionOutcome::Unchanged
            }
        };
        match action {
            Action::NextPage => moved(self.next_page()),
            Action::PrevPage => moved(self.prev_page()),
            Action::NextUnit => moved(self.next_unit()),
            Action::PrevUnit => moved(self.prev_unit()),
            // The bottom of the back stack is where the platform's own
            // Back takes over, on both mobile targets. Declining it here
            // is what lets a shell forward the gesture unconditionally
            // instead of shadowing the history to know when not to.
            Action::Back => {
                if self.back() {
                    ActionOutcome::Changed
                } else {
                    ActionOutcome::Unhandled
                }
            }
            // `adjust_font` clamps, so at either stop this correctly says
            // nothing moved rather than asking for a redraw of the same
            // page at the same size. The key is consumed either way.
            Action::FontUp | Action::FontDown => {
                let before = self.settings.base_font_px;
                let step = if action == Action::FontUp {
                    FONT_STEP_PX
                } else {
                    -FONT_STEP_PX
                };
                self.adjust_font(step);
                moved(self.settings.base_font_px != before)
            }
            Action::CycleTheme => {
                self.cycle_theme();
                ActionOutcome::Changed
            }
            // A reader's chrome belongs to the shell: there is nothing
            // here to toggle, and the shell that bound this wants the
            // event back.
            Action::ToggleMenu => ActionOutcome::Unhandled,
            // `Action` is `#[non_exhaustive]`. An intent this engine has
            // no verb for is one the shell may still want to act on.
            _ => ActionOutcome::Unhandled,
        }
    }

    fn relayout_keeping_position(&mut self) {
        let locator = self.current_offset();
        self.layouts.clear();
        self.placeholders.clear();
        if matches!(self.book, OpenBook::Epub(_)) {
            self.images.clear();
        }
        let spine = self.spine;
        if let Some(layout) = self.layout_unit(spine) {
            self.page = layout.page_of(locator);
        }
        self.mark(FrameIntent::Relayout);
    }

    // ---- Navigation ----

    /// The book's table of contents, for a shell that offers one.
    pub fn toc(&self) -> &[TocEntry] {
        self.book.publication().toc()
    }

    /// Where the reader is now, as a locator.
    pub fn locator(&self) -> Locator {
        Locator::new(self.spine, self.current_offset())
    }

    /// Jump to a locator, remembering where we came from. `false` if the
    /// spine index doesn't exist.
    pub fn goto(&mut self, target: Locator) -> bool {
        self.jump(target.spine_index, Some(target.char_offset), None)
    }

    /// Jump to an element `id` within a unit — a TOC fragment or a
    /// footnote. `false` if the spine index doesn't exist; a fragment that
    /// turns out not to be in the unit lands at its start.
    pub fn goto_anchor(&mut self, spine_index: usize, fragment: &str) -> bool {
        self.jump(spine_index, None, Some(fragment.to_string()))
    }

    /// Jump to a TOC entry, by spine index where the entry has one and by
    /// href otherwise.
    pub fn goto_toc(&mut self, entry: &TocEntry) -> bool {
        let Some(spine) = entry
            .spine_index
            .or_else(|| entry.href.as_deref().and_then(|h| self.spine_index_of(h)))
        else {
            return false;
        };
        match &entry.fragment {
            Some(fragment) => self.goto_anchor(spine, fragment),
            None => self.goto(Locator::chapter_start(spine)),
        }
    }

    /// The link under a point in panel coordinates, as written in the
    /// document. Only inside the link's own text: pressing the margin
    /// beside a link is not pressing the link.
    pub fn link_at(&mut self, x: f32, y: f32) -> Option<String> {
        let (px, py) = self.metrics.map_or((x, y), |m| m.panel_to_page(x, y));
        let (spine, page) = (self.spine, self.page);
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(px, py))?;
        self.links
            .get(&spine)?
            .iter()
            .find(|link| offset >= link.start && offset < link.end)
            .map(|link| link.href.clone())
    }

    /// Follow a document-internal link. External links (anything with a
    /// scheme) are a shell decision, not a reading position, so they
    /// return `false` untouched.
    pub fn follow_link(&mut self, href: &str) -> bool {
        if href.contains("://") || href.starts_with("mailto:") {
            return false;
        }
        let (path, fragment) = match href.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment.to_string())),
            None => (href, None),
        };
        // An empty path is a jump within this unit.
        let spine = if path.is_empty() {
            self.spine
        } else {
            let Ok(item) = self.book.publication().spine_item(self.spine) else {
                return false;
            };
            let resolved = chapbook_epub::resolve_href(&item.href.clone(), path);
            match self.spine_index_of(&resolved) {
                Some(spine) => spine,
                None => return false,
            }
        };
        match fragment {
            Some(fragment) => self.goto_anchor(spine, &fragment),
            None => self.goto(Locator::chapter_start(spine)),
        }
    }

    /// Return to where the last jump started. `false` with nothing to go
    /// back to.
    pub fn back(&mut self) -> bool {
        let Some(target) = self.back_stack.pop() else {
            return false;
        };
        self.land(target.spine_index, Some(target.char_offset), None);
        true
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    /// Spine index of a container-root path, tolerating the leading slash
    /// EPUB manifests may or may not carry.
    fn spine_index_of(&self, href: &str) -> Option<usize> {
        let want = href
            .split('#')
            .next()
            .unwrap_or(href)
            .trim_start_matches('/');
        self.book
            .publication()
            .spine()
            .iter()
            .position(|item| item.href.trim_start_matches('/') == want)
    }

    /// A jump: remember where we were, then land.
    fn jump(&mut self, spine: usize, offset: Option<u32>, anchor: Option<String>) -> bool {
        if spine >= self.book.publication().spine().len() {
            return false;
        }
        let from = self.locator();
        self.land(spine, offset, anchor);
        // Cap the trail: a reader chasing footnotes shouldn't grow it
        // without bound.
        self.back_stack.push(from);
        if self.back_stack.len() > 64 {
            self.back_stack.remove(0);
        }
        true
    }

    fn land(&mut self, spine: usize, offset: Option<u32>, anchor: Option<String>) {
        let same_unit = spine == self.spine;
        self.spine = spine;
        self.page = 0;
        self.pending_offset = offset.map(|offset| (spine, offset));
        self.pending_anchor = anchor.map(|anchor| (spine, anchor));
        self.selection = None;
        self.mark(if same_unit {
            FrameIntent::PageTurn
        } else {
            FrameIntent::UnitChange
        });
    }

    // ---- Search ----

    /// Search one unit. The building block: a shell wanting the whole book
    /// without blocking drives this unit by unit on a worker.
    ///
    /// Matching is case-insensitive by simple per-character lowercasing —
    /// no full case folding, no diacritic folding — which keeps every hit
    /// on an exact char offset in the unit's locator text.
    pub fn search_unit(&mut self, spine: usize, query: &str) -> Vec<SearchHit> {
        let needle: Vec<char> = query.chars().map(fold_char).collect();
        if needle.is_empty() {
            return Vec::new();
        }
        let Some(text) = self.unit_text(spine) else {
            return Vec::new();
        };
        let chars: Vec<char> = text.chars().collect();
        let folded: Vec<char> = chars.iter().copied().map(fold_char).collect();
        let mut hits = Vec::new();
        let mut at = 0;
        while at + needle.len() <= folded.len() {
            if folded[at..at + needle.len()] == needle[..] {
                let (start, end) = (at as u32, (at + needle.len()) as u32);
                let (context, match_range) = context_around(&chars, start, end);
                hits.push(SearchHit {
                    locator: Locator::new(spine, start),
                    end,
                    context,
                    match_range,
                });
                at += needle.len();
            } else {
                at += 1;
            }
        }
        hits
    }

    /// Search the whole book, stopping at `limit` hits.
    ///
    /// **Blocking:** this reads and extracts every unit's text, which for a
    /// long book is seconds — the same contract as
    /// [`chapbook_core::Publication::unit_bytes`], and the same rule: not
    /// on a UI thread. Comics contribute nothing, having no text layer, and
    /// a PDF page contributes only once the loader has decoded it — its
    /// text layer comes out of the same pass as its pixels.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<SearchHit> {
        let mut hits = Vec::new();
        for spine in 0..self.book.publication().spine().len() {
            if hits.len() >= limit {
                break;
            }
            let mut unit = self.search_unit(spine, query);
            unit.truncate(limit - hits.len());
            hits.append(&mut unit);
        }
        hits
    }

    // ---- Selection ----

    /// Begin a selection at a point in panel coordinates (CSS px, and
    /// identical to page space unless the metrics carry a rotation).
    /// Returns whether the point hit text.
    pub fn selection_begin(&mut self, x: f32, y: f32) -> bool {
        self.selection = None;
        self.mark(FrameIntent::Selection);
        let Some(offset) = self.offset_at(x, y) else {
            return false;
        };
        self.selection = Some((offset, offset));
        true
    }

    /// Extend the selection to a point (drag). No-op without an anchor.
    pub fn selection_drag(&mut self, x: f32, y: f32) {
        let Some((anchor, _)) = self.selection else {
            return;
        };
        if let Some(offset) = self.offset_at(x, y) {
            self.selection = Some((anchor, offset));
            self.mark(FrameIntent::Selection);
        }
    }

    /// Select a locator range directly — what a shell does to show where a
    /// search hit or an annotation sits on the page.
    pub fn select_range(&mut self, start: u32, end: u32) {
        self.selection = Some((start, end));
        self.mark(FrameIntent::Selection);
    }

    pub fn selection_clear(&mut self) {
        self.selection = None;
        self.mark(FrameIntent::Selection);
    }

    /// The selected locator range `[start, end)`, when non-empty.
    pub fn selected_range(&self) -> Option<(u32, u32)> {
        let (a, b) = self.selection?;
        let (start, end) = (a.min(b), a.max(b));
        (end > start).then_some((start, end))
    }

    /// The selection's text, ready to paste.
    ///
    /// EPUB units slice the unit's locator text — the space the offsets
    /// are defined in. PDF units read their hidden text layer instead,
    /// one entry per extracted line. Comics have no text layer.
    ///
    /// Locator text is the raw offset space, so it still carries the
    /// source document's line breaks and indentation; runs of ASCII
    /// whitespace collapse to a single space here (non-breaking spaces are
    /// content and survive).
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selected_range()?;
        let raw: String = match self.book.publication().kind() {
            BookKind::Epub => {
                let unit = unit_locator_text(self.book.publication(), self.spine)?;
                unit.chars()
                    .skip(start as usize)
                    .take((end - start) as usize)
                    .collect()
            }
            BookKind::Pdf => self.hidden_text_in_range(start, end)?,
            BookKind::Comic => return None,
        };
        let text = raw.split_ascii_whitespace().collect::<Vec<_>>().join(" ");
        (!text.is_empty()).then_some(text)
    }

    /// A PDF unit's hidden text over a locator range. Line breaks aren't
    /// part of that locator space — the extracted lines are contiguous in
    /// it — so they are re-inserted between the sliced lines here.
    fn hidden_text_in_range(&self, start: u32, end: u32) -> Option<String> {
        let layout = self.layouts.get(&self.spine)?;
        let mut lines = Vec::new();
        for page in &layout.pages {
            for fragment in &page.fragments {
                let chapbook_paint::FragmentKind::HiddenText(line) = &fragment.kind else {
                    continue;
                };
                let line_end = line.locator_start + line.text.chars().count() as u32;
                if line_end <= start || line.locator_start >= end {
                    continue;
                }
                let from = start.saturating_sub(line.locator_start) as usize;
                let to = (end.min(line_end) - line.locator_start) as usize;
                lines.push(line.text.chars().take(to).skip(from).collect::<String>());
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn offset_at(&mut self, x: f32, y: f32) -> Option<u32> {
        // Input is in panel space; hit-testing happens in page space.
        let (x, y) = self.metrics.map_or((x, y), |m| m.panel_to_page(x, y));
        let (spine, page) = (self.spine, self.page);
        let layout = self.layout_unit(spine)?;
        layout.pages.get(page)?.offset_at(Point::new(x, y))
    }

    // ---- Highlights ----

    /// Persist the current selection as a highlight and return its library
    /// id. `None` without a text selection, without a library (OPDS
    /// streams have no local record), or on a comic — no text layer, so
    /// nothing to anchor to.
    #[cfg(feature = "library")]
    pub fn add_highlight(&mut self) -> Option<i64> {
        self.add_ranged(AnnotationKind::Highlight, None)
    }

    /// Attach a note to the current selection. The quoted text is kept as
    /// the annotation's text, the note body as its own record — a note is a
    /// highlight that says something.
    #[cfg(feature = "library")]
    pub fn add_note(&mut self, body: &str) -> Option<i64> {
        self.add_ranged(AnnotationKind::Note, Some(body))
    }

    /// Bookmark the current page. A point, not a range, so it needs no
    /// selection — but it does need a text unit to anchor in.
    #[cfg(feature = "library")]
    pub fn add_bookmark(&mut self) -> Option<i64> {
        let offset = self.current_offset();
        let (start, _) = self.capture_endpoints(offset, offset)?;
        let book_id = self.book_id?;
        let id = self
            .library
            .as_mut()?
            .add_annotation(book_id, AnnotationKind::Bookmark, &start, None, None, None)
            .map_err(|e| log::error!("failed to save bookmark: {e}"))
            .ok()?;
        let spine = self.spine;
        self.stored.push(StoredAnnotation {
            id,
            kind: AnnotationKind::Bookmark,
            target: spine,
            href_matched: true,
            start,
            end: None,
            text: None,
            color: None,
        });
        self.mark(FrameIntent::Annotation);
        Some(id)
    }

    #[cfg(feature = "library")]
    fn add_ranged(&mut self, kind: AnnotationKind, body: Option<&str>) -> Option<i64> {
        let (start, end) = self.selected_range()?;
        let quote = self.selected_text()?;
        let (start_loc, end_loc) = self.capture_endpoints(start, end)?;
        let book_id = self.book_id?;
        // A note keeps its body; a highlight keeps the words it marks.
        let text = body.unwrap_or(quote.as_str());
        let id = self
            .library
            .as_mut()?
            .add_annotation(book_id, kind, &start_loc, Some(&end_loc), Some(text), None)
            .map_err(|e| log::error!("failed to save annotation: {e}"))
            .ok()?;
        let spine = self.spine;
        let text = Some(text.to_string());
        self.stored.push(StoredAnnotation {
            id,
            kind,
            target: spine,
            href_matched: true,
            start: start_loc,
            end: Some(end_loc),
            text: text.clone(),
            color: None,
        });
        self.mark_range(FrameIntent::Annotation, start, end);
        // Show it immediately: the cache is authoritative once populated.
        self.resolved_highlights
            .entry(spine)
            .or_default()
            .push(Highlight {
                id,
                spine,
                start,
                end,
                text,
                color: None,
            });
        Some(id)
    }

    /// Every mark in the book, as stored — no unit text is read, so this
    /// is cheap enough for a list. Ordered by position in the book.
    #[cfg(feature = "library")]
    pub fn annotations(&self) -> Vec<AnnotationSummary> {
        let mut all: Vec<AnnotationSummary> = self
            .stored
            .iter()
            .map(|a| AnnotationSummary {
                id: a.id,
                kind: a.kind,
                spine_index: a.target,
                progression: a.start.book_progression,
                text: a.text.clone(),
                color: a.color.clone(),
            })
            .collect();
        all.sort_by(|a, b| {
            a.progression
                .total_cmp(&b.progression)
                .then(a.id.cmp(&b.id))
        });
        all
    }

    /// Marks that paint in `spine`, resolved into its locator space and
    /// cached. Empty while that text is unavailable — an image book's unit
    /// resolves only once its page has loaded. Bookmarks are points and
    /// paint nothing, so they aren't here.
    #[cfg(feature = "library")]
    pub fn highlights(&mut self, spine: usize) -> &[Highlight] {
        if !self.resolved_highlights.contains_key(&spine) {
            // Extracting a unit's text is not free; skip it entirely when
            // nothing is stored against this unit.
            let resolved = if self.stored.iter().any(|a| a.target == spine) {
                let Some(text) = self.unit_text(spine) else {
                    return &[];
                };
                self.resolve_highlights(spine, &text)
            } else {
                Vec::new()
            };
            self.resolved_highlights.insert(spine, resolved);
        }
        self.resolved_highlights
            .get(&spine)
            .map_or(&[][..], Vec::as_slice)
    }

    /// The stored highlight under a point in panel coordinates — what a
    /// tap needs to select, recolor, or delete one by touching it. Only
    /// inside the marked text, like a link.
    #[cfg(feature = "library")]
    pub fn highlight_at(&mut self, x: f32, y: f32) -> Option<i64> {
        let (px, py) = self.metrics.map_or((x, y), |m| m.panel_to_page(x, y));
        let (spine, page) = (self.spine, self.page);
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(px, py))?;
        self.highlights(spine)
            .iter()
            .find(|h| offset >= h.start && offset < h.end)
            .map(|h| h.id)
    }

    /// Recolor a highlight. `None` hands it back to the theme color.
    /// Colors are `#rgb`, `#rrggbb`, or `#rrggbbaa`.
    #[cfg(feature = "library")]
    pub fn set_highlight_color(&mut self, id: i64, color: Option<&str>) {
        if let Some(library) = self.library_mut() {
            if let Err(e) = library.set_annotation_color(id, color) {
                log::error!("failed to recolor annotation: {e}");
                return;
            }
        }
        let color = color.map(str::to_string);
        for stored in self.stored.iter_mut().filter(|a| a.id == id) {
            stored.color = color.clone();
        }
        for resolved in self.resolved_highlights.values_mut() {
            for highlight in resolved.iter_mut().filter(|h| h.id == id) {
                highlight.color = color.clone();
            }
        }
        match self.highlight_range(id) {
            Some((start, end)) => self.mark_range(FrameIntent::Annotation, start, end),
            None => self.mark(FrameIntent::Annotation),
        }
    }

    /// Jump to a stored annotation. `false` if its unit can't be read.
    #[cfg(feature = "library")]
    pub fn goto_annotation(&mut self, id: i64) -> bool {
        let Some((target, start)) = self
            .stored
            .iter()
            .find(|a| a.id == id)
            .map(|a| (a.target, a.start.clone()))
        else {
            return false;
        };
        let Some(text) = self.unit_text(target) else {
            return false;
        };
        let trusted = self.same_edition && self.stored.iter().any(|a| a.id == id && a.href_matched);
        let offset = resolve_in_text(&text, &start, trusted).offset();
        self.goto(Locator::new(target, offset))
    }

    /// Delete an annotation (a soft delete in the library, kept for sync).
    #[cfg(feature = "library")]
    pub fn remove_annotation(&mut self, id: i64) {
        if let Some(library) = self.library_mut() {
            if let Err(e) = library.delete_annotation(id) {
                log::error!("failed to delete annotation: {e}");
                return;
            }
        }
        // Its extent has to be read before it is dropped from the cache.
        let range = self.highlight_range(id);
        self.stored.retain(|a| a.id != id);
        for resolved in self.resolved_highlights.values_mut() {
            resolved.retain(|h| h.id != id);
        }
        match range {
            Some((start, end)) => self.mark_range(FrameIntent::Annotation, start, end),
            None => self.mark(FrameIntent::Annotation),
        }
    }

    #[cfg(feature = "library")]
    fn resolve_highlights(&self, spine: usize, text: &str) -> Vec<Highlight> {
        self.stored
            .iter()
            .filter(|a| a.target == spine && a.kind != AnnotationKind::Bookmark)
            .filter_map(|a| {
                let end_loc = a.end.as_ref()?;
                let trusted = self.same_edition && a.href_matched;
                let start = resolve_in_text(text, &a.start, trusted).offset();
                let end = resolve_in_text(text, end_loc, trusted).offset();
                // A range that collapsed under re-anchoring has nothing
                // left to paint.
                (end > start).then(|| Highlight {
                    id: a.id,
                    spine,
                    start,
                    end,
                    text: a.text.clone(),
                    color: a.color.clone(),
                })
            })
            .collect()
    }

    /// Full layered locators for a selection's endpoints.
    #[cfg(feature = "library")]
    fn capture_endpoints(&self, start: u32, end: u32) -> Option<(LayeredLocator, LayeredLocator)> {
        let href = self
            .book
            .publication()
            .spine_item(self.spine)
            .ok()?
            .href
            .clone();
        match self.book.publication().kind() {
            BookKind::Epub => {
                let ctx = self.unit_char_context();
                let capture = |offset| {
                    LayeredLocator::capture(
                        &href, self.spine, &ctx.text, offset, ctx.prior, ctx.total,
                    )
                };
                Some((capture(start), capture(end)))
            }
            // A PDF page's extracted text is a real locator space, so the
            // within-unit layers are exact — but the whole-book
            // progression stays page-based, as it is for every image book.
            BookKind::Pdf => {
                let text = self.unit_text(self.spine)?;
                let units = self.book.publication().spine().len() as u64;
                let capture = |offset| {
                    let mut loc = LayeredLocator::capture(&href, self.spine, &text, offset, 0, 0);
                    loc.book_progression =
                        chapbook_core::book_progression(self.spine as u64, 0, units);
                    loc
                };
                Some((capture(start), capture(end)))
            }
            BookKind::Comic => None,
        }
    }

    /// A unit's locator text — the space its selection offsets live in.
    /// EPUB chapters extract it on demand; a PDF page's is its text layer,
    /// contiguous across the extracted lines, and only exists once the
    /// page has loaded.
    fn unit_text(&self, spine: usize) -> Option<String> {
        match self.book.publication().kind() {
            BookKind::Epub => unit_locator_text(self.book.publication(), spine),
            BookKind::Pdf => {
                let unit = self.loaded_units.get(&spine)?;
                Some(unit.text.iter().map(|line| line.text.as_str()).collect())
            }
            BookKind::Comic => None,
        }
    }

    /// Char counts around the current unit, extracted over the whole
    /// spine. One pass serves any number of captures in the same unit.
    #[cfg(feature = "library")]
    fn unit_char_context(&self) -> UnitCharContext {
        let mut ctx = UnitCharContext {
            text: String::new(),
            prior: 0,
            total: 0,
        };
        for i in 0..self.book.publication().spine().len() {
            let text = unit_locator_text(self.book.publication(), i).unwrap_or_default();
            let chars = text.chars().count() as u64;
            if i < self.spine {
                ctx.prior += chars;
            }
            if i == self.spine {
                ctx.text = text;
            }
            ctx.total += chars;
        }
        ctx
    }

    // ---- Rendering ----

    /// The extent of a resolved highlight on the current page.
    #[cfg(feature = "library")]
    fn highlight_range(&self, id: i64) -> Option<(u32, u32)> {
        self.resolved_highlights
            .get(&self.spine)?
            .iter()
            .find(|h| h.id == id)
            .map(|h| (h.start, h.end))
    }

    /// Record what changed. The strongest intent since the last frame is
    /// the one that describes it, so this never downgrades.
    ///
    /// A change recorded here does not say where it landed, so the frame
    /// falls back to a full repaint. Use [`Session::mark_range`] when the
    /// change is confined to a span of text.
    fn mark(&mut self, intent: FrameIntent) {
        self.pending = self.pending.max(intent);
        // A selection is the one exception: it states its region at frame
        // time instead, from the difference between what is painted and
        // what is selected now.
        if intent != FrameIntent::Selection {
            self.pending_damage.unstated = true;
        }
    }

    /// Record a change confined to a locator range on the current page.
    ///
    /// Damage accumulates independently of the intent ordering. A
    /// highlight landing while a selection is live must not lose its
    /// region just because `Annotation` outranks `Selection` — both name
    /// where they changed, so the frame reports the union of the two.
    #[cfg(feature = "library")]
    fn mark_range(&mut self, intent: FrameIntent, start: u32, end: u32) {
        match self.range_damage(start, end) {
            Some(region) => self.mark_rect(intent, region),
            // The range is on another page, so it disturbs nothing here —
            // which is a stated region of nothing, not an unstated one.
            None => self.pending = self.pending.max(intent),
        }
    }

    /// Record a change confined to a region the engine already knows in
    /// page coordinates, rather than one it has to derive from locators.
    // Highlights state a region, and so does a page image arriving.
    #[cfg(any(feature = "library", feature = "_image-book"))]
    fn mark_rect(&mut self, intent: FrameIntent, region: Rect) {
        self.pending = self.pending.max(intent);
        if self.pending_damage.unstated {
            return;
        }
        self.pending_damage.region = Some(match self.pending_damage.region {
            Some(existing) => existing.union(&region),
            None => region,
        });
    }

    /// The area a locator range covers on the current page, or `None` when
    /// it lies on another page and so disturbs nothing here.
    #[cfg(feature = "library")]
    fn range_damage(&self, start: u32, end: u32) -> Option<Rect> {
        let page = self.layouts.get(&self.spine)?.pages.get(self.page)?;
        page.rects_for_range(start, end)
            .into_iter()
            .reduce(|damage, rect| damage.union(&rect))
    }

    /// The current page as paint-neutral display ops, plus what changed
    /// since the last frame — the backend contract from
    /// `docs/ARCHITECTURE.md`, for shells that rasterize themselves: a GPU
    /// backend, a platform canvas, an e-ink panel, an exporter.
    /// [`Session::render`] is this plus the bundled CPU rasterizer.
    ///
    /// `Image` ops carry keys into the unit's [`Session::image_store`],
    /// not pixels, and glyph runs name faces in its font database, so a
    /// shell needs [`Session::paint_resources`] too.
    ///
    /// Taking a frame consumes the change record: the next one reports
    /// [`FrameIntent::Repaint`] until something else moves.
    ///
    /// `None` until metrics are set, and while the unit has no page — an
    /// image book still loading, or a unit that failed to load.
    pub fn frame(&mut self) -> Option<Frame> {
        let list = self.page_display_list()?;
        let intent = std::mem::take(&mut self.pending);
        let damage = self.damage_for();
        self.pending_damage = PendingDamage::default();
        self.painted_selection = self.selected_range();
        Some(Frame {
            list,
            intent,
            damage,
        })
    }

    /// The region a frame disturbs, when it is cheaper to state than to
    /// repaint — `None` meaning "assume the whole page", which is always
    /// correct and sometimes wasteful.
    ///
    /// This deliberately does not consult the intent. Intent answers "how
    /// disturbing is this change", damage answers "where is it", and
    /// tying the second to the ordering of the first meant a highlight
    /// repainted the entire page because `Annotation` outranked the
    /// `Selection` that had already named its lines.
    fn damage_for(&self) -> Option<Rect> {
        if self.pending_damage.unstated {
            return None;
        }
        let mut damage = self.pending_damage.region;
        // A moved selection disturbs both where it was and where it is.
        let current = self.selected_range();
        if current != self.painted_selection {
            let page = self.layouts.get(&self.spine)?.pages.get(self.page)?;
            let moved = [self.painted_selection, current]
                .into_iter()
                .flatten()
                .flat_map(|(start, end)| page.rects_for_range(start, end))
                .reduce(|damage, rect| damage.union(&rect));
            damage = match (damage, moved) {
                (Some(a), Some(b)) => Some(a.union(&b)),
                (a, b) => a.or(b),
            };
        }
        damage
    }

    fn page_display_list(&mut self) -> Option<chapbook_paint::DisplayList> {
        self.metrics?;
        // Resolve a restored offset, or a jump's anchor, once the unit
        // has laid out.
        // Both of these are dropped rather than deferred once the reader
        // has left the unit they were captured in. A pending landing is a
        // statement about one unit, and the reader having navigated away
        // supersedes it — carrying it along would resolve an offset from
        // one chapter against the pages of another.
        if let Some((spine, fragment)) = self.pending_anchor.take() {
            if spine == self.spine {
                match self.layout_unit(spine) {
                    Some(layout) => {
                        // A fragment that isn't in the unit lands at its start.
                        self.page = layout.anchors.get(&fragment).copied().unwrap_or(0);
                    }
                    None => self.pending_anchor = Some((spine, fragment)),
                }
            }
        }
        if let Some((spine, offset)) = self.pending_offset {
            if spine != self.spine {
                self.pending_offset = None;
            } else if let Some(layout) = self.layout_unit(spine) {
                self.page = layout.page_of(offset);
                self.pending_offset = None;
            }
        }
        let (spine, page_idx) = (self.spine, self.page);
        // Stored highlights first, the live selection on top of them.
        // Without a library nothing is stored, so the live selection is
        // the only thing that paints.
        #[cfg(not(feature = "library"))]
        let mut selections: Vec<Selection> = Vec::new();
        #[cfg(feature = "library")]
        let highlight_color = self.settings.theme.highlight();
        #[cfg(feature = "library")]
        let mut selections: Vec<Selection> = self
            .highlights(spine)
            .iter()
            .map(|h| Selection {
                start: h.start,
                end: h.end,
                // A stored color keeps the theme's transparency unless it
                // states its own; an unparseable one falls back rather
                // than vanishing.
                color: h
                    .color
                    .as_deref()
                    .and_then(|hex| Rgba::from_hex(hex, highlight_color.a))
                    .unwrap_or(highlight_color),
            })
            .collect();
        if let Some((start, end)) = self.selected_range() {
            selections.push(Selection {
                start,
                end,
                color: self.settings.theme.selection(),
            });
        }
        let background = self.settings.theme.background();
        let page_count = self.layout_unit(spine).map_or(0, |l| l.pages.len());
        if page_count == 0 {
            return None;
        }
        self.page = self.page.min(page_count - 1);
        let page_idx = page_idx.min(page_count - 1);
        let layout = self.layouts.get(&spine)?;
        let page = layout.pages.get(page_idx)?;
        Some(chapbook_paint::build_display_list(
            page,
            background,
            &selections,
        ))
    }

    /// Convert rendered pages for a panel that can't show full color —
    /// 16-level grey or 1-bit e-ink. [`Session::render`] applies it; a
    /// shell rasterizing a frame itself calls
    /// [`chapbook_paint::quantize`] at the same point — panel policy
    /// belongs to the target, not to whichever rasterizer produced the
    /// pixels.
    ///
    /// This changes pixels, not ops, so it does not disturb the frame
    /// record: the display list is identical either way.
    pub fn set_pixel_format(&mut self, format: PixelFormat) {
        self.pixel_format = format;
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// What the font source produced: how many faces loaded, and any CSS
    /// generic that resolved to a family nothing carries.
    ///
    /// Worth printing once at startup on a platform you have not run on.
    /// Every font failure this API exists to prevent is silent — a page
    /// still lays out, still renders, still conforms — so this is the only
    /// cheap way to tell a correctly configured device from a broken one
    /// without looking at pixels.
    pub fn font_report(&self) -> &FontReport {
        &self.font_report
    }

    /// Every font family the session can match, deduplicated and sorted.
    ///
    /// The read-back half of [`FontSource`], and what a font-family picker
    /// needs: a shell cannot offer a choice it cannot enumerate. Includes
    /// families registered from a book's own `@font-face` rules once that
    /// unit has laid out, so the list grows as chapters load.
    pub fn font_families(&self) -> Vec<String> {
        let mut families: Vec<String> = self
            .fonts
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        // The embedded math face is a rendering resource, not a reading
        // typeface: a family picker must not offer it.
        #[cfg(feature = "mathml")]
        families.retain(|name| name != chapbook_layout::MATH_FONT_FAMILY);
        families.sort_unstable();
        families.dedup();
        families
    }

    /// The image store backing the current unit's `Image` ops. Empty for
    /// units that carry no images.
    pub fn image_store(&self) -> &ImageStore {
        self.images.get(&self.spine).unwrap_or(&self.empty_images)
    }

    /// What a display list's ops resolve against: the font database its
    /// `GlyphRun`s name faces in — glyphs are shaped already, so a backend
    /// only rasterizes them — and the image store its `Image` ops key
    /// into. A shell rasterizing for itself needs both, and they are
    /// disjoint fields, so they come back together.
    pub fn paint_resources(&mut self) -> (&mut cosmic_text::FontSystem, &ImageStore) {
        let images = self.images.get(&self.spine).unwrap_or(&self.empty_images);
        (&mut self.fonts, images)
    }

    /// Rasterize the current page at the current metrics with the bundled
    /// CPU backend — the convenience path over [`Session::frame`]. The
    /// result is in panel orientation and panel color: the metrics'
    /// rotation and the session's pixel format are both applied here.
    /// `None` under the same conditions as a frame.
    /// Give back everything that can be rebuilt, keeping only the unit on
    /// screen.
    ///
    /// The lever `onTrimMemory` and `didReceiveMemoryWarning` had nothing
    /// to call. Distinct from lowering the budget: a budget is a
    /// steady-state cap, this is "right now, as much as you can". Nothing
    /// dropped here is authoritative — pages are re-read and re-decoded,
    /// chapters laid out again — so the cost is a slower next page turn,
    /// not a lost anything.
    pub fn release_caches(&mut self) {
        let pinned = self.spine;
        self.layouts.retain(|spine, _| *spine == pinned);
        self.images.retain(|spine, _| *spine == pinned);
        self.loaded_units.retain(|spine, _| *spine == pinned);
        self.placeholders.retain(|spine| *spine == pinned);
        #[cfg(feature = "library")]
        self.resolved_highlights.retain(|spine, _| *spine == pinned);
        self.used_at.retain(|spine, _| *spine == pinned);
    }

    /// You are about to be stopped: persist, and let go of the database.
    ///
    /// Android's `onStop` is the only guaranteed callback there is and it
    /// has a time budget, so this saves the one piece of state that is
    /// written lazily — the reading position. Settings and annotations are
    /// already written when they change.
    ///
    /// iOS wants the same call for a second reason Android never raises.
    /// Bundled SQLite takes POSIX advisory locks, and an app still holding
    /// one on a file in a *shared* container when it suspends is killed by
    /// the watchdog with `0xdead10cc`. So this closes the connection rather
    /// than merely flushing it. A share extension or a widget means an app
    /// group, and an app group means a shared container, which is when that
    /// stops being hypothetical.
    ///
    /// Caches go too: a stopped app should not be holding decoded pages.
    ///
    /// The session stays usable. `onStop` is often followed by `onStart`
    /// with the process still alive, so the next thing that needs the
    /// library reopens it. What this does *not* do is stop the loader
    /// thread; a suspended session with a page still arriving will finish
    /// decoding it.
    pub fn suspend(&mut self) {
        self.save_position();
        self.release_caches();
        // Without a library there is no connection to let go of, and
        // nothing was lazily written that needs flushing first.
        #[cfg(feature = "library")]
        {
            self.library = None;
            self.suspended = true;
        }
    }

    /// The library, reopened if [`suspend`](Self::suspend) closed it.
    ///
    /// Every access goes through here, so "the database is shut because we
    /// were told to let go of it" and "this platform has no library" stay
    /// distinguishable — both are `None` in the field and only one should
    /// be retried.
    #[cfg(feature = "library")]
    fn library_mut(&mut self) -> Option<&mut chapbook_library::Library> {
        if self.suspended {
            self.suspended = false;
            if let Some(dir) = &self.library_dir {
                self.library = chapbook_library::Library::open(dir)
                    .map_err(|e| log::warn!("library unavailable after resume: {e}"))
                    .ok();
            }
        }
        self.library.as_mut()
    }

    /// Bytes the layout and image caches currently hold.
    ///
    /// Exact for decoded images, approximate for laid-out chapters — see
    /// `ChapterLayout::approx_bytes`. A host reporting memory, or deciding
    /// whether to lower the budget, wants this.
    pub fn cache_bytes(&self) -> usize {
        self.layouts
            .values()
            .map(|l| l.approx_bytes())
            .sum::<usize>()
            + self.images.values().map(|i| i.bytes()).sum::<usize>()
    }

    /// What the caches are allowed to hold.
    pub fn cache_budget(&self) -> usize {
        self.cache_budget
    }

    /// Change the cap, evicting immediately if the caches are now over it.
    ///
    /// Runtime rather than construction-only because memory pressure is a
    /// runtime event: a host told to trim lowers this and the session
    /// obeys at once.
    pub fn set_cache_budget(&mut self, bytes: usize) {
        self.cache_budget = bytes;
        self.evict_to_budget();
    }

    /// Note that a unit was just used, for eviction order.
    fn touch(&mut self, spine: usize) {
        self.use_clock += 1;
        self.used_at.insert(spine, self.use_clock);
    }

    /// Drop least-recently-used units until the caches fit the budget.
    ///
    /// **The current unit is never evicted.** Everything else is fair game,
    /// which is safe because none of it is authoritative: a comic page is
    /// re-read from the archive and re-decoded, a chapter is laid out
    /// again. The caches have always been reconstructible; they had simply
    /// never been treated as caches, and were cleared only wholesale — and
    /// for image books, not even then.
    ///
    /// A miss costs a re-decode, measured at 77-120 ms for a 1600x2400
    /// comic page, so a budget below a few pages trades a memory problem
    /// for a paging-back problem. The default is sized well clear of that.
    fn evict_to_budget(&mut self) {
        self.evict_keeping(None);
    }

    /// Evict, additionally sparing `keep` — the unit a caller is in the
    /// middle of building, which would otherwise be a candidate the moment
    /// it is not the current one (a prefetch, a search).
    fn evict_keeping(&mut self, keep: Option<usize>) {
        let pinned = self.spine;
        while self.cache_bytes() > self.cache_budget {
            // Oldest use first; a unit with no recorded use is older still.
            let victim = self
                .layouts
                .keys()
                .chain(self.images.keys())
                .copied()
                .filter(|spine| *spine != pinned && Some(*spine) != keep)
                .min_by_key(|spine| self.used_at.get(spine).copied().unwrap_or(0));
            let Some(victim) = victim else {
                // Only the pinned unit is left. One page over budget beats
                // a session with nothing to show.
                return;
            };
            self.layouts.remove(&victim);
            self.images.remove(&victim);
            self.loaded_units.remove(&victim);
            self.placeholders.remove(&victim);
            self.used_at.remove(&victim);
        }
    }

    /// The device-pixel size [`render`](Self::render) and
    /// [`render_into`](Self::render_into) produce, rotation included.
    ///
    /// A host allocating its own surface needs this before it can ask for
    /// one; `None` until metrics are set.
    pub fn render_size(&self) -> Option<(u32, u32)> {
        let metrics = self.metrics?;
        let scale = metrics.dpi_scale;
        let (w, h) = (
            (metrics.size.w * scale) as u32,
            (metrics.size.h * scale) as u32,
        );
        Some(if metrics.rotation.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        })
    }

    /// Rasterize the current page straight into a buffer the caller owns.
    ///
    /// Returns whether anything was drawn: `false` for a size that does not
    /// match [`render_size`](Self::render_size), a `stride` narrower than a
    /// row, a buffer too short, or nothing to render.
    ///
    /// This exists because every host platform already owns the memory it
    /// wants the page in — Android's `AndroidBitmap_lockPixels` hands back
    /// the bitmap's own backing store, iOS has a `CGBitmapContext`, a WASM
    /// build has the `Uint8ClampedArray` behind an `ImageData` — and
    /// [`render`](Self::render) allocating a fresh `Pixmap` per page turn
    /// meant a copy into that memory on every one.
    ///
    /// Pixels are premultiplied RGBA8888, which is what
    /// `Bitmap.Config.ARGB_8888` holds natively, so the common Android path
    /// is a straight rasterize with no conversion at either end.
    ///
    /// **What is actually free.** An unrotated page whose `stride` is
    /// exactly `width * 4` is rasterized directly into `dst`: no
    /// allocation, no copy. A rotated page, or a padded stride, still goes
    /// through an intermediate — rotation cannot be done in place and
    /// tiny-skia cannot target a strided buffer — and is copied row by row.
    /// Both are correct; only the first is free, and it is the one every
    /// named platform hits.
    pub fn render_into(&mut self, dst: &mut [u8], width: u32, height: u32, stride: usize) -> bool {
        let Some((want_w, want_h)) = self.render_size() else {
            return false;
        };
        if width != want_w || height != want_h {
            return false;
        }
        let row = match (width as usize).checked_mul(4) {
            Some(row) if stride >= row => row,
            _ => return false,
        };
        if dst.len() < stride.saturating_mul(height as usize) {
            return false;
        }

        let Some(metrics) = self.metrics else {
            return false;
        };
        // The free path: the engine's output and the host's buffer are the
        // same shape, so rasterize into it and stop.
        if metrics.rotation == Rotation::None && stride == row {
            let Some(mut target) = tiny_skia::PixmapMut::from_bytes(dst, width, height) else {
                return false;
            };
            return self.render_page_into(&mut target).is_some();
        }

        let Some(pixmap) = self.render() else {
            return false;
        };
        for (y, line) in pixmap.data().chunks_exact(row).enumerate() {
            let at = y * stride;
            dst[at..at + row].copy_from_slice(line);
        }
        true
    }

    /// Rasterize and quantize into `target`, which must already be the
    /// unrotated page size. Shared by both entry points.
    fn render_page_into(&mut self, target: &mut tiny_skia::PixmapMut<'_>) -> Option<()> {
        let metrics = self.metrics?;
        let dl = self.frame()?.list;
        let spine = self.spine;
        let scale = metrics.dpi_scale;
        // `render_size` derives from the metrics and this derives from the
        // display list. They agree — the page is laid out to the metrics —
        // but a caller's buffer is sized from the first and rasterized
        // against the second, so disagreeing must fail rather than
        // misdraw. `render_size_matches_what_render_produces` is the test
        // that keeps them honest.
        if target.width() != (dl.size.w * scale) as u32
            || target.height() != (dl.size.h * scale) as u32
        {
            return None;
        }
        let images = self.images.get(&spine).unwrap_or(&self.empty_images);
        self.renderer
            .render(&dl, &mut self.fonts, images, scale, target);
        // Panel policy is backend-neutral: the same conversion a GPU shell
        // would apply to its own pixels. Dithering is scoped to where the
        // page has images, because diffusing error through body text
        // stipples every glyph's edge and not diffusing it through a
        // photograph flattens the photograph.
        let (w, h) = (target.width(), target.height());
        let dithered = dl.dither_regions(scale);
        chapbook_paint::quantize_regions(target.data_mut(), w, h, self.pixel_format, &dithered);
        Some(())
    }

    pub fn render(&mut self) -> Option<tiny_skia::Pixmap> {
        let metrics = self.metrics?;
        let dl = self.frame()?.list;
        let spine = self.spine;
        let scale = metrics.dpi_scale;
        let mut pixmap =
            tiny_skia::Pixmap::new((dl.size.w * scale) as u32, (dl.size.h * scale) as u32)?;
        let images = self.images.get(&spine).unwrap_or(&self.empty_images);
        self.renderer
            .render(&dl, &mut self.fonts, images, scale, &mut pixmap.as_mut());
        // Panel policy is backend-neutral: the same conversion a GPU shell
        // would apply to its own pixels. Dithering is scoped to where the
        // page has images, because diffusing error through body text
        // stipples every glyph's edge and not diffusing it through a
        // photograph flattens the photograph.
        let (w, h) = (pixmap.width(), pixmap.height());
        let dithered = dl.dither_regions(scale);
        chapbook_paint::quantize_regions(pixmap.data_mut(), w, h, self.pixel_format, &dithered);
        if metrics.rotation == Rotation::None {
            return Some(pixmap);
        }
        let turned = chapbook_paint::rotate(pixmap.data(), w, h, metrics.rotation);
        let (tw, th) = if metrics.rotation.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        };
        tiny_skia::Pixmap::from_vec(turned.into_owned(), tiny_skia::IntSize::from_wh(tw, th)?)
    }

    // ---- Persistence ----

    /// Locator offset of the current page (0 for comics).
    pub fn current_offset(&self) -> u32 {
        self.layouts
            .get(&self.spine)
            .and_then(|l| l.char_map.get(self.page).copied())
            .unwrap_or(0)
    }

    /// Capture the position as a full layered locator and persist it.
    /// Comics persist page-unit progression (see `chapbook_core::locator`).
    pub fn save_position(&mut self) {
        #[cfg(feature = "library")]
        {
            let Some(id) = self.book_id else {
                return;
            };
            let offset = self
                .layouts
                .get(&self.spine)
                .and_then(|l| l.char_map.get(self.page).copied())
                .unwrap_or(0);
            let Ok(item) = self.book.publication().spine_item(self.spine) else {
                return;
            };
            let href = item.href.clone();
            let locator = match self.book.publication().kind() {
                BookKind::Epub => {
                    let ctx = self.unit_char_context();
                    LayeredLocator::capture(
                        &href, self.spine, &ctx.text, offset, ctx.prior, ctx.total,
                    )
                }
                // Image books: the progression unit is pages.
                BookKind::Comic | BookKind::Pdf => LayeredLocator::capture(
                    &href,
                    self.spine,
                    "",
                    0,
                    self.spine as u64,
                    self.book.publication().spine().len() as u64,
                ),
            };
            let Some(library) = self.library_mut() else {
                return;
            };
            if let Err(e) = library.set_position(id, &locator) {
                log::error!("failed to save position: {e}");
            }
        }
    }

    // ---- Layout ----

    /// Lay out one spine unit (cached per metrics+settings): the full
    /// text pipeline for EPUB chapters, a fabricated single-image page for
    /// comic units.
    fn layout_unit(&mut self, spine: usize) -> Option<&ChapterLayout> {
        let metrics = self.metrics?;
        if !self.layouts.contains_key(&spine) {
            let built = match self.book.publication().kind() {
                BookKind::Epub => self.layout_text_unit(spine, &metrics),
                // Image books load on the worker; a placeholder shows
                // until the decoded unit arrives. Their pixels live in the
                // image store already (inserted by poll_loaded).
                BookKind::Comic | BookKind::Pdf => {
                    let layout = self.layout_image_unit(spine, &metrics);
                    self.layouts.insert(spine, layout);
                    self.touch(spine);
                    self.evict_keeping(Some(spine));
                    return self.layouts.get(&spine);
                }
            };
            let (layout, images) = match built {
                Some((layout, images)) => (layout, Some(images)),
                None => return None,
            };
            self.layouts.insert(spine, layout);
            if let Some(images) = images {
                self.images.insert(spine, images);
            }
        }
        // Touch on every call, not only on a build: eviction order is
        // about what is being *read*, and a unit served from cache is the
        // most-used thing there is.
        self.touch(spine);
        self.evict_keeping(Some(spine));
        self.layouts.get(&spine)
    }

    /// Build the one-page layout for an image-book unit. When the unit
    /// hasn't loaded yet, this queues it on the worker (plus a one-page
    /// prefetch) and returns an empty placeholder page — the shell redraws
    /// via the waker when the pixels arrive. PDF units also get their
    /// hidden text layer, scaled from natural (point) coordinates into the
    /// placed image rect, so selection works on them.
    fn layout_image_unit(&mut self, spine: usize, metrics: &PageMetrics) -> ChapterLayout {
        // Prefetch the next unit while we're here. Without a
        // background-loading format there is no thread to prefetch onto —
        // and no unit that could reach here to want one.
        #[cfg(feature = "_image-book")]
        if let Some(loader) = self.loader.as_mut() {
            let next = spine + 1;
            if next < self.book.publication().spine().len()
                && !self.loaded_units.contains_key(&next)
                && !self.load_errors.contains_key(&next)
            {
                loader.request(next);
            }
        }
        let Some(unit) = self.loaded_units.get(&spine) else {
            #[cfg(feature = "_image-book")]
            if !self.load_errors.contains_key(&spine) {
                if let Some(loader) = self.loader.as_mut() {
                    loader.request(spine);
                    self.placeholders.insert(spine);
                }
            }
            // Placeholder: an empty themed page until the load lands.
            let content = chapbook_core::Rect::new(
                metrics.margins.left,
                metrics.margins.top,
                metrics.content_width(),
                metrics.content_height(),
            );
            return ChapterLayout {
                pages: vec![chapbook_paint::Page {
                    size: metrics.size,
                    content,
                    fragments: Vec::new(),
                }],
                char_map: vec![0],
                anchors: HashMap::new(),
            };
        };
        let resource = spine as u64 + 1;
        let mut page = chapbook_paint::image_page(metrics, unit.width, unit.height, resource);
        if !unit.text.is_empty() {
            if let Some(image_rect) = page.fragments.first().map(|f| f.rect) {
                push_hidden_text(&mut page, &unit.text, unit.natural, image_rect);
            }
        }
        ChapterLayout {
            pages: vec![page],
            char_map: vec![0],
            anchors: HashMap::new(),
        }
    }

    fn layout_text_unit(
        &mut self,
        spine: usize,
        metrics: &PageMetrics,
    ) -> Option<(ChapterLayout, ImageStore)> {
        // Irrefutable when EPUB is the only format compiled in, and the
        // guard is still the right thing to write: which variants exist is
        // a build option, and this function is only correct for one of them.
        #[allow(irrefutable_let_patterns)]
        let OpenBook::Epub(epub) = &self.book
        else {
            return None;
        };
        let href = epub.spine_item(spine).ok()?.href.clone();
        let bytes = epub.unit_bytes(spine).ok()?;
        let mut doc = dom::parse_xhtml(&bytes, &href).ok()?;
        let css: Vec<(String, String)> = doc
            .stylesheet_sources()
            .iter()
            .filter_map(|s| match s {
                dom::StylesheetSource::Inline(t) => Some((t.clone(), href.clone())),
                dom::StylesheetSource::External(rel) => epub.resource(&href, rel).ok().map(|r| {
                    (
                        String::from_utf8_lossy(&r.data).into_owned(),
                        chapbook_epub::resolve_href(&href, rel),
                    )
                }),
            })
            .collect();

        for face in chapbook_layout::extract_font_faces(&css) {
            if !self.registered_fonts.insert(face.family.clone()) {
                continue;
            }
            for src in &face.sources {
                if let Ok(res) = epub.resource(&face.base, src) {
                    if chapbook_layout::register_font(&mut self.fonts, &face.family, res.data) {
                        break;
                    }
                }
            }
        }
        let images = chapbook_layout::collect_images(&doc, Some(&self.fonts), |img_href| {
            epub.resource(&href, img_href).ok().map(|r| r.data)
        });

        let sheets: Vec<String> = css.iter().map(|(text, _)| text.clone()).collect();
        let mut engine = cascade::StyleEngine::new(metrics, &self.settings);
        engine.set_author_sheets(&sheets);
        engine.style_document(&mut doc);
        // The document is parsed here and nowhere else; take its links
        // while we have it.
        self.links.insert(spine, dom::links(&doc));
        let layout = chapbook_layout::paginate(&doc, &sheets, metrics, &mut self.fonts, &images);
        Some((layout, images))
    }
}

/// Locator text of a text unit; `None` for image units (comics), which
/// pushes the restore chain to its progression tiers.
fn unit_locator_text(book: &dyn Publication, spine: usize) -> Option<String> {
    if book.kind() != BookKind::Epub {
        return None;
    }
    let href = book.spine_item(spine).ok()?.href.clone();
    let bytes = book.unit_bytes(spine).ok()?;
    let doc = dom::parse_xhtml(&bytes, &href).ok()?;
    Some(dom::locator_text(&doc))
}

/// Map a PDF unit's extracted text lines into hidden-text fragments over
/// the placed page image: natural (point) coordinates scale uniformly into
/// the image rect, glyph offsets become the page's locator space.
fn push_hidden_text(
    page: &mut chapbook_paint::Page,
    lines: &[chapbook_core::TextLine],
    natural: (f32, f32),
    image_rect: chapbook_core::Rect,
) {
    let factor = image_rect.size.w / natural.0.max(0.001);
    for line in lines {
        let Some(first) = line.glyphs.first() else {
            continue;
        };
        let min_x = line
            .glyphs
            .iter()
            .map(|g| g.x)
            .fold(f32::INFINITY, f32::min);
        let max_x = line
            .glyphs
            .iter()
            .map(|g| g.x + g.width)
            .fold(f32::NEG_INFINITY, f32::max);
        if max_x <= min_x {
            continue;
        }
        let rect = chapbook_core::Rect::new(
            image_rect.origin.x + min_x * factor,
            image_rect.origin.y + line.top * factor,
            (max_x - min_x) * factor,
            line.height * factor,
        );
        let glyphs: Vec<chapbook_paint::Glyph> = line
            .glyphs
            .iter()
            .map(|g| chapbook_paint::Glyph {
                id: 0,
                x: (g.x - min_x) * factor,
                y: 0.0,
                advance: g.width * factor,
                locator: g.offset,
            })
            .collect();
        page.fragments.push(chapbook_paint::Fragment {
            rect,
            kind: chapbook_paint::FragmentKind::HiddenText(chapbook_paint::LineFragment {
                baseline: rect.size.h * 0.8,
                runs: vec![chapbook_paint::GlyphRun {
                    font: cosmic_text::fontdb::ID::dummy(),
                    font_size: rect.size.h,
                    font_weight: 400,
                    color: chapbook_core::Rgba::new(0, 0, 0, 0),
                    glyphs,
                }],
                decorations: Vec::new(),
                text: line.text.clone(),
                locator_start: first.offset,
            }),
            tag: 0,
        });
    }
}

/// Lowercase a char one-to-one, so folded offsets still index the original
/// text. Multi-char expansions (ß → ss) keep their first char, which costs
/// a rare miss and buys exact offsets.
fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Text around a match with its whitespace collapsed, plus where the match
/// sits inside it. Locator text is raw source text — newlines and XHTML
/// indentation and all — so a results list needs it tidied.
fn context_around(chars: &[char], start: u32, end: u32) -> (String, (u32, u32)) {
    const CONTEXT_CHARS: usize = 40;
    let from = (start as usize).saturating_sub(CONTEXT_CHARS);
    let to = (end as usize + CONTEXT_CHARS).min(chars.len());

    let mut context = String::new();
    let (mut match_start, mut match_end) = (0u32, 0u32);
    let mut pending_space = false;
    for (i, c) in chars[from..to].iter().enumerate() {
        let at = from + i;
        let here = |context: &String| context.chars().count() as u32;
        if c.is_ascii_whitespace() {
            // A match ending at whitespace ends before the space that
            // collapsing may or may not emit.
            if at == end as usize && match_end == 0 {
                match_end = here(&context);
            }
            // Collapse runs, and never lead with one.
            pending_space = !context.is_empty();
            continue;
        }
        if pending_space {
            context.push(' ');
            pending_space = false;
        }
        // Both boundaries are recorded after any collapsed space is
        // emitted, so they index the string that comes back.
        if at == start as usize {
            match_start = here(&context);
        }
        if at == end as usize && match_end == 0 {
            match_end = here(&context);
        }
        context.push(*c);
    }
    if match_end == 0 {
        match_end = context.chars().count() as u32;
    }
    (context, (match_start, match_end))
}
