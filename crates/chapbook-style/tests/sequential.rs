//! Stylo must keep running sequentially, because a `unsafe impl` depends on it.
//!
//! stylo is a parallel style engine: `traverse_dom` takes an optional rayon
//! pool, and `STYLE_THREAD_POOL` will spin one up on demand. Chapbook always
//! passes `None`, and `chapbook-dom`'s `unsafe impl Send for Node` /
//! `unsafe impl Sync for Node` cite exactly that when they justify the
//! `Cell`/`UnsafeCell` state a traversal mutates: the traversal is the only
//! writer, and there is only ever one of it.
//!
//! So handing `traverse_dom` a pool would not merely make layout parallel.
//! It would silently invalidate a safety argument, in a build that still
//! compiles and mostly still works — the worst shape a defect can take.
//! Until this was written the invariant was held up by a comment.
//!
//! Source text, not behaviour: the property is "nobody wrote the other
//! thing", and observing the live pool would force the `LazyLock` and
//! spawn the very threads under test.

use std::path::{Path, PathBuf};

/// Every `.rs` file under a crate's `src`, recursively.
fn sources(crate_dir: &str) -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("read source");
                out.push((path, text));
            }
        }
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(crate_dir)
        .join("src");
    let mut out = Vec::new();
    walk(&dir, &mut out);
    assert!(!out.is_empty(), "no sources under {}", dir.display());
    out
}

/// The crates that drive, define, or consume stylo's traversal.
const STYLO_SIDE: &[&str] = &["chapbook-style", "chapbook-dom", "chapbook-layout"];

#[test]
fn every_traversal_is_handed_no_thread_pool() {
    let mut seen = 0;
    for krate in STYLO_SIDE {
        for (path, text) in sources(krate) {
            for (offset, _) in text.match_indices("traverse_dom(") {
                // The argument list, however rustfmt has since broken it.
                let call = &text[offset..];
                let end = call.find(");").map(|i| i + 2).unwrap_or(call.len());
                let args = &call[.."traverse_dom(".len() + end];
                seen += 1;
                assert!(
                    args.contains("None"),
                    "{}: traverse_dom is being given a thread pool.\n{args}\n\n\
                     chapbook-dom's `unsafe impl Send/Sync for Node` is only sound \
                     while the style traversal is the single writer of each node's \
                     interior-mutable state. A parallel traversal races it. If this \
                     is deliberate, that unsafe impl has to be re-argued first.",
                    path.display(),
                );
            }
        }
    }
    assert!(
        seen > 0,
        "no traverse_dom call found — did the call site move?"
    );
}

#[test]
fn the_style_side_never_reaches_for_rayon() {
    for krate in STYLO_SIDE {
        for (path, text) in sources(krate) {
            for line in text.lines() {
                let code = line.split("//").next().unwrap_or(line);
                assert!(
                    !code.contains("rayon") && !code.contains("STYLE_THREAD_POOL"),
                    "{}: {}\n\nstylo's pool stays unbuilt. `STYLE_THREAD_POOL` is a \
                     `LazyLock`, so it costs nothing while nothing dereferences it — \
                     and merely reading it to check spawns the threads.",
                    path.display(),
                    line.trim(),
                );
            }
        }
    }
}
