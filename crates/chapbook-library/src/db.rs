//! Schema and migrations. `PRAGMA user_version` tracks the schema version;
//! migrations apply in order inside a transaction.
//!
//! Schema hygiene per docs/LOCATORS.md: stable ids, updated-at timestamps,
//! and soft deletes on positions/annotations — single-device today, but
//! sync later becomes serialization, not redesign.

use rusqlite::Connection;

use chapbook_core::{ChapbookError, Result};

const MIGRATIONS: &[&str] = &[
    // v1
    "
    CREATE TABLE books (
        id INTEGER PRIMARY KEY,
        title TEXT NOT NULL,
        language TEXT,
        identifier TEXT,
        file_path TEXT NOT NULL,
        source_path TEXT,
        -- Edition fingerprint (SHA-1 of file bytes). Book identity is the
        -- library id; a changed fingerprint triggers the re-anchor chain,
        -- never orphans state.
        fingerprint TEXT NOT NULL,
        added_at INTEGER NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_books_fingerprint ON books(fingerprint);

    CREATE TABLE authors (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    );
    CREATE TABLE book_authors (
        book_id INTEGER NOT NULL REFERENCES books(id),
        author_id INTEGER NOT NULL REFERENCES authors(id),
        position INTEGER NOT NULL,
        PRIMARY KEY (book_id, author_id)
    );

    -- One reading position per book: the full layered locator record.
    CREATE TABLE positions (
        book_id INTEGER PRIMARY KEY REFERENCES books(id),
        spine_href TEXT NOT NULL,
        spine_index INTEGER NOT NULL,
        char_offset INTEGER NOT NULL,
        locator_version INTEGER NOT NULL,
        quote_prefix TEXT NOT NULL,
        quote_exact TEXT NOT NULL,
        quote_suffix TEXT NOT NULL,
        spine_fraction REAL NOT NULL,
        book_progression REAL NOT NULL,
        updated_at INTEGER NOT NULL
    );

    -- Annotations carry layered locators per endpoint (a drifted highlight
    -- silently marks the wrong text, so the quote layer is mandatory).
    CREATE TABLE annotations (
        id INTEGER PRIMARY KEY,
        book_id INTEGER NOT NULL REFERENCES books(id),
        kind TEXT NOT NULL CHECK (kind IN ('bookmark','highlight','note')),
        start_spine_href TEXT NOT NULL,
        start_spine_index INTEGER NOT NULL,
        start_char_offset INTEGER NOT NULL,
        start_locator_version INTEGER NOT NULL,
        start_quote_prefix TEXT NOT NULL,
        start_quote_exact TEXT NOT NULL,
        start_quote_suffix TEXT NOT NULL,
        start_spine_fraction REAL NOT NULL,
        start_book_progression REAL NOT NULL,
        end_spine_href TEXT,
        end_spine_index INTEGER,
        end_char_offset INTEGER,
        end_locator_version INTEGER,
        end_quote_prefix TEXT,
        end_quote_exact TEXT,
        end_quote_suffix TEXT,
        end_spine_fraction REAL,
        end_book_progression REAL,
        note_text TEXT,
        color TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_annotations_book ON annotations(book_id, deleted);

    -- Catalog URLs may embed per-user API keys: opaque, never logged.
    CREATE TABLE opds_sources (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL,
        title TEXT,
        auth_user TEXT,
        auth_secret TEXT,
        added_at INTEGER NOT NULL,
        deleted INTEGER NOT NULL DEFAULT 0
    );
    ",
    // v2
    "
    -- Reading settings, stored complete per scope: book_id 0 is the
    -- reader's default and any other id is that book's override, so a
    -- customized book keeps its own settings when the default changes.
    -- No foreign key: 0 is deliberately not a book.
    CREATE TABLE reading_settings (
        book_id INTEGER PRIMARY KEY,
        base_font_px REAL NOT NULL,
        line_height REAL NOT NULL,
        justify INTEGER NOT NULL,
        publisher_styles INTEGER NOT NULL,
        theme TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );
    ",
    // v3
    "
    -- Secrets leave the library. A password sitting in plaintext in a
    -- SQLite file is a desktop habit that does not survive a phone, and
    -- keeping the column would mean two stores with the insecure one
    -- winning by accident. Credentials now live behind an injected
    -- `chapbook_core::CredentialStore` — Keychain, Keystore, Secret
    -- Service — keyed by this row's id. `auth_user` stays: an account name
    -- is a label, not a secret, and a settings screen needs it without
    -- unlocking anything.
    ALTER TABLE opds_sources DROP COLUMN auth_secret;
    ",
    // v4
    "
    -- What a shelf needs and could not ask for. A cover is the thing a
    -- reader recognises a book by, and every `Publication` can already
    -- produce one — but `import` only ever saw the metadata, so nothing
    -- captured it and a browsing UI would have had to reopen every book on
    -- every paint. NULL means no cover; the file lives beside the managed
    -- copy under `covers/`.
    ALTER TABLE books ADD COLUMN cover_path TEXT;
    ",
];

pub(crate) fn open_and_migrate(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| ChapbookError::Library(format!("open {}: {e}", path.display())))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(db_err)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(db_err)?;

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_err)?;
    let target = MIGRATIONS.len() as i64;
    if version > target {
        return Err(ChapbookError::Library(format!(
            "database schema v{version} is newer than this build supports (v{target})"
        )));
    }
    for (i, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let next = i as i64 + 1;
        conn.execute_batch(&format!(
            "BEGIN;\n{migration}\nPRAGMA user_version = {next};\nCOMMIT;"
        ))
        .map_err(|e| ChapbookError::Library(format!("migration to v{next}: {e}")))?;
    }
    Ok(conn)
}

pub(crate) fn db_err(e: rusqlite::Error) -> ChapbookError {
    ChapbookError::Library(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        use std::hash::{BuildHasher, Hasher};
        let suffix = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let dir = std::env::temp_dir().join(format!(
            "chapbook-db-test-{}-{name}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_fresh_database_has_nowhere_to_put_a_secret() {
        let dir = scratch("fresh");
        let conn = open_and_migrate(&dir.join("library.db")).unwrap();
        let names = columns(&conn, "opds_sources");
        assert!(
            !names.iter().any(|c| c == "auth_secret"),
            "secrets belong in the credential store: {names:?}"
        );
        assert!(names.iter().any(|c| c == "auth_user"), "{names:?}");
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upgrading_an_old_database_takes_the_plaintext_password_with_it() {
        // A v1 file written before the credential store existed, with a
        // password sitting in it. The migration has to remove the value,
        // not just stop reading it.
        let dir = scratch("upgrade");
        let path = dir.join("library.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!(
                "BEGIN;\n{}\nPRAGMA user_version = 1;\nCOMMIT;",
                MIGRATIONS[0]
            ))
            .unwrap();
            conn.execute(
                "INSERT INTO opds_sources (url, title, auth_user, auth_secret, added_at)
                 VALUES ('https://cat.example.com/opds/', 'Old', 'user', 'hunter2', 0)",
                [],
            )
            .unwrap();
        }

        let conn = open_and_migrate(&path).unwrap();
        let names = columns(&conn, "opds_sources");
        assert!(!names.iter().any(|c| c == "auth_secret"), "{names:?}");

        // The row survives; the account label survives; the password does
        // not exist anywhere in the file's page contents.
        let (url, user): (String, String) = conn
            .query_row("SELECT url, auth_user FROM opds_sources", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(url, "https://cat.example.com/opds/");
        assert_eq!(user, "user");
        conn.execute_batch("VACUUM").unwrap();
        drop(conn);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes.windows(7).any(|w| w == b"hunter2"),
            "the old plaintext password is still in the database file"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
