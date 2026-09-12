//! Nothing that runs by default may depend on `fixtures/corpus/`.
//!
//! That directory is a download — `fixtures/fetch-corpus.sh` writes it and
//! `.gitignore` keeps it out of the repo — so a test that reads it passes
//! for whoever last ran the script and fails for everyone else. It is the
//! worst shape a test failure comes in: green on the machine of the person
//! who wrote it, red on a clean checkout and in CI, and the error is a
//! bare `No such file or directory` that names an ENOENT rather than the
//! missing step.
//!
//! Three tests had drifted into it before this one existed. The rule is
//! not "don't use the corpus" — the corpus is worth having, and the sweeps
//! that use it are real tests. The rule is that reaching for it puts a
//! test behind `#[ignore]`, where a clean checkout skips it and
//! `-- --ignored` runs it.
//!
//! Crude on purpose, in the same way `chapbook-ffi/tests/discipline.rs` is:
//! the shape being looked for is a string literal, and a syntax tree would
//! buy nothing.

use std::path::{Path, PathBuf};

/// Every `.rs` file under the workspace's crates and tools.
fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `target/` holds vendored sources and build output, and
                // nothing there is ours to hold to this rule.
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((path, text));
                }
            }
        }
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    walk(&root.join("crates"), &mut out);
    walk(&root.join("tools"), &mut out);
    assert!(!out.is_empty(), "found no sources to check");
    // Reported relative to the workspace root: the absolute path runs
    // through the `../..` above and is unreadable in a failure.
    for (path, _) in &mut out {
        if let Ok(rel) = path.strip_prefix(&root) {
            *path = rel.to_path_buf();
        }
    }
    out
}

/// The `#[test]` attribute that precedes `offset`, and whether the
/// attributes between the two carry `#[ignore]`.
///
/// Scanning backwards from the use rather than forwards from the test,
/// because a test function's body has no closing marker at column 0 that
/// is cheap to find, and a `#[test]` above the reference always is.
fn nearest_test_is_ignored(text: &str, offset: usize) -> Option<bool> {
    let before = &text[..offset];
    let at = before.rfind("#[test]")?;
    Some(before[at..].contains("#[ignore"))
}

#[test]
fn no_default_test_reads_the_downloaded_corpus() {
    let mut offenders = Vec::new();
    for (path, text) in sources() {
        // This file names the path it forbids, which would be a fine joke
        // to have to debug at some later date.
        if path
            .file_name()
            .is_some_and(|n| n == "fixture_discipline.rs")
        {
            continue;
        }
        for needle in ["\"corpus/", "corpus\", \"epub\"", "fixtures/corpus"] {
            let mut from = 0;
            while let Some(at) = text[from..].find(needle) {
                let at = from + at;
                from = at + needle.len();
                // A mention in a comment or a doc line is fine — the
                // ignored tests explain themselves, and so does the fetch
                // script's own documentation.
                let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
                let line = &text[line_start..at];
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if nearest_test_is_ignored(&text, at) == Some(false) {
                    offenders.push(format!("{}", path.display()));
                }
            }
        }
    }
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "these read fixtures/corpus/ from a test that runs by default, so a \
         clean checkout fails:\n  {}\nEither mark the test \
         `#[ignore = \"requires fixtures/fetch-corpus.sh\"]`, or point it at \
         a checked-in fixture — `fixtures/epub/long.epub` is the long one.",
        offenders.join("\n  ")
    );
}
