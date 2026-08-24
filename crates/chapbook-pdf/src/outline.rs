//! The document outline — PDF's bookmarks — as a [`TocEntry`] tree.
//!
//! The catalog's `/Outlines` is a doubly-linked tree of item dictionaries:
//! `/First` descends, `/Next` walks a level, `/Title` names the entry, and
//! `/Dest` (or a `/GoTo` action's `/D`) says where it lands. A destination
//! names its page by object reference, so turning one into a spine index is
//! a lookup against the page objects hayro already resolved.
//!
//! Two things here are defensive rather than decorative. The links are
//! author-supplied and nothing forbids a cycle, so a visited set and a depth
//! cap bound the walk. And every step is fallible without being fatal: an
//! entry whose destination we cannot resolve keeps its label and loses only
//! its target, because a table of contents with a dead row is still more
//! use than no table of contents.

use std::collections::{HashMap, HashSet};

use chapbook_core::TocEntry;
use hayro::hayro_syntax::object::{
    Array, Dict, MaybeRef, Name as PdfName, Object, ObjectIdentifier, String as PdfString,
};
use hayro::hayro_syntax::Pdf;

use crate::strings::text_string;

/// Real outlines nest a handful deep; this only has to stop a malformed one.
const MAX_DEPTH: usize = 32;

/// Everything a destination needs to become a spine index.
struct Targets {
    /// Page object id -> spine index.
    pages: HashMap<ObjectIdentifier, usize>,
    /// Named destination -> spine index, pre-resolved.
    named: HashMap<Vec<u8>, usize>,
    page_count: usize,
}

/// Read the outline, or an empty toc if the document has none.
pub(crate) fn read(pdf: &Pdf) -> Vec<TocEntry> {
    let xref = pdf.xref();
    let Some(catalog) = xref.get::<Dict>(xref.root_id()) else {
        return Vec::new();
    };
    let Some(root) = catalog.get::<Dict>(b"Outlines") else {
        return Vec::new();
    };

    let pages: HashMap<ObjectIdentifier, usize> = pdf
        .pages()
        .iter()
        .enumerate()
        .filter_map(|(i, page)| Some((page.raw().obj_id()?, i)))
        .collect();
    let mut targets = Targets {
        page_count: pages.len(),
        pages,
        named: HashMap::new(),
    };
    // Resolved against a `targets` whose `named` is still empty, which only
    // matters for a named destination pointing at another named destination
    // — not a thing the spec allows.
    let named = named_destinations(&catalog, &targets);
    targets.named = named;

    let mut out = Vec::new();
    let mut visited = HashSet::new();
    walk(&root, &targets, 0, &mut visited, &mut out);
    out
}

/// Walk one level of `/First`/`/Next` siblings under `parent`.
fn walk(
    parent: &Dict,
    targets: &Targets,
    depth: usize,
    visited: &mut HashSet<ObjectIdentifier>,
    out: &mut Vec<TocEntry>,
) {
    let mut node = parent.get::<Dict>(b"First");
    while let Some(item) = node {
        // An item with no id is a direct (non-indirect) dict, which cannot
        // be the target of a back-link and so cannot close a cycle.
        if item.obj_id().is_some_and(|id| !visited.insert(id)) {
            break;
        }
        let spine_index = destination(&item, targets);
        let mut children = Vec::new();
        if depth < MAX_DEPTH {
            walk(&item, targets, depth + 1, visited, &mut children);
        }
        out.push(TocEntry {
            label: item
                .get::<PdfString>(b"Title")
                .map(|t| text_string(t.as_bytes()))
                .unwrap_or_default(),
            // PDF spine hrefs are synthetic page names; see `PdfBook::open`.
            href: spine_index.map(|i| format!("page-{}", i + 1)),
            // Fragments are a text-format notion; a PDF destination's
            // in-page coordinates have nowhere to go in a locator.
            fragment: None,
            spine_index,
            children,
        });
        node = item.get::<Dict>(b"Next");
    }
}

/// The spine index an outline item points at: `/Dest`, or the `/D` of an
/// attached `/GoTo` action. Other action types (`/URI`, `/GoToR`) leave the
/// document, so they resolve to nothing here even when they carry a `/D`.
fn destination(item: &Dict, targets: &Targets) -> Option<usize> {
    let dest = match item.get::<Object>(b"Dest") {
        Some(dest) => dest,
        None => {
            let action = item.get::<Dict>(b"A")?;
            if action.get::<PdfName>(b"S")?.as_ref() != b"GoTo" {
                return None;
            }
            action.get::<Object>(b"D")?
        }
    };
    resolve(dest, targets)
}

/// Resolve a destination object to a spine index. A destination is an
/// explicit array, a name or string naming one, or a dict wrapping one
/// under `/D`.
fn resolve(dest: Object, targets: &Targets) -> Option<usize> {
    // Bounded rather than recursive: `/D` pointing at another `/D` is
    // malformed, and unrolling it here means no unbounded self-reference.
    let mut dest = dest;
    for _ in 0..4 {
        match dest {
            Object::Array(array) => return page_of(&array, targets),
            Object::Name(name) => return targets.named.get(name.as_ref()).copied(),
            Object::String(s) => return targets.named.get(s.as_bytes()).copied(),
            Object::Dict(d) => dest = d.get::<Object>(b"D")?,
            _ => return None,
        }
    }
    None
}

/// The page an explicit destination array names. Its first element is a
/// reference to the page object; the remainder is the view (`/XYZ`, `/Fit`),
/// which a reflowed reader has no use for.
fn page_of(array: &Array, targets: &Targets) -> Option<usize> {
    match array.raw_iter().next()? {
        MaybeRef::Ref(page) => targets.pages.get(&page.into()).copied(),
        // Destinations written for a *remote* document give the page by
        // number instead. They turn up in files assembled from others; take
        // it as a zero-based index, but only if it is one we have.
        MaybeRef::NotRef(object) => object
            .into_i32()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n < targets.page_count),
    }
}

/// Flatten both named-destination stores into name -> spine index: the
/// PDF 1.1 `/Dests` dictionary, and the `/Names /Dests` name tree that
/// replaced it. Producers still emit either, and some emit both.
fn named_destinations(catalog: &Dict, targets: &Targets) -> HashMap<Vec<u8>, usize> {
    let mut out = HashMap::new();
    if let Some(dests) = catalog.get::<Dict>(b"Dests") {
        for key in dests.keys() {
            if let Some(page) = dests
                .get::<Object>(key.as_ref())
                .and_then(|dest| resolve(dest, targets))
            {
                out.insert(key.as_ref().to_vec(), page);
            }
        }
    }
    if let Some(tree) = catalog
        .get::<Dict>(b"Names")
        .and_then(|names| names.get::<Dict>(b"Dests"))
    {
        name_tree(&tree, targets, 0, &mut out);
    }
    out
}

/// Walk a name tree: interior nodes carry `/Kids`, leaves carry `/Names` as
/// a flat key/value array.
fn name_tree(node: &Dict, targets: &Targets, depth: usize, out: &mut HashMap<Vec<u8>, usize>) {
    if depth > MAX_DEPTH {
        return;
    }
    if let Some(names) = node.get::<Array>(b"Names") {
        let mut iter = names.flex_iter();
        while let Some(key) = iter.next::<PdfString>() {
            let Some(value) = iter.next::<Object>() else {
                break;
            };
            if let Some(page) = resolve(value, targets) {
                out.insert(key.as_bytes().to_vec(), page);
            }
        }
    }
    if let Some(kids) = node.get::<Array>(b"Kids") {
        for kid in kids.iter::<Dict>() {
            name_tree(&kid, targets, depth + 1, out);
        }
    }
}
