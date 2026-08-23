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

pub fn layout(epub: &Path, spine: usize) -> Result<String> {
    let book = Book::open(epub)?;
    let href = book.spine_item(spine)?.href.clone();
    let (doc, css, notes) = styled_chapter(&book, spine, &href)?;

    // Deterministic fonts: vendored fixture faces only, never host fonts.
    let fonts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    let mut fonts = chapbook_layout::fixture_font_system(&fonts_dir, "Crimson Text");
    let layout = chapbook_layout::paginate(&doc, &css, &PageMetrics::default(), &mut fonts);

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
    let metrics = PageMetrics::default();
    let layout = chapbook_layout::paginate(&doc, &css, &metrics, &mut fonts);

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
    renderer.render(&dl, &mut fonts, scale, &mut pixmap);
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

/// Parse + cascade one chapter: shared plumbing for `styles` and `layout`.
fn styled_chapter(
    book: &Book,
    spine: usize,
    href: &str,
) -> Result<(chapbook_dom::Document, Vec<String>, String)> {
    let bytes = book.unit_bytes(spine)?;
    let mut doc = chapbook_dom::parse_xhtml(&bytes, href)?;

    // Author stylesheets in document order: <style> contents inline,
    // <link rel=stylesheet> resolved against the chapter. A missing external
    // sheet degrades to "no publisher styles from that link", noted in the
    // output so goldens surface it.
    let mut css = Vec::new();
    let mut notes = String::new();
    for source in doc.stylesheet_sources() {
        match source {
            chapbook_dom::StylesheetSource::Inline(text) => css.push(text),
            chapbook_dom::StylesheetSource::External(rel) => match book.resource(href, &rel) {
                Ok(res) => css.push(String::from_utf8_lossy(&res.data).into_owned()),
                Err(_) => notes.push_str(&format!("!! stylesheet not found: {rel}\n")),
            },
        }
    }

    let mut engine =
        chapbook_style::StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
    engine.set_author_sheets(&css);
    engine.style_document(&mut doc);
    Ok((doc, css, notes))
}

fn push_field(out: &mut String, name: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push_str(&format!("{:<11} {v}\n", format!("{name}:")));
    }
}
