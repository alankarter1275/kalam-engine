//! The rules in the crate documentation, enforced against the source.
//!
//! "Every entry point is wrapped in `catch_unwind`" is a property that
//! holds by convention, and conventions decay silently: the thirty-seventh
//! function to be added is the one that forgets, and nothing fails until a
//! panic reaches a host and takes the process with it. So the convention is
//! read back off the source and asserted.
//!
//! Crude on purpose. A real check would want a syntax tree, and the cost of
//! that — a `syn` dependency in the gate — buys nothing here: the shape
//! being looked for is one line long and never legitimately absent.

use std::path::PathBuf;

fn sources() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("src/ is readable") {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_some_and(|e| e == "rs") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            out.push((name, std::fs::read_to_string(&path).expect("readable")));
        }
    }
    assert!(!out.is_empty(), "found no sources to check");
    out
}

/// Every `extern "C"` function, as `(file, name, body)`.
fn entry_points() -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    for (file, text) in sources() {
        let mut rest = text.as_str();
        while let Some(at) = rest.find(r#"extern "C" fn "#) {
            let after = &rest[at + r#"extern "C" fn "#.len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            // The body runs to the closing brace at column 0, which is what
            // rustfmt guarantees for a top-level item.
            let body_start = after.find('{').unwrap_or(0);
            let body_end = after[body_start..]
                .find("\n}")
                .map(|e| body_start + e)
                .unwrap_or(after.len());
            found.push((file.clone(), name, after[body_start..body_end].to_string()));
            rest = &after[body_start..];
        }
    }
    assert!(
        found.len() > 30,
        "expected the whole ABI, found {} entry points",
        found.len()
    );
    found
}

#[test]
fn every_entry_point_stops_unwinding_at_the_boundary() {
    // A panic crossing an `extern "C"` frame is undefined behaviour, and
    // under `panic = "abort"` it is a process kill the host cannot observe,
    // report or attribute to us.
    let missing: Vec<_> = entry_points()
        .into_iter()
        .filter(|(_, _, body)| !body.contains("guard("))
        .map(|(file, name, _)| format!("{file}: {name}"))
        .collect();
    assert!(
        missing.is_empty(),
        "these entry points do not wrap their body in `guard(...)`, so a \
         panic inside them would unwind into the host:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_entry_point_is_exported_under_its_c_name() {
    // Without `#[no_mangle]` the symbol a host links against does not
    // exist, and the failure arrives at `dlopen` or at link time on
    // somebody else's machine. This is the Rust-side half of the check
    // `android/build-jni.sh` performs against a built `.so`.
    for (file, text) in sources() {
        let mut rest = text.as_str();
        while let Some(at) = rest.find(r#"extern "C" fn "#) {
            let before = &rest[..at];
            let name: String = rest[at + r#"extern "C" fn "#.len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            // Look back over the attributes and doc comments attached to
            // this item — i.e. to the end of the previous item.
            let attrs = before.rsplit("\n\n").next().unwrap_or(before);
            assert!(
                attrs.contains("#[no_mangle]"),
                "{file}: {name} is `extern \"C\"` but not `#[no_mangle]`, so no \
                 host can link it"
            );
            rest = &rest[at + r#"extern "C" fn "#.len()..];
        }
    }
}

#[test]
fn nothing_hands_out_memory_the_host_would_have_to_free() {
    // The absence of `cb_free_string` is what makes a whole class of defect
    // impossible — a host freeing Rust's allocation with the wrong
    // allocator. It stays absent only if nothing starts returning owned
    // pointers, so: no entry point may return a `*mut c_char`, and the only
    // pointers returned at all are the opaque handles, which have matching
    // destructors in this same ABI.
    let header = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("include/chapbook.h"),
    )
    .expect("the header is checked in");
    assert!(
        !header.contains("char *cb_") && !header.contains("char* cb_"),
        "an entry point returns an owned string; use the buf/cap/needed idiom instead"
    );
    for destructor in ["cb_session_close", "cb_config_free", "cb_font_source_free"] {
        assert!(
            header.contains(destructor),
            "{destructor} is missing: every handle this ABI hands out needs one"
        );
    }
}
