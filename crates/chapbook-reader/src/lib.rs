//! The shared reading session: everything a viewer shell needs that isn't
//! windowing.
//!
//! [`Session`] owns the open publication (EPUB, local CBZ, or an OPDS-PSE
//! stream — dispatched by [`Session::open`]), the font system, per-chapter
//! layout and image caches, reading settings, the selection, and the
//! library glue (import/match on open, layered-locator persistence on
//! save). Shells — winit, GTK, anything with a keyboard and a pixel
//! buffer — translate input events into `Session` calls and blit the
//! [`Session::render`] result.
//!
//! Text units run the full dom→stylo→layout pipeline; comic units fabricate
//! a one-page [`ChapterLayout`] around a single scaled image fragment, so
//! navigation, the char map, and position persistence are one code path.
//!
//! Blocking caveat, inherited from `Publication::unit_bytes`: layout of a
//! not-yet-cached unit may block on I/O (seconds, for a cold PSE page).
//! These shells are dev harnesses and call it on the UI thread anyway —
//! the documented debt; a loader thread slots in here, not in the shells.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

mod loader;
use loader::{DecodedUnit, LoadSource, Loader};

use chapbook_core::{
    BookKind, ChapbookError, LayeredLocator, PageMetrics, Point, Publication, ReadingSettings,
    Result,
};
use chapbook_layout::ChapterLayout;
use chapbook_paint::{ImageStore, Selection};

pub use chapbook_core;
pub use chapbook_render_tinyskia::tiny_skia;

/// The open publication. Text units need the concrete EPUB surface
/// (relative resource resolution, stylesheets) that deliberately isn't on
/// the `Publication` trait, so the session keeps the concrete type.
enum OpenBook {
    Epub(Box<chapbook_epub::Book>),
    Comic(Arc<dyn Publication + Send + Sync>),
    /// PDFs keep their concrete type: the loader renders straight to RGBA
    /// and extracts the text layer through it.
    Pdf(Arc<chapbook_pdf::PdfBook>),
}

impl OpenBook {
    fn publication(&self) -> &dyn Publication {
        match self {
            OpenBook::Epub(book) => book.as_ref(),
            OpenBook::Comic(comic) => comic.as_ref(),
            OpenBook::Pdf(pdf) => pdf.as_ref(),
        }
    }

    fn load_source(&self) -> Option<LoadSource> {
        match self {
            OpenBook::Epub(_) => None,
            OpenBook::Comic(comic) => Some(LoadSource::Comic(comic.clone())),
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
    text: Vec<chapbook_pdf::TextLine>,
    natural: (f32, f32),
}

/// One open book and everything needed to read it.
pub struct Session {
    book: OpenBook,
    title: String,
    fonts: cosmic_text::FontSystem,
    renderer: chapbook_render_tinyskia::Renderer,
    settings: ReadingSettings,
    metrics: Option<PageMetrics>,
    layouts: HashMap<usize, ChapterLayout>,
    images: HashMap<usize, ImageStore>,
    registered_fonts: HashSet<String>,
    spine: usize,
    page: usize,
    /// Restored char offset, turned into a page once the unit lays out.
    pending_offset: Option<u32>,
    /// Worker for image-book units (comics, PDFs); `None` for EPUBs.
    loader: Option<Loader>,
    /// Metrics-independent metadata of loaded units (pixels live in
    /// `images`, which image books never clear on relayout).
    loaded_units: HashMap<usize, LoadedUnit>,
    load_errors: HashMap<usize, String>,
    /// Units currently showing a placeholder page (relaid once loaded).
    placeholders: HashSet<usize>,
    waker: WakerCell,
    /// Selection anchor and cursor as locator offsets (unordered).
    selection: Option<(u32, u32)>,
    library: Option<chapbook_library::Library>,
    book_id: Option<chapbook_library::BookId>,
}

impl Session {
    /// Open a book from a path (`.epub`, `.cbz`) or an `http(s)://` OPDS
    /// URL (resolved to a PSE page stream). Local books are matched into
    /// the library (fingerprint, then identifier for replaced editions,
    /// else imported) and their stored position restored; streams skip the
    /// library (no local file to fingerprint) but honor `pse:lastRead`.
    pub fn open(source: &str) -> Result<Session> {
        let mut library =
            chapbook_library::Library::open(&chapbook_library::Library::default_dir())
                .map_err(|e| eprintln!("chapbook: library unavailable: {e}"))
                .ok();

        let (book, book_id, start_spine, pending_offset): (
            OpenBook,
            Option<chapbook_library::BookId>,
            usize,
            Option<u32>,
        ) = if source.starts_with("http://") || source.starts_with("https://") {
            let mut client = chapbook_opds::OpdsClient::new();
            if let (Ok(user), Ok(pass)) = (
                std::env::var("CHAPBOOK_OPDS_USER"),
                std::env::var("CHAPBOOK_OPDS_PASSWORD"),
            ) {
                client.set_basic_auth(&user, &pass);
            }
            let cache = chapbook_library::Library::default_dir().join("pse-cache");
            let comic = chapbook_opds::StreamedComic::open(client, source, &cache)
                .map_err(ChapbookError::from)?;
            let resume = comic.resume_page().unwrap_or(0);
            (OpenBook::Comic(Arc::new(comic)), None, resume, None)
        } else {
            let path = Path::new(source);
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            let book = match ext.as_deref() {
                Some("cbz") => OpenBook::Comic(Arc::new(chapbook_cbz::ComicBook::open(path)?)),
                Some("pdf") => OpenBook::Pdf(Arc::new(chapbook_pdf::PdfBook::open(path)?)),
                _ => OpenBook::Epub(Box::new(chapbook_epub::Book::open(path)?)),
            };

            let mut same_edition = true;
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
                lib.import(path, book.publication().metadata()).ok()
            });

            let (start_spine, pending_offset) = match (&library, book_id) {
                (Some(lib), Some(id)) => match lib.position(id) {
                    Ok(Some(stored)) => {
                        let (locator, tier) = chapbook_library::restore_position(
                            &stored.locator,
                            same_edition,
                            book.publication().spine(),
                            |i| unit_locator_text(book.publication(), i),
                        );
                        eprintln!(
                            "chapbook: resuming at unit {} ({tier:?})",
                            locator.spine_index + 1
                        );
                        (locator.spine_index, Some(locator.char_offset))
                    }
                    _ => (0, None),
                },
                _ => (0, None),
            };
            (book, book_id, start_spine, pending_offset)
        };

        let title = book
            .publication()
            .metadata()
            .title
            .clone()
            .unwrap_or_else(|| "chapbook".to_string());
        let waker: WakerCell = Arc::new(Mutex::new(None));
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
        Ok(Session {
            book,
            title,
            fonts: chapbook_layout::system_font_system(),
            renderer: chapbook_render_tinyskia::Renderer::new(),
            settings: ReadingSettings::default(),
            metrics: None,
            layouts: HashMap::new(),
            images: HashMap::new(),
            registered_fonts: HashSet::new(),
            spine: start_spine,
            page: 0,
            pending_offset,
            loader,
            loaded_units: HashMap::new(),
            load_errors: HashMap::new(),
            placeholders: HashSet::new(),
            waker,
            selection: None,
            library,
            book_id,
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
    /// anything arrived (the shell should redraw).
    pub fn poll_loaded(&mut self) -> bool {
        let Some(loader) = self.loader.as_mut() else {
            return false;
        };
        let results = loader.drain();
        let mut any = false;
        for (spine, result) in results {
            any = true;
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
                    eprintln!("chapbook: page {} failed to load: {message}", spine + 1);
                    self.load_errors.insert(spine, message);
                }
            }
            if self.placeholders.remove(&spine) {
                self.layouts.remove(&spine);
            }
        }
        any
    }

    /// Whether background loads are in flight (placeholder pages showing).
    pub fn has_pending_loads(&self) -> bool {
        self.loader.as_ref().is_some_and(Loader::has_pending)
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

    pub fn settings(&self) -> &ReadingSettings {
        &self.settings
    }

    /// Page count of the current unit (`0` until metrics are known).
    pub fn page_count(&mut self) -> usize {
        let spine = self.spine;
        self.layout_unit(spine).map_or(0, |l| l.pages.len())
    }

    /// Set page geometry (window size in CSS px + margins + dpi scale).
    /// A change relayouts, keeping the reading position via the char map.
    pub fn set_metrics(&mut self, metrics: PageMetrics) {
        if self.metrics == Some(metrics) {
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
    }

    // ---- Navigation ----

    pub fn next_page(&mut self) {
        let count = self.page_count();
        if self.page + 1 < count {
            self.page += 1;
        } else if self.spine + 1 < self.spine_len() {
            self.spine += 1;
            self.page = 0;
        }
        self.selection = None;
    }

    pub fn prev_page(&mut self) {
        if self.page > 0 {
            self.page -= 1;
        } else if self.spine > 0 {
            self.spine -= 1;
            let spine = self.spine;
            self.page = self
                .layout_unit(spine)
                .map_or(0, |l| l.pages.len())
                .saturating_sub(1);
        }
        self.selection = None;
    }

    pub fn next_unit(&mut self) {
        if self.spine + 1 < self.spine_len() {
            self.spine += 1;
            self.page = 0;
            self.selection = None;
        }
    }

    pub fn prev_unit(&mut self) {
        if self.spine > 0 {
            self.spine -= 1;
            self.page = 0;
            self.selection = None;
        }
    }

    // ---- Settings ----

    pub fn adjust_font(&mut self, delta: f32) {
        self.settings.base_font_px = (self.settings.base_font_px + delta).clamp(10.0, 40.0);
        self.relayout_keeping_position();
    }

    pub fn cycle_theme(&mut self) {
        self.settings.theme = self.settings.theme.cycle();
        self.relayout_keeping_position();
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
    }

    // ---- Selection ----

    /// Begin a selection at a page-space point (CSS px). Returns whether
    /// the point hit text.
    pub fn selection_begin(&mut self, x: f32, y: f32) -> bool {
        self.selection = None;
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
        }
    }

    pub fn selection_clear(&mut self) {
        self.selection = None;
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
        let (spine, page) = (self.spine, self.page);
        let layout = self.layout_unit(spine)?;
        layout.pages.get(page)?.offset_at(Point::new(x, y))
    }

    // ---- Rendering ----

    /// Rasterize the current page at the current metrics. `None` until
    /// metrics are set or when the unit fails to load.
    pub fn render(&mut self) -> Option<tiny_skia::Pixmap> {
        let metrics = self.metrics?;
        // Resolve a restored offset once the unit has laid out.
        if let Some(offset) = self.pending_offset {
            let spine = self.spine;
            if let Some(layout) = self.layout_unit(spine) {
                self.page = layout.page_of(offset);
                self.pending_offset = None;
            }
        }
        let selection = self.selected_range().map(|(start, end)| Selection {
            start,
            end,
            color: self.settings.theme.selection(),
        });
        let background = self.settings.theme.background();
        let (spine, page_idx) = (self.spine, self.page);
        let page_count = self.layout_unit(spine).map_or(0, |l| l.pages.len());
        if page_count == 0 {
            return None;
        }
        self.page = self.page.min(page_count - 1);
        let page_idx = page_idx.min(page_count - 1);
        let layout = self.layouts.get(&spine)?;
        let page = layout.pages.get(page_idx)?;
        let dl = chapbook_paint::build_display_list(page, background, selection);

        let scale = metrics.dpi_scale;
        let mut pixmap =
            tiny_skia::Pixmap::new((dl.size.w * scale) as u32, (dl.size.h * scale) as u32)?;
        let empty = ImageStore::default();
        let images = self.images.get(&spine).unwrap_or(&empty);
        self.renderer
            .render(&dl, &mut self.fonts, images, scale, &mut pixmap);
        Some(pixmap)
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
        let (Some(library), Some(id)) = (self.library.as_mut(), self.book_id) else {
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
                let mut prior_chars = 0u64;
                let mut total_chars = 0u64;
                let mut current_text = String::new();
                for i in 0..self.book.publication().spine().len() {
                    let text = unit_locator_text(self.book.publication(), i).unwrap_or_default();
                    let chars = text.chars().count() as u64;
                    if i < self.spine {
                        prior_chars += chars;
                    }
                    if i == self.spine {
                        current_text = text;
                    }
                    total_chars += chars;
                }
                LayeredLocator::capture(
                    &href,
                    self.spine,
                    &current_text,
                    offset,
                    prior_chars,
                    total_chars,
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
        if let Err(e) = library.set_position(id, &locator) {
            eprintln!("chapbook: failed to save position: {e}");
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
        self.layouts.get(&spine)
    }

    /// Build the one-page layout for an image-book unit. When the unit
    /// hasn't loaded yet, this queues it on the worker (plus a one-page
    /// prefetch) and returns an empty placeholder page — the shell redraws
    /// via the waker when the pixels arrive. PDF units also get their
    /// hidden text layer, scaled from natural (point) coordinates into the
    /// placed image rect, so selection works on them.
    fn layout_image_unit(&mut self, spine: usize, metrics: &PageMetrics) -> ChapterLayout {
        // Prefetch the next unit while we're here.
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
        let OpenBook::Epub(epub) = &self.book else {
            return None;
        };
        let href = epub.spine_item(spine).ok()?.href.clone();
        let bytes = epub.unit_bytes(spine).ok()?;
        let mut doc = chapbook_dom::parse_xhtml(&bytes, &href).ok()?;
        let css: Vec<(String, String)> = doc
            .stylesheet_sources()
            .iter()
            .filter_map(|s| match s {
                chapbook_dom::StylesheetSource::Inline(t) => Some((t.clone(), href.clone())),
                chapbook_dom::StylesheetSource::External(rel) => {
                    epub.resource(&href, rel).ok().map(|r| {
                        (
                            String::from_utf8_lossy(&r.data).into_owned(),
                            chapbook_epub::resolve_href(&href, rel),
                        )
                    })
                }
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
        let images = chapbook_layout::collect_images(&doc, |img_href| {
            epub.resource(&href, img_href).ok().map(|r| r.data)
        });

        let sheets: Vec<String> = css.iter().map(|(text, _)| text.clone()).collect();
        let mut engine = chapbook_style::StyleEngine::new(metrics, &self.settings);
        engine.set_author_sheets(&sheets);
        engine.style_document(&mut doc);
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
    let doc = chapbook_dom::parse_xhtml(&bytes, &href).ok()?;
    Some(chapbook_dom::locator_text(&doc))
}

/// Map a PDF unit's extracted text lines into hidden-text fragments over
/// the placed page image: natural (point) coordinates scale uniformly into
/// the image rect, glyph offsets become the page's locator space.
fn push_hidden_text(
    page: &mut chapbook_paint::Page,
    lines: &[chapbook_pdf::TextLine],
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
