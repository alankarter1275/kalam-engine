//! The `lib` subcommands, driven end to end against a real library.
//!
//! Not a snapshot test: the output carries a temp directory and a
//! fingerprint, and normalizing those away would leave a snapshot of the
//! formatting rather than of the behaviour. What is asserted is what a
//! person running these would be looking for.
//!
//! One test, deliberately. `CHAPBOOK_LIBRARY_DIR` is process-global and
//! the commands take no directory argument — that is the point of the
//! variable — so the flow runs in order inside a single test rather than
//! racing itself across several.

use chapbook_cli::commands;
use chapbook_library::{ReadingState, Sort};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

fn ls(search: Option<&str>, collection: Option<&str>, state: Option<ReadingState>) -> String {
    commands::lib_ls(search, collection, None, state, Sort::Title, None).unwrap()
}

#[test]
fn the_shelf_commands_drive_a_library() {
    let dir = std::env::temp_dir().join(format!("chapbook-cli-shelf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);

    // An empty shelf says how to fill it.
    assert!(ls(None, None, None).contains("library is empty"));

    for name in [
        "epub/minimal.epub",
        "epub/series.epub",
        "epub/series-legacy.epub",
        "cbz/minimal.cbz",
    ] {
        commands::lib_import(&fixture(name)).unwrap();
    }

    let all = ls(None, None, None);
    assert_eq!(all.lines().count(), 4, "{all}");
    assert!(all.contains("The Fixture Cycle #2.5"), "{all}");
    assert!(
        all.contains("The Fixture Cycle #1"),
        "series index 1, not 1.0: {all}"
    );

    // Search reaches the series, and reaches it by prefix.
    let cycle = ls(Some("fixture cycle"), None, None);
    assert_eq!(cycle.lines().count(), 2, "{cycle}");
    assert!(ls(Some("leg"), None, None).contains("The Legacy Fixture"));
    // And a narrowed shelf that matches nothing says so as a filter
    // problem, not as an empty library.
    let none = ls(Some("nothing here"), None, None);
    assert!(
        none.contains("matches") && !none.contains("empty"),
        "{none}"
    );

    let series = commands::lib_series().unwrap();
    assert!(series.contains("The Fixture Cycle"), "{series}");
    assert!(series.contains("Cogs & Levers"), "{series}");

    // Collections, addressed by the name a person typed.
    assert!(commands::lib_collections()
        .unwrap()
        .contains("no collections"));
    commands::lib_collection_add("To Reread", 2).unwrap();
    commands::lib_collection_add("To Reread", 3).unwrap();
    assert!(commands::lib_collections()
        .unwrap()
        .contains("2  To Reread"));
    // Case-folded: a collection is not a different collection because
    // the shift key was not held.
    let members = ls(None, Some("to reread"), None);
    assert_eq!(members.lines().count(), 2, "{members}");
    assert!(members.contains("{To Reread}"), "{members}");

    commands::lib_collection_remove("To Reread", 3).unwrap();
    assert_eq!(ls(None, Some("To Reread"), None).lines().count(), 1);

    commands::lib_collection_rename("To Reread", "Favourites").unwrap();
    assert!(commands::lib_collections().unwrap().contains("Favourites"));
    commands::lib_collection_rm("Favourites").unwrap();
    assert!(commands::lib_collections()
        .unwrap()
        .contains("no collections"));
    assert_eq!(
        ls(None, None, None).lines().count(),
        4,
        "deleting a collection took a book with it"
    );

    // Reading state.
    assert_eq!(
        ls(None, None, Some(ReadingState::Unread)).lines().count(),
        4
    );
    commands::lib_finish(1, true).unwrap();
    let finished = ls(None, None, Some(ReadingState::Finished));
    assert_eq!(finished.lines().count(), 1, "{finished}");
    assert!(finished.contains("finished"), "{finished}");
    commands::lib_finish(1, false).unwrap();
    assert!(ls(None, None, Some(ReadingState::Finished)).contains("matches"));

    // `show` carries what `ls` has no room for — and never a service URL,
    // which may embed a per-user key.
    let shown = commands::lib_show(2).unwrap();
    assert!(shown.contains("The Fixture Cycle #2.5"), "{shown}");
    assert!(shown.contains("annotations  0"), "{shown}");
    assert!(shown.contains("nothing (sideloaded)"), "{shown}");

    // Naming something that is not there is an error with a way out of
    // it, not a panic and not a silent nothing.
    assert!(commands::lib_show(99).is_err());
    assert!(commands::lib_finish(99, true).is_err());
    let missing = commands::lib_collection_rm("Never Existed").unwrap_err();
    assert!(
        missing.to_string().contains("collection ls"),
        "the error should say how to find the real names: {missing}"
    );

    // Removing is soft, and putting the same file back is the same book.
    commands::lib_rm(1).unwrap();
    assert_eq!(ls(None, None, None).lines().count(), 3);
    let again = commands::lib_import(&fixture("epub/minimal.epub")).unwrap();
    assert!(
        again.contains("#1"),
        "a second row would orphan its marks: {again}"
    );
    assert_eq!(ls(None, None, None).lines().count(), 4);

    std::fs::remove_dir_all(&dir).ok();
}
