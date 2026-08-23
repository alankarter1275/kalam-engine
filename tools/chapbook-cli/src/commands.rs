//! CLI subcommands, one milestone each. Output formats are deterministic —
//! the snapshot tests in `tests/` capture them verbatim.

use std::path::Path;

use chapbook_core::{PageMetrics, Publication, ReadingSettings, Result, TocEntry};
use chapbook_epub::Book;

pub fn meta(epub: &Path) -> Result<String> {
    let book = Book::open(epub)?;
    let md = book.metadata();
    let mut out = String::new();
    push_field(&mut out, "title", md.title.as_deref());
    for author in &md.authors {
        push_field(&mut out, "author", Some(author));
    }
    push_field(&mut out, "language", md.language.as_deref());
    push_field(&mut out, "identifier", md.identifier.as_deref());
    push_field(&mut out, "version", Some(&md.format_version));
    push_field(
        &mut out,
        "layout",
        Some(if book.is_fixed_layout() {
            "fixed (unsupported)"
        } else {
            "reflowable"
        }),
    );
    out.push_str(&format!("spine:      {} items\n", book.spine().len()));
    Ok(out)
}

pub fn toc(epub: &Path) -> Result<String> {
    let book = Book::open(epub)?;
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
        let doc = chapbook_dom::parse_xhtml(&bytes, &href)?;
        if spine.is_none() {
            out.push_str(&format!("==== spine {i} ({href}) ====\n"));
        }
        out.push_str(&chapbook_dom::extract_text(&doc));
    }
    Ok(out)
}

pub fn styles(epub: &Path, spine: usize) -> Result<String> {
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let (doc, _css, notes) = styled_chapter(&book, spine, &href)?;
    Ok(notes + &chapbook_style::dump_computed_styles(&doc))
}

/// Register @font-face fonts into `fonts` and decode the chapter's images.
fn load_chapter_assets(
    book: &Book,
    chapter_href: &str,
    doc: &chapbook_dom::Document,
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
    let (doc, css, notes) = styled_chapter(&book, spine, &href)?;

    // Deterministic fonts: vendored fixture faces only, never host fonts.
    let fonts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    let mut fonts = chapbook_layout::fixture_font_system(&fonts_dir, "Crimson Text");
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

pub fn render(epub: &Path, spine: usize, page: usize, out: &Path) -> Result<String> {
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let (doc, css, _notes) = styled_chapter(&book, spine, &href)?;

    let fonts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    let mut fonts = chapbook_layout::fixture_font_system(&fonts_dir, "Crimson Text");
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

    let dl = chapbook_paint::build_display_list(page_data, chapbook_core::Rgba::WHITE);
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
) -> Result<(chapbook_dom::Document, CssSheets, String)> {
    let bytes = book.unit_bytes(spine)?;
    let mut doc = chapbook_dom::parse_xhtml(&bytes, href)?;

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
            chapbook_dom::StylesheetSource::Inline(text) => {
                css.push((text, href.to_string()));
            }
            chapbook_dom::StylesheetSource::External(rel) => match book.resource(href, &rel) {
                Ok(res) => css.push((
                    String::from_utf8_lossy(&res.data).into_owned(),
                    chapbook_epub::resolve_href(href, &rel),
                )),
                Err(_) => notes.push_str(&format!("!! stylesheet not found: {rel}\n")),
            },
        }
    }

    let sheets: Vec<String> = css.iter().map(|(text, _)| text.clone()).collect();
    let mut engine =
        chapbook_style::StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
    engine.set_author_sheets(&sheets);
    engine.style_document(&mut doc);
    Ok((doc, css, notes))
}

fn push_field(out: &mut String, name: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push_str(&format!("{:<11} {v}\n", format!("{name}:")));
    }
}

fn opds_client() -> chapbook_opds::OpdsClient {
    let mut client = chapbook_opds::OpdsClient::new();
    // Credentials via environment for the dev CLI; the viewer will prompt
    // using the Authentication Document instead.
    if let (Ok(user), Ok(pass)) = (
        std::env::var("CHAPBOOK_OPDS_USER"),
        std::env::var("CHAPBOOK_OPDS_PASSWORD"),
    ) {
        client.set_basic_auth(&user, &pass);
    }
    client
}

fn describe_opds_error(e: chapbook_opds::OpdsError) -> chapbook_core::ChapbookError {
    if let chapbook_opds::OpdsError::AuthRequired(Some(doc)) = &e {
        let flows: Vec<&str> = doc.authentication.iter().map(|f| f.kind.as_str()).collect();
        return chapbook_core::ChapbookError::Opds(format!(
            "authentication required by \"{}\" (flows: {}) — set CHAPBOOK_OPDS_USER / CHAPBOOK_OPDS_PASSWORD",
            doc.title,
            flows.join(", ")
        ));
    }
    e.into()
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
    let feed = opds_client().fetch(url).map_err(describe_opds_error)?;
    Ok(dump_feed(&feed))
}

pub fn opds_search(url: &str, query: &str) -> Result<String> {
    let client = opds_client();
    let feed = client.fetch(url).map_err(describe_opds_error)?;
    let results = client
        .search(&feed, url, query)
        .map_err(describe_opds_error)?;
    Ok(dump_feed(&results))
}

pub fn opds_get(url: &str, out: &Path) -> Result<String> {
    opds_client()
        .download(url, out)
        .map_err(describe_opds_error)?;
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    Ok(format!("downloaded {} ({size} bytes)\n", out.display()))
}

pub fn lib_import(epub: &Path) -> Result<String> {
    let book = Book::open(epub)?;
    let mut lib = chapbook_library::Library::open(&chapbook_library::Library::default_dir())?;
    let id = lib.import(epub, book.metadata())?;
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
        return Ok("library is empty — chapbook lib import <book.epub>\n".into());
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
