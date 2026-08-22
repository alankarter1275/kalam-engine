//! M1 subcommands: meta, toc, text. Output formats are deterministic — the
//! snapshot tests in `tests/` capture them verbatim.

use std::path::Path;

use chapbook_core::Result;
use chapbook_epub::{Book, TocEntry};

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
    push_field(&mut out, "version", Some(&md.epub_version));
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
        let bytes = book.chapter_xhtml(i)?;
        let doc = chapbook_dom::parse_xhtml(&bytes, &book.spine()[i].href)?;
        if spine.is_none() {
            out.push_str(&format!(
                "==== spine {} ({}) ====\n",
                i,
                book.spine()[i].href
            ));
        }
        out.push_str(&chapbook_dom::extract_text(&doc));
    }
    Ok(out)
}

fn push_field(out: &mut String, name: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push_str(&format!("{:<11} {v}\n", format!("{name}:")));
    }
}
