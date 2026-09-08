//! CLI subcommands, one milestone each. Output formats are deterministic —
//! the snapshot tests in `tests/` capture them verbatim.

use std::path::Path;

use chapbook_core::{PageMetrics, Publication, ReadingSettings, Result, TocEntry};
use chapbook_epub::Book;
use chapbook_layout::{cascade, dom};

/// The fixture corpus's fonts: vendored faces only, never host fonts, so
/// every stage this CLI dumps is byte-identical on any machine.
///
/// Crimson Text answers all five CSS generics and covers the Latin corpus.
/// The second directory holds the Hebrew and Arabic faces, reached only
/// through per-script fallback — a Latin page never sees them, which is
/// why adding them moved no existing golden. It is separate because
/// `fixtures/fonts` is scanned recursively and pinned at four faces by a
/// test, and is what every other fixture falls back to.
///
/// Without the mapping the bidi fixture would still lay out, still
/// paginate and still render. It would render as tofu.
fn fixture_fonts() -> chapbook_core::FontSource {
    use chapbook_core::{Faces, FallbackFamilies, Fallbacks, FontSource, ScriptTag};

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut source = FontSource::embedded(fixtures.join("fonts"), "Crimson Text");
    source.faces.push(Faces::Dir(fixtures.join("fonts-bidi")));
    source.fallback = Fallbacks::Explicit(FallbackFamilies {
        common: Vec::new(),
        per_script: vec![
            (
                ScriptTag::new("Hebr").expect("Hebr is a script tag"),
                vec!["Noto Sans Hebrew".into()],
            ),
            (
                ScriptTag::new("Arab").expect("Arab is a script tag"),
                vec!["Noto Naskh Arabic".into()],
            ),
        ],
        forbidden: Vec::new(),
    });
    source
}

/// Open a local book. kalam: EPUB only — the CBZ and PDF producers were
/// removed with their crates (docs/kalam/PLAN.md §4). Kept as a
/// `Publication` trait object so the callers below did not change shape.
fn open_publication(path: &Path) -> Result<Box<dyn chapbook_core::Publication>> {
    Ok(Box::new(Book::open(path)?))
}

pub fn meta(book_path: &Path) -> Result<String> {
    // EPUBs report their fixed-layout status; the trait surface doesn't
    // carry it.
    let layout_note = if Book::open(book_path)?.is_fixed_layout() {
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
    chapbook_layout::collect_images(doc, Some(fonts), |href| {
        book.resource(chapter_href, href).ok().map(|r| r.data)
    })
}

pub fn layout(epub: &Path, spine: usize) -> Result<String> {
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let (doc, css, notes) = styled_chapter(&book, spine, &href, &ReadingSettings::default())?;

    let (mut fonts, _) = chapbook_layout::build_font_system(&fixture_fonts())?;
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
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let settings = ReadingSettings {
        theme,
        ..ReadingSettings::default()
    };
    let (doc, css, _notes) = styled_chapter(&book, spine, &href, &settings)?;

    let (mut fonts, _) = chapbook_layout::build_font_system(&fixture_fonts())?;
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
    renderer.render(&dl, &mut fonts, &images, scale, &mut pixmap.as_mut());
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

/// Import a publication into the library.
pub fn lib_import(book_path: &Path) -> Result<String> {
    let book = open_publication(book_path)?;
    let mut lib = open_library()?;
    let id = lib.import(book_path, book.as_ref())?;
    let record = lib.book(id)?.expect("just imported");
    Ok(format!(
        "imported #{} \"{}\" ({} authors, {} spine items)\n",
        id.0,
        record.title,
        record.authors.len(),
        book.spine().len()
    ))
}

/// The shelf, narrowed however the caller asked.
///
/// The default sort is `read` rather than `added`: someone running
/// `lib ls` is looking for what they were reading, which is the same
/// thing a shelf shows first.
pub fn lib_ls(
    search: Option<&str>,
    collection: Option<&str>,
    series: Option<&str>,
    state: Option<chapbook_library::ReadingState>,
    sort: chapbook_library::Sort,
    limit: Option<usize>,
) -> Result<String> {
    let lib = open_library()?;
    let collection = collection
        .map(|name| named_collection(&lib, name))
        .transpose()?;
    let books = lib.query(&chapbook_library::BookQuery {
        search,
        collection,
        series,
        state,
        sort,
        limit,
        offset: 0,
    })?;
    if books.is_empty() {
        // Distinguish "nothing here" from "nothing matched": the second
        // is a filter to relax, and telling a reader to import a book
        // when they have fifty is unhelpful.
        let narrowed =
            search.is_some() || collection.is_some() || series.is_some() || state.is_some();
        return Ok(if narrowed {
            "nothing on the shelf matches\n".into()
        } else {
            "library is empty — chapbook lib import <book>\n".into()
        });
    }
    let mut out = String::new();
    for book in books {
        out.push_str(&format!("#{:<4} {}", book.id.0, book.title));
        if !book.authors.is_empty() {
            out.push_str(&format!(" — {}", book.authors.join(", ")));
        }
        if let Some(series) = &book.series {
            out.push_str(&format!("  [{series}"));
            if let Some(index) = book.series_index {
                out.push_str(&format!(" #{}", trim_index(index)));
            }
            out.push(']');
        }
        out.push_str(&format!("  {}", describe_state(&book)));
        if !book.collections.is_empty() {
            let names: Vec<&str> = book.collections.iter().map(|c| c.name.as_str()).collect();
            out.push_str(&format!("  {{{}}}", names.join(", ")));
        }
        if book.cover_path.is_some() {
            out.push_str("  (cover)");
        }
        out.push('\n');
    }
    Ok(out)
}

/// Everything the library holds about one book, including the parts
/// `ls` has no room for.
pub fn lib_show(id: i64) -> Result<String> {
    let lib = open_library()?;
    let id = chapbook_library::BookId(id);
    let book = require_book(&lib, id)?;

    let mut out = String::new();
    out.push_str(&format!("#{}  {}\n", book.id.0, book.title));
    if !book.authors.is_empty() {
        out.push_str(&format!("  authors      {}\n", book.authors.join(", ")));
    }
    if let Some(series) = &book.series {
        let position = book
            .series_index
            .map(|i| format!(" #{}", trim_index(i)))
            .unwrap_or_default();
        out.push_str(&format!("  series       {series}{position}\n"));
    }
    if let Some(language) = &book.language {
        out.push_str(&format!("  language     {language}\n"));
    }
    if let Some(identifier) = &book.identifier {
        out.push_str(&format!("  identifier   {identifier}\n"));
    }
    out.push_str(&format!("  state        {}\n", describe_state(&book)));
    out.push_str(&format!("  fingerprint  {}\n", book.fingerprint));
    // Empty for an adopted book: the platform owns the file and the
    // shell owns the means of reaching it again.
    out.push_str(&format!(
        "  file         {}\n",
        if book.file_path.as_os_str().is_empty() {
            "(adopted — the shell holds the handle)".to_string()
        } else {
            book.file_path.display().to_string()
        }
    ));
    if let Some(cover) = &book.cover_path {
        out.push_str(&format!("  cover        {}\n", cover.display()));
    }
    if !book.collections.is_empty() {
        let names: Vec<&str> = book.collections.iter().map(|c| c.name.as_str()).collect();
        out.push_str(&format!("  collections  {}\n", names.join(", ")));
    }
    out.push_str(&format!("  annotations  {}\n", lib.annotations(id)?.len()));

    // Never the URLs themselves: a service URL may embed a per-user key,
    // and this prints to a terminal that scrolls into a bug report.
    let targets = lib.sync_targets(id)?;
    let services = [
        ("position", targets.progression_url.is_some()),
        ("annotations", targets.annotation_container.is_some()),
    ]
    .iter()
    .filter(|(_, present)| *present)
    .map(|(name, _)| *name)
    .collect::<Vec<_>>();
    out.push_str(&format!(
        "  syncs        {}\n",
        if services.is_empty() {
            "nothing (sideloaded)".to_string()
        } else {
            services.join(", ")
        }
    ));
    Ok(out)
}

pub fn lib_rm(id: i64) -> Result<String> {
    let mut lib = open_library()?;
    let id = chapbook_library::BookId(id);
    let record = require_book(&lib, id)?;
    lib.delete_book(id)?;
    Ok(format!(
        "removed #{} \"{}\" (annotations kept: a re-import finds them again)\n",
        id.0, record.title
    ))
}

pub fn lib_finish(id: i64, finished: bool) -> Result<String> {
    let mut lib = open_library()?;
    let id = chapbook_library::BookId(id);
    let record = require_book(&lib, id)?;
    lib.set_finished(id, finished)?;
    Ok(format!(
        "#{} \"{}\" is {}\n",
        id.0,
        record.title,
        if finished { "finished" } else { "unfinished" }
    ))
}

pub fn lib_series() -> Result<String> {
    let lib = open_library()?;
    let series = lib.series()?;
    if series.is_empty() {
        return Ok("no book on the shelf names a series\n".into());
    }
    let mut out = String::new();
    for (name, count) in series {
        out.push_str(&format!("{count:>4}  {name}\n"));
    }
    Ok(out)
}

pub fn lib_collections() -> Result<String> {
    let lib = open_library()?;
    let collections = lib.collections()?;
    if collections.is_empty() {
        return Ok("no collections — chapbook lib collection new <name>\n".into());
    }
    let mut out = String::new();
    for collection in collections {
        out.push_str(&format!("{:>4}  {}\n", collection.books, collection.name));
    }
    Ok(out)
}

pub fn lib_collection_new(name: &str) -> Result<String> {
    let mut lib = open_library()?;
    let id = lib.create_collection(name)?;
    Ok(format!("collection #{} \"{name}\"\n", id.0))
}

pub fn lib_collection_rename(name: &str, new_name: &str) -> Result<String> {
    let mut lib = open_library()?;
    let id = named_collection(&lib, name)?;
    lib.rename_collection(id, new_name)?;
    Ok(format!("\"{name}\" is now \"{new_name}\"\n"))
}

pub fn lib_collection_rm(name: &str) -> Result<String> {
    let mut lib = open_library()?;
    let id = named_collection(&lib, name)?;
    lib.delete_collection(id)?;
    Ok(format!("removed \"{name}\" (the books stayed)\n"))
}

/// Creating the collection if it does not exist: `create_collection` is
/// idempotent on the name, and asking someone to declare a shelf before
/// putting a book on it is a step with no question behind it.
pub fn lib_collection_add(name: &str, id: i64) -> Result<String> {
    let mut lib = open_library()?;
    let book = chapbook_library::BookId(id);
    let record = require_book(&lib, book)?;
    let collection = lib.create_collection(name)?;
    lib.add_to_collection(book, collection)?;
    Ok(format!("#{id} \"{}\" is in \"{name}\"\n", record.title))
}

pub fn lib_collection_remove(name: &str, id: i64) -> Result<String> {
    let mut lib = open_library()?;
    let book = chapbook_library::BookId(id);
    let record = require_book(&lib, book)?;
    let collection = named_collection(&lib, name)?;
    lib.remove_from_collection(book, collection)?;
    Ok(format!("#{id} \"{}\" is out of \"{name}\"\n", record.title))
}

fn open_library() -> Result<chapbook_library::Library> {
    chapbook_library::Library::open(&chapbook_library::Library::default_dir()?)
}

fn require_book(
    lib: &chapbook_library::Library,
    id: chapbook_library::BookId,
) -> Result<chapbook_library::BookRecord> {
    lib.book(id)?.ok_or_else(|| {
        chapbook_core::ChapbookError::Library(format!("no book #{} in the library", id.0))
    })
}

/// Collections are addressed by name here rather than by id: a person
/// typing at a terminal knows the name they gave it, and the id is not
/// printed anywhere they would have looked.
fn named_collection(
    lib: &chapbook_library::Library,
    name: &str,
) -> Result<chapbook_library::CollectionId> {
    lib.collections()?
        .into_iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
        .map(|c| c.id)
        .ok_or_else(|| {
            chapbook_core::ChapbookError::Library(format!(
                "no collection \"{name}\" — chapbook lib collection ls"
            ))
        })
}

/// "reading 42%", "finished", "unread" — the state with the progress
/// that qualifies it, where there is one.
fn describe_state(book: &chapbook_library::BookRecord) -> String {
    let percent = book
        .progress
        .map(|p| format!(" {:.0}%", p * 100.0))
        .unwrap_or_default();
    match book.state() {
        chapbook_library::ReadingState::Unread => "unread".to_string(),
        chapbook_library::ReadingState::Reading => format!("reading{percent}"),
        // The progress of a finished book is where the reader is now,
        // which may be the beginning again — so it is not shown beside
        // a word it would contradict.
        chapbook_library::ReadingState::Finished => "finished".to_string(),
    }
}

/// `2` rather than `2.0`, but `2.5` intact: series positions are whole
/// numbers except when they are not.
fn trim_index(index: f64) -> String {
    if index.fract() == 0.0 {
        format!("{index:.0}")
    } else {
        format!("{index}")
    }
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
