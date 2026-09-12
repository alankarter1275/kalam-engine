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
