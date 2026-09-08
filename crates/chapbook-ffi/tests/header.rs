//! `include/chapbook.h` is a golden.
//!
//! The header is the Contract-tier artifact — the thing a Kotlin, Swift or C
//! host actually compiles against — and it is checked in so that consuming
//! this ABI needs no cbindgen, no build script and no Rust toolchain. That
//! only stays honest if something notices when the Rust moves and the header
//! does not, because the symptom otherwise is a link error on somebody
//! else's machine, or worse, a silently changed struct layout.
//!
//! So: regenerate into memory, compare, and fail with the diff. Update it
//! deliberately, the same way the render goldens are updated:
//!
//! ```text
//! UPDATE_FFI_HEADER=1 cargo test -p chapbook-ffi
//! ```
//!
//! and read what changed before committing it. A diff here is a diff every
//! downstream host will feel.

use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn generate() -> String {
    let config = cbindgen::Config::from_file(crate_dir().join("cbindgen.toml"))
        .expect("cbindgen.toml is readable and valid");
    let mut out = Vec::new();
    cbindgen::Builder::new()
        .with_crate(crate_dir())
        .with_config(config)
        .generate()
        .expect("the crate parses and the header generates")
        .write(&mut out);
    String::from_utf8(out).expect("cbindgen emits UTF-8")
}

#[test]
fn the_checked_in_header_matches_the_crate() {
    let path = crate_dir().join("include/chapbook.h");
    let generated = generate();

    if std::env::var_os("UPDATE_FFI_HEADER").is_some() {
        std::fs::create_dir_all(path.parent().expect("include/ has a parent"))
            .expect("include/ is creatable");
        std::fs::write(&path, &generated).expect("the header is writable");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}\n\nGenerate it with:\n    UPDATE_FFI_HEADER=1 cargo test -p chapbook-ffi",
            path.display()
        )
    });

    if committed == generated {
        return;
    }

    // A whole-file diff is unreadable; the first divergence is the message.
    let (mut line, mut detail) = (0usize, String::from("the files differ in length only"));
    for (n, (a, b)) in committed.lines().zip(generated.lines()).enumerate() {
        if a != b {
            line = n + 1;
            detail = format!("committed: {a}\ngenerated: {b}");
            break;
        }
    }
    panic!(
        "include/chapbook.h is out of date with the crate, from line {line}.\n\n{detail}\n\n\
         Regenerate and read the diff:\n    UPDATE_FFI_HEADER=1 cargo test -p chapbook-ffi"
    );
}

/// The header has to be *compilable C*, not merely plausible text.
///
/// cbindgen will happily emit something that does not parse — a doc comment
/// containing a comment terminator is the classic — and nothing else in
/// this workspace would notice, because nothing else in this workspace
/// compiles C. A host would notice, on their machine, at their integration.
///
/// Skipped rather than failed where there is no compiler, so the gate stays
/// runnable on a machine that has only a Rust toolchain.
#[test]
fn the_header_compiles_as_c() {
    let Some(cc) = c_compiler() else {
        eprintln!("skipped: no C compiler on PATH");
        return;
    };
    let dir = std::env::temp_dir().join(format!("chapbook-ffi-header-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir is creatable");
    let source = dir.join("check.c");
    // Include it twice: the include guard is part of the contract, and a
    // header that breaks on a second include breaks any real project.
    std::fs::write(
        &source,
        "#include \"chapbook.h\"\n#include \"chapbook.h\"\nint main(void){return (int)cb_abi_version();}\n",
    )
    .expect("the check source is writable");

    for std in ["c99", "c11", "c17"] {
        let out = std::process::Command::new(&cc)
            .args([
                &format!("-std={std}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(crate_dir().join("include"))
            .arg(&source)
            .output()
            .expect("the compiler runs");
        assert!(
            out.status.success(),
            "include/chapbook.h does not compile as {std}:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn c_compiler() -> Option<std::ffi::OsString> {
    if let Some(cc) = std::env::var_os("CC") {
        return Some(cc);
    }
    ["cc", "gcc", "clang"].into_iter().find_map(|name| {
        std::process::Command::new(name)
            .arg("--version")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|_| std::ffi::OsString::from(name))
    })
}
