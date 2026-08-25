//! CLI subcommands, one milestone each. Output formats are deterministic —
//! the snapshot tests in `tests/` capture them verbatim.

use std::path::Path;

use chapbook_core::{PageMetrics, Publication, ReadingSettings, Result, TocEntry};
use chapbook_epub::Book;
use chapbook_layout::{cascade, dom};

/// Open a local book by extension: `.cbz`/`.pdf` -> image-per-page
/// producers, else EPUB.
fn open_publication(path: &Path) -> Result<Box<dyn chapbook_core::Publication>> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("cbz") => Ok(Box::new(chapbook_cbz::ComicBook::open(path)?)),
        Some("pdf") => Ok(Box::new(chapbook_pdf::PdfBook::open(path)?)),
        _ => Ok(Box::new(Book::open(path)?)),
    }
}

fn is_image_book(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cbz") || e.eq_ignore_ascii_case("pdf"))
}

pub fn meta(book_path: &Path) -> Result<String> {
    // EPUBs report their fixed-layout status; the trait surface doesn't
    // carry it (comics are inherently fixed pages).
    let layout_note = if is_image_book(book_path) {
        "pages (image book)"
    } else if Book::open(book_path)?.is_fixed_layout() {
        "fixed (unsupported)"
    } else {
        "reflowable"
    };
    let book = open_publication(book_path)?;
    let md = book.metadata();
    let mut out = String::new();
    push_field(&mut out, "title", md.title.as_deref());
    for author in &md.authors {
        push_field(&mut out, "author", Some(author));
    }
    push_field(&mut out, "language", md.language.as_deref());
    push_field(&mut out, "identifier", md.identifier.as_deref());
    push_field(&mut out, "version", Some(&md.format_version));
    push_field(&mut out, "layout", Some(layout_note));
    out.push_str(&format!("spine:      {} items\n", book.spine().len()));
    Ok(out)
}

/// Any publication's toc, not just an EPUB's: a PDF's outline and a CBZ's
/// ComicInfo bookmarks land here too.
pub fn toc(book_path: &Path) -> Result<String> {
    let book = open_publication(book_path)?;
    let mut out = String::new();
    fn walk(entries: &[TocEntry], depth: usize, out: &mut String) {
        for e in entries {
            let target = match (&e.href, &e.fragment) {
                (Some(h), Some(f)) => format!("{h}#{f}"),
                (Some(h), None) => h.clone(),
                _ => "-".to_string(),
            };
            out.push_str(&format!(
                "{}{}  [{}]\n",
                "  ".repeat(depth),
                e.label,
                target
            ));
            walk(&e.children, depth + 1, out);
        }
    }
    walk(book.toc(), 0, &mut out);
    Ok(out)
}

pub fn text(epub: &Path, spine: Option<usize>) -> Result<String> {
    let book = Book::open(epub)?;
    let indices: Vec<usize> = match spine {
        Some(i) => vec![i],
        None => (0..book.spine().len()).collect(),
    };
    let mut out = String::new();
    for i in indices {
        let href = book.spine_item(i)?.href.clone();
        let bytes = book.unit_bytes(i)?;
        let doc = dom::parse_xhtml(&bytes, &href)?;
        if spine.is_none() {
            out.push_str(&format!("==== spine {i} ({href}) ====\n"));
        }
        out.push_str(&dom::extract_text(&doc));
    }
    Ok(out)
}

pub fn styles(epub: &Path, spine: usize) -> Result<String> {
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let (doc, _css, notes) = styled_chapter(&book, spine, &href, &ReadingSettings::default())?;
    Ok(notes + &cascade::dump_computed_styles(&doc))
}

/// Register @font-face fonts into `fonts` and decode the chapter's images.
fn load_chapter_assets(
    book: &Book,
    chapter_href: &str,
    doc: &dom::Document,
    css_pairs: &[(String, String)],
    fonts: &mut cosmic_text::FontSystem,
) -> chapbook_paint::ImageStore {
    for face in chapbook_layout::extract_font_faces(css_pairs) {
        for src in &face.sources {
            if let Ok(res) = book.resource(&face.base, src) {
                if chapbook_layout::register_font(fonts, &face.family, res.data) {
                    break;
                }
            }
        }
    }
    chapbook_layout::collect_images(doc, |href| {
        book.resource(chapter_href, href).ok().map(|r| r.data)
    })
}

pub fn layout(epub: &Path, spine: usize) -> Result<String> {
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let (doc, css, notes) = styled_chapter(&book, spine, &href, &ReadingSettings::default())?;

    // Deterministic fonts: vendored fixture faces only, never host fonts.
    let fonts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    let (mut fonts, _) = chapbook_layout::build_font_system(&chapbook_core::FontSource::embedded(
        &fonts_dir,
        "Crimson Text",
    ))?;
    let images = load_chapter_assets(&book, &href, &doc, &css, &mut fonts);
    let sheets: Vec<String> = css.iter().map(|(text, _)| text.clone()).collect();
    let layout =
        chapbook_layout::paginate(&doc, &sheets, &PageMetrics::default(), &mut fonts, &images);

    let mut out = notes;
    out.push_str(&format!("pages: {}\n", layout.pages.len()));
    let mut anchors: Vec<_> = layout.anchors.iter().collect();
    anchors.sort();
    for (id, page) in anchors {
        out.push_str(&format!("anchor #{id} -> page {page}\n"));
    }
    for (i, page) in layout.pages.iter().enumerate() {
        out.push_str(&format!(
            "-- page {i} (locator start {})\n",
            layout.char_map[i]
        ));
        for fragment in &page.fragments {
            let r = &fragment.rect;
            match &fragment.kind {
                chapbook_paint::FragmentKind::Line(line) => {
                    out.push_str(&format!(
                        "  [{:7.1},{:7.1} {:6.1}x{:5.1}] base={:5.1} loc={:<5} {:?}\n",
                        r.origin.x,
                        r.origin.y,
                        r.size.w,
                        r.size.h,
                        line.baseline,
                        line.locator_start,
                        line.text,
                    ));
                }
                other => out.push_str(&format!(
                    "  [{:7.1},{:7.1} {:6.1}x{:5.1}] {other:?}\n",
                    r.origin.x, r.origin.y, r.size.w, r.size.h
                )),
            }
        }
    }
    Ok(out)
}

pub fn render(
    epub: &Path,
    spine: usize,
    page: usize,
    out: &Path,
    theme: chapbook_core::Theme,
) -> Result<String> {
    if is_image_book(epub) {
        return render_image_book(epub, spine, out, theme);
    }
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let settings = ReadingSettings {
        theme,
        ..ReadingSettings::default()
    };
    let (doc, css, _notes) = styled_chapter(&book, spine, &href, &settings)?;

    let fonts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    let (mut fonts, _) = chapbook_layout::build_font_system(&chapbook_core::FontSource::embedded(
        &fonts_dir,
        "Crimson Text",
    ))?;
    let images = load_chapter_assets(&book, &href, &doc, &css, &mut fonts);
    let sheets: Vec<String> = css.iter().map(|(text, _)| text.clone()).collect();
    let metrics = PageMetrics::default();
    let layout = chapbook_layout::paginate(&doc, &sheets, &metrics, &mut fonts, &images);

    let page_data = layout.pages.get(page).ok_or_else(|| {
        chapbook_core::ChapbookError::Layout(format!(
            "page {page} out of range (chapter has {})",
            layout.pages.len()
        ))
    })?;

    let dl = chapbook_paint::build_display_list(page_data, theme.background(), &[]);
    let scale = metrics.dpi_scale;
    let mut pixmap = chapbook_render_tinyskia::tiny_skia::Pixmap::new(
        (dl.size.w * scale) as u32,
        (dl.size.h * scale) as u32,
    )
    .ok_or_else(|| chapbook_core::ChapbookError::Layout("empty page size".into()))?;
    let mut renderer = chapbook_render_tinyskia::Renderer::new();
    renderer.render(&dl, &mut fonts, &images, scale, &mut pixmap);
    pixmap
        .save_png(out)
        .map_err(|e| chapbook_core::ChapbookError::Io(std::io::Error::other(e)))?;
    Ok(format!(
        "rendered spine {spine} page {page}/{} ({}x{}) to {}\n",
        layout.pages.len() - 1,
        pixmap.width(),
        pixmap.height(),
        out.display()
    ))
}

/// A chapter's stylesheets as `(css text, container path of the sheet)`.
type CssSheets = Vec<(String, String)>;

/// Parse + cascade one chapter: shared plumbing for `styles` and `layout`.
fn styled_chapter(
    book: &Book,
    spine: usize,
    href: &str,
    settings: &ReadingSettings,
) -> Result<(dom::Document, CssSheets, String)> {
    let bytes = book.unit_bytes(spine)?;
    let mut doc = dom::parse_xhtml(&bytes, href)?;

    // Author stylesheets in document order as (text, container path of the
    // declaring sheet): <style> contents inline (base = the chapter),
    // <link rel=stylesheet> resolved against the chapter (base = the
    // stylesheet itself, for @font-face url() resolution). A missing sheet
    // degrades to "no publisher styles from that link", noted in the output
    // so goldens surface it.
    let mut css: Vec<(String, String)> = Vec::new();
    let mut notes = String::new();
    for source in doc.stylesheet_sources() {
        match source {
            dom::StylesheetSource::Inline(text) => {
                css.push((text, href.to_string()));
            }
            dom::StylesheetSource::External(rel) => match book.resource(href, &rel) {
                Ok(res) => css.push((
                    String::from_utf8_lossy(&res.data).into_owned(),
                    chapbook_epub::resolve_href(href, &rel),
                )),
                Err(_) => notes.push_str(&format!("!! stylesheet not found: {rel}\n")),
            },
        }
    }

    let sheets: Vec<String> = css.iter().map(|(text, _)| text.clone()).collect();
    let mut engine = cascade::StyleEngine::new(&PageMetrics::default(), settings);
    engine.set_author_sheets(&sheets);
    engine.style_document(&mut doc);
    Ok((doc, css, notes))
}

fn push_field(out: &mut String, name: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push_str(&format!("{:<11} {v}\n", format!("{name}:")));
    }
}

fn opds_client(url: &str) -> chapbook_opds::OpdsClient {
    use chapbook_core::{CredentialLookup, CredentialStore, Freshness};

    let mut client = chapbook_opds::OpdsClient::with_ureq();
    // The dev CLI's store is the environment; a viewer on a platform with
    // real secret storage injects a different one and this code is the
    // same shape. Keyed by origin so a catalog URL's secret-bearing path
    // never becomes part of a key.
    let store = chapbook_core::EnvCredentials;
    if let Some(key) = chapbook_core::CredentialKey::http_origin(url) {
        if let CredentialLookup::Found(credential) = store.get(&key, Freshness::Cached) {
            client.set_authorization(credential.authorization);
        }
    }
    client
}

fn describe_opds_error(e: chapbook_opds::OpdsError) -> chapbook_core::ChapbookError {
    if let chapbook_opds::OpdsError::AuthRequired(Some(doc)) = &e {
        let flows: Vec<&str> = doc.authentication.iter().map(|f| f.kind.as_str()).collect();
        return chapbook_core::ChapbookError::Opds(format!(
            "authentication required by \"{}\" (flows: {}) — set {} / {}",
            doc.title,
            flows.join(", "),
            chapbook_core::EnvCredentials::USER_VAR,
            chapbook_core::EnvCredentials::PASSWORD_VAR,
        ));
    }
    chapbook_opds::to_chapbook_error(e)
}

fn dump_feed(feed: &chapbook_opds::Feed) -> String {
    let mut out = format!("{} [{:?}]\n", feed.title, feed.version);
    if let Some(total) = feed.totals.total_results {
        out.push_str(&format!("total: {total}"));
        if let Some(per) = feed.totals.items_per_page {
            out.push_str(&format!(" ({per}/page)"));
        }
        out.push('\n');
    }
    for (group, facets) in feed.facet_groups() {
        let names: Vec<String> = facets
            .iter()
            .map(|f| {
                let name = f.title.clone().unwrap_or_default();
                if f.active_facet {
                    format!("[{name}]")
                } else {
                    name
                }
            })
            .collect();
        out.push_str(&format!("facets/{group}: {}\n", names.join(" ")));
    }
    fn list_entry(out: &mut String, entry: &chapbook_opds::Entry) {
        out.push_str(&format!("- {}", entry.title));
        if !entry.authors.is_empty() {
            out.push_str(&format!(" — {}", entry.authors.join(", ")));
        }
        out.push('\n');
        for acq in entry.acquisitions() {
            let kind = acq
                .rel
                .iter()
                .find_map(|r| r.rsplit('/').next())
                .unwrap_or("acquisition");
            let mt = acq
                .media_type
                .as_ref()
                .map(|t| t.essence.clone())
                .unwrap_or_default();
            out.push_str(&format!("    {kind} {mt}: {}\n", acq.href));
        }
        if let Some(nav) = entry.navigation() {
            out.push_str(&format!("    -> {}\n", nav.href));
        }
        if let Some(stream) = entry.pse_stream() {
            out.push_str(&format!(
                "    pages: {} (stream){}\n",
                stream.pse_count.unwrap_or(0),
                stream
                    .pse_last_read
                    .map(|p| format!(" last-read {p}"))
                    .unwrap_or_default()
            ));
        }
    }
    for entry in &feed.entries {
        list_entry(&mut out, entry);
    }
    for group in &feed.groups {
        out.push_str(&format!("group: {}\n", group.title));
        for entry in &group.entries {
            list_entry(&mut out, entry);
        }
    }
    if let Some(next) = feed.next() {
        out.push_str(&format!("next: {}\n", next.href));
    }
    if let Some(previous) = feed.previous() {
        out.push_str(&format!("previous: {}\n", previous.href));
    }
    out
}

pub fn opds_ls(url: &str) -> Result<String> {
    let feed = opds_client(url).fetch(url).map_err(describe_opds_error)?;
    Ok(dump_feed(&feed))
}

pub fn opds_search(url: &str, query: &str) -> Result<String> {
    let client = opds_client(url);
    let feed = client.fetch(url).map_err(describe_opds_error)?;
    let results = client
        .search(&feed, url, query)
        .map_err(describe_opds_error)?;
    Ok(dump_feed(&results))
}

pub fn opds_get(url: &str, out: &Path) -> Result<String> {
    opds_client(url)
        .download(url, out)
        .map_err(describe_opds_error)?;
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    Ok(format!("downloaded {} ({size} bytes)\n", out.display()))
}

/// Import any publication. The library was built format-agnostic — it
/// takes a `BookMetadata` and keeps the source extension so the format can
/// be sniffed on reopen — but this entry point opened everything as an
/// EPUB, so importing a comic died inside the zip reader.
pub fn lib_import(book_path: &Path) -> Result<String> {
    let book = open_publication(book_path)?;
    let mut lib = chapbook_library::Library::open(&chapbook_library::Library::default_dir())?;
    let id = lib.import(book_path, book.metadata())?;
    let record = lib.book(id)?.expect("just imported");
    Ok(format!(
        "imported #{} \"{}\" ({} authors, {} spine items)\n",
        id.0,
        record.title,
        record.authors.len(),
        book.spine().len()
    ))
}

pub fn lib_ls() -> Result<String> {
    let lib = chapbook_library::Library::open(&chapbook_library::Library::default_dir())?;
    let books = lib.books(None)?;
    if books.is_empty() {
        return Ok("library is empty — chapbook lib import <book>\n".into());
    }
    let mut out = String::new();
    for book in books {
        out.push_str(&format!("#{:<4} {}", book.id.0, book.title));
        if !book.authors.is_empty() {
            out.push_str(&format!(" — {}", book.authors.join(", ")));
        }
        if let Some(position) = lib.position(book.id)? {
            out.push_str(&format!(
                "  [{:.0}%]",
                position.locator.book_progression * 100.0
            ));
        }
        out.push('\n');
    }
    Ok(out)
}

/// Convert between reading positions and EPUB CFIs. Encode with
/// `--spine N --offset M`; decode with `--cfi "epubcfi(...)"`. Resolution
/// needs only the parsed document — no styling.
pub fn cfi(
    epub: &Path,
    spine: Option<usize>,
    offset: Option<u32>,
    cfi_str: Option<&str>,
) -> Result<String> {
    let book = Book::open(epub)?;
    let chapter_doc = |spine: usize| -> Result<dom::Document> {
        let href = book.spine_item(spine)?.href.clone();
        let bytes = book.unit_bytes(spine)?;
        let doc = dom::parse_xhtml(&bytes, &href)?;
        Ok(doc)
    };
    match (spine, offset, cfi_str) {
        (Some(spine), Some(offset), None) => {
            let doc = chapter_doc(spine)?;
            let cfi = dom::cfi_for_offset(&doc, spine, offset)
                .ok_or_else(|| chapbook_core::ChapbookError::Cfi("chapter has no text".into()))?;
            Ok(format!("{cfi}\n"))
        }
        (None, None, Some(cfi_str)) => {
            let cfi = chapbook_core::Cfi::parse(cfi_str)?;
            let spine = cfi.spine_index().ok_or_else(|| {
                chapbook_core::ChapbookError::Cfi(
                    "package part does not address a spine item".into(),
                )
            })?;
            let doc = chapter_doc(spine)?;
            let offset = dom::offset_for_cfi(&doc, &cfi).ok_or_else(|| {
                chapbook_core::ChapbookError::Cfi("CFI does not resolve in this chapter".into())
            })?;
            let text = dom::locator_text(&doc);
            let chars: Vec<char> = text.chars().collect();
            let start = (offset as usize).saturating_sub(30);
            let end = (offset as usize + 30).min(chars.len());
            let excerpt: String = chars[start..end].iter().collect();
            Ok(format!(
                "spine {spine} offset {offset}\n…{}…\n",
                excerpt.replace(['\n', '\r'], " ")
            ))
        }
        _ => Err(chapbook_core::ChapbookError::Cfi(
            "pass either --spine N --offset M (encode) or --cfi CFI (decode)".into(),
        )),
    }
}

/// Render one image-book page (CBZ or PDF): decode the image,
/// scale-to-fit page model, no dom/stylo/shaping anywhere in the path.
fn render_image_book(
    path: &Path,
    spine: usize,
    out: &Path,
    theme: chapbook_core::Theme,
) -> Result<String> {
    let book = open_publication(path)?;
    let bytes = book.unit_bytes(spine)?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|e| chapbook_core::ChapbookError::BookMalformed(format!("page image: {e}")))?
        .to_rgba8();
    let (w, h) = decoded.dimensions();
    let mut images = chapbook_paint::ImageStore::default();
    images.insert(1, w, h, decoded.into_raw());

    let metrics = PageMetrics::default();
    let page = chapbook_paint::image_page(&metrics, w, h, 1);
    let dl = chapbook_paint::build_display_list(&page, theme.background(), &[]);
    let scale = metrics.dpi_scale;
    let mut pixmap = chapbook_render_tinyskia::tiny_skia::Pixmap::new(
        (dl.size.w * scale) as u32,
        (dl.size.h * scale) as u32,
    )
    .ok_or_else(|| chapbook_core::ChapbookError::Layout("empty page size".into()))?;
    let fonts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    let (mut fonts, _) = chapbook_layout::build_font_system(&chapbook_core::FontSource::embedded(
        &fonts_dir,
        "Crimson Text",
    ))?;
    let mut renderer = chapbook_render_tinyskia::Renderer::new();
    renderer.render(&dl, &mut fonts, &images, scale, &mut pixmap);
    pixmap
        .save_png(out)
        .map_err(|e| chapbook_core::ChapbookError::Io(std::io::Error::other(e)))?;
    Ok(format!(
        "rendered page {spine}/{} ({}x{}) to {}\n",
        book.spine().len(),
        pixmap.width(),
        pixmap.height(),
        out.display()
    ))
}
