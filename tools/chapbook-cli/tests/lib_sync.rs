//! `lib sync`, as far as it goes without a server.
//!
//! What is asserted here is the wiring the CLI owns: which books get
//! walked, what a book with no service is told, and that the device id is
//! minted once and then kept. The reconcile itself — push, pull, refusal,
//! conflict, the annotation container — is `chapbook-sync`'s own test
//! suite against a fake transport, and duplicating it here would prove
//! nothing extra while needing a network.
//!
//! One test, and its own file, for the reason `lib_shelf.rs` gives:
//! `CHAPBOOK_LIBRARY_DIR` is process-global, so a second test in this file
//! would race it. Separate integration files are separate processes.

use chapbook_cli::commands;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

#[test]
fn sync_says_what_it_can_and_cannot_reconcile() {
    let dir = std::env::temp_dir().join(format!("chapbook-cli-sync-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);

    // An empty shelf and a shelf where nothing syncs are the same answer
    // here, and it points at the thing that would fix it.
    let empty = commands::lib_sync(None).unwrap();
    assert!(empty.contains("no book in the library"), "{empty}");
    assert!(empty.contains("catalog entry"), "{empty}");

    // Nothing was contacted, so nothing needed a device identity yet.
    assert!(
        !dir.join("device").exists(),
        "a library with nothing to sync should not mint a device id"
    );

    commands::lib_import(&fixture("epub/minimal.epub")).unwrap();

    // A sideloaded book still has no service, and the whole-shelf walk
    // simply does not visit it.
    let still_none = commands::lib_sync(None).unwrap();
    assert!(
        still_none.contains("no book in the library"),
        "{still_none}"
    );

    // Named explicitly, it is reported — and *not* as a failure, because
    // having no service is the ordinary state of a sideloaded book.
    let named = commands::lib_sync(Some(1)).unwrap();
    assert!(named.contains("nothing (sideloaded)"), "{named}");
    assert!(named.contains("1 with no service"), "{named}");
    assert!(
        !named.contains("failed"),
        "a book with no service is not a failure: {named}"
    );

    // Building the engine minted one, and it is a URI a service can parse.
    let minted = std::fs::read_to_string(dir.join("device")).expect("a device id was written");
    assert!(minted.starts_with("urn:uuid:"), "{minted}");
    assert_eq!(minted.len(), "urn:uuid:".len() + 36, "{minted}");
    // Version 4, variant 1 — the two nibbles a reader of the URN checks.
    assert_eq!(minted.as_bytes()[9 + 14] as char, '4', "{minted}");
    assert!(
        matches!(minted.as_bytes()[9 + 19] as char, '8' | '9' | 'a' | 'b'),
        "{minted}"
    );

    // Kept, not re-minted: a fresh id every run would make "last read on
    // …" a list of strangers.
    let again = commands::lib_sync(Some(1)).unwrap();
    assert!(again.contains("nothing (sideloaded)"), "{again}");
    assert_eq!(
        minted,
        std::fs::read_to_string(dir.join("device")).unwrap(),
        "the device id must survive a second run"
    );

    // A book that is not there is an error, not an empty report.
    let missing = commands::lib_sync(Some(404)).unwrap_err();
    assert!(missing.to_string().contains("no book #404"), "{missing}");

    let _ = std::fs::remove_dir_all(&dir);
}
