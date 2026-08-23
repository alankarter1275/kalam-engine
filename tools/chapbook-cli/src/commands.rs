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
    let bytes = book.unit_bytes(spine)?;
    let mut doc = chapbook_dom::parse_xhtml(&bytes, &href)?;

    // Author stylesheets in document order: <style> contents inline,
    // <link rel=stylesheet> resolved against the chapter. A missing external
    // sheet degrades to "no publisher styles from that link", noted in the
    // output so goldens surface it.
    let mut css = Vec::new();
    let mut notes = String::new();
    for source in doc.stylesheet_sources() {
        match source {
            chapbook_dom::StylesheetSource::Inline(text) => css.push(text),
            chapbook_dom::StylesheetSource::External(rel) => match book.resource(&href, &rel) {
                Ok(res) => css.push(String::from_utf8_lossy(&res.data).into_owned()),
                Err(_) => notes.push_str(&format!("!! stylesheet not found: {rel}\n")),
            },
        }
    }

    let mut engine =
        chapbook_style::StyleEngine::new(&PageMetrics::default(), &ReadingSettings::default());
    engine.set_author_sheets(&css);
    engine.style_document(&mut doc);

    Ok(notes + &chapbook_style::dump_computed_styles(&doc))
}

fn push_field(out: &mut String, name: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push_str(&format!("{:<11} {v}\n", format!("{name}:")));
    }
}
