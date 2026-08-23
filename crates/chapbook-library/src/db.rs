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
