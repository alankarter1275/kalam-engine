//! Local bookshelf persistence: imported books and their metadata, reading
//! positions (full [`LayeredLocator`] records), bookmarks/highlights/notes,
//! and saved OPDS sources. SQLite via rusqlite (bundled, WAL), with
//! `user_version`-pragma migrations.
//!
//! Book identity ≠ file identity: books are keyed by library id, the file
//! hash is an *edition fingerprint*. A changed fingerprint routes position
//! restore through the re-anchor chain in [`restore_position`] — state is
//! re-anchored, never orphaned.

mod db;
mod restore;

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha1::{Digest, Sha1};

use chapbook_core::{BookMetadata, LayeredLocator, Quote, ReadingSettings, Result, Theme};

use crate::db::db_err;
pub use crate::restore::{restore_position, RestoreTier};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BookId(pub i64);

#[derive(Debug, Clone)]
pub struct BookRecord {
    pub id: BookId,
    pub title: String,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    /// The library-managed copy of the file.
    pub file_path: PathBuf,
    pub fingerprint: String,
    pub added_at: i64,
}

#[derive(Debug, Clone)]
pub struct StoredPosition {
    pub locator: LayeredLocator,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    Bookmark,
    Highlight,
    Note,
}

impl AnnotationKind {
    fn as_str(&self) -> &'static str {
        match self {
            AnnotationKind::Bookmark => "bookmark",
            AnnotationKind::Highlight => "highlight",
            AnnotationKind::Note => "note",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "highlight" => AnnotationKind::Highlight,
            "note" => AnnotationKind::Note,
            _ => AnnotationKind::Bookmark,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Annotation {
    pub id: i64,
    pub kind: AnnotationKind,
    pub start: LayeredLocator,
    /// Range end for highlights; bookmarks are points.
    pub end: Option<LayeredLocator>,
    pub text: Option<String>,
    pub color: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct OpdsSource {
    pub id: i64,
    /// Opaque, possibly secret-bearing — never log or normalize.
    pub url: String,
    pub title: Option<String>,
    pub auth_user: Option<String>,
    pub auth_secret: Option<String>,
}

pub struct Library {
    conn: Connection,
    books_dir: PathBuf,
}

impl Library {
    /// Open (creating if needed) the library at `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        let books_dir = dir.join("books");
        std::fs::create_dir_all(&books_dir)?;
        let conn = db::open_and_migrate(&dir.join("chapbook.db"))?;
        Ok(Library { conn, books_dir })
    }

    /// The default per-user library location (`$CHAPBOOK_LIBRARY_DIR`,
    /// else XDG data dir).
    pub fn default_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("CHAPBOOK_LIBRARY_DIR") {
            return PathBuf::from(dir);
        }
        let base = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(".local/share")
            });
        base.join("chapbook")
    }

    /// Import a book: copy the file into the library, index its metadata.
    /// Re-importing a file with a known fingerprint returns the existing
    /// book instead of duplicating it.
    pub fn import(&mut self, source: &Path, metadata: &BookMetadata) -> Result<BookId> {
        let bytes = std::fs::read(source)?;
        let fingerprint = hex(&Sha1::digest(&bytes));

        if let Some(existing) = self
            .conn
            .query_row(
                "SELECT id FROM books WHERE fingerprint = ?1 AND deleted = 0",
                params![fingerprint],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(db_err)?
        {
            return Ok(BookId(existing));
        }

        let title = metadata.title.clone().unwrap_or_else(|| {
            source
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let tx = self.conn.transaction().map_err(db_err)?;
        tx.execute(
            "INSERT INTO books (title, language, identifier, file_path, source_path, fingerprint, added_at)
             VALUES (?1, ?2, ?3, '', ?4, ?5, strftime('%s','now'))",
            params![
                title,
                metadata.language,
                metadata.identifier,
                source.to_string_lossy(),
                fingerprint
            ],
        )
        .map_err(db_err)?;
        let id = tx.last_insert_rowid();

        for (i, author) in metadata.authors.iter().enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO authors (name) VALUES (?1)",
                params![author],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO book_authors (book_id, author_id, position)
                 SELECT ?1, id, ?2 FROM authors WHERE name = ?3",
                params![id, i as i64, author],
            )
            .map_err(db_err)?;
        }

        // Managed copy named by id; extension preserved for format sniffing.
        let extension = source
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "epub".into());
        let file_path = self.books_dir.join(format!("{id}.{extension}"));
        std::fs::write(&file_path, &bytes)?;
        tx.execute(
            "UPDATE books SET file_path = ?1 WHERE id = ?2",
            params![file_path.to_string_lossy(), id],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(BookId(id))
    }

    /// All (non-deleted) books, optionally filtered by a substring match on
    /// title or author, newest first.
    pub fn books(&self, filter: Option<&str>) -> Result<Vec<BookRecord>> {
        let pattern = filter.map(|f| format!("%{f}%"));
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT b.id, b.title, b.language, b.identifier, b.file_path,
                        b.fingerprint, b.added_at
                 FROM books b
                 LEFT JOIN book_authors ba ON ba.book_id = b.id
                 LEFT JOIN authors a ON a.id = ba.author_id
                 WHERE b.deleted = 0
                   AND (?1 IS NULL OR b.title LIKE ?1 OR a.name LIKE ?1)
                 ORDER BY b.added_at DESC, b.id DESC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![pattern], |row| {
                Ok(BookRecord {
                    id: BookId(row.get(0)?),
                    title: row.get(1)?,
                    authors: Vec::new(),
                    language: row.get(2)?,
                    identifier: row.get(3)?,
                    file_path: PathBuf::from(row.get::<_, String>(4)?),
                    fingerprint: row.get(5)?,
                    added_at: row.get(6)?,
                })
            })
            .map_err(db_err)?;
        let mut books: Vec<BookRecord> = rows.collect::<rusqlite::Result<_>>().map_err(db_err)?;
        for book in &mut books {
            book.authors = self.authors_of(book.id)?;
        }
        Ok(books)
    }

    fn authors_of(&self, id: BookId) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.name FROM authors a
                 JOIN book_authors ba ON ba.author_id = a.id
                 WHERE ba.book_id = ?1 ORDER BY ba.position",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![id.0], |row| row.get::<_, String>(0))
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_err)
    }

    /// Find a book by its publication identifier — the cross-edition match:
    /// a re-downloaded edition has a new fingerprint but usually the same
    /// dc:identifier.
    pub fn find_by_identifier(&self, identifier: &str) -> Result<Option<BookId>> {
        self.conn
            .query_row(
                "SELECT id FROM books WHERE identifier = ?1 AND deleted = 0
                 ORDER BY added_at DESC LIMIT 1",
                params![identifier],
                |row| row.get::<_, i64>(0).map(BookId),
            )
            .optional()
            .map_err(db_err)
    }

    /// Adopt a new edition of an existing book: replace the managed copy and
    /// fingerprint. Positions/annotations stay keyed to the book id and get
    /// re-anchored by [`restore_position`], never orphaned.
    pub fn update_edition(&mut self, id: BookId, source: &Path) -> Result<()> {
        let bytes = std::fs::read(source)?;
        let fingerprint = hex(&Sha1::digest(&bytes));
        let file_path: String = self
            .conn
            .query_row(
                "SELECT file_path FROM books WHERE id = ?1",
                params![id.0],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        std::fs::write(&file_path, &bytes)?;
        self.conn
            .execute(
                "UPDATE books SET fingerprint = ?1, source_path = ?2 WHERE id = ?3",
                params![fingerprint, source.to_string_lossy(), id.0],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// One book record by id.
    pub fn book(&self, id: BookId) -> Result<Option<BookRecord>> {
        let record = self
            .conn
            .query_row(
                "SELECT id, title, language, identifier, file_path, fingerprint, added_at
                 FROM books WHERE id = ?1 AND deleted = 0",
                params![id.0],
                |row| {
                    Ok(BookRecord {
                        id: BookId(row.get(0)?),
                        title: row.get(1)?,
                        authors: Vec::new(),
                        language: row.get(2)?,
                        identifier: row.get(3)?,
                        file_path: PathBuf::from(row.get::<_, String>(4)?),
                        fingerprint: row.get(5)?,
                        added_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)?;
        match record {
            Some(mut record) => {
                record.authors = self.authors_of(record.id)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Find a book by its edition fingerprint (SHA-1 hex of the file bytes).
    pub fn find_by_fingerprint(&self, fingerprint: &str) -> Result<Option<BookId>> {
        self.conn
            .query_row(
                "SELECT id FROM books WHERE fingerprint = ?1 AND deleted = 0",
                params![fingerprint],
                |row| row.get::<_, i64>(0).map(BookId),
            )
            .optional()
            .map_err(db_err)
    }

    pub fn fingerprint_of_file(path: &Path) -> Result<String> {
        Ok(hex(&Sha1::digest(std::fs::read(path)?)))
    }

    pub fn position(&self, id: BookId) -> Result<Option<StoredPosition>> {
        self.conn
            .query_row(
                "SELECT spine_href, spine_index, char_offset, locator_version,
                        quote_prefix, quote_exact, quote_suffix,
                        spine_fraction, book_progression, updated_at
                 FROM positions WHERE book_id = ?1",
                params![id.0],
                |row| {
                    Ok(StoredPosition {
                        locator: LayeredLocator {
                            spine_href: row.get(0)?,
                            spine_index: row.get::<_, i64>(1)? as usize,
                            char_offset: row.get::<_, i64>(2)? as u32,
                            locator_version: row.get::<_, i64>(3)? as u32,
                            quote: Quote {
                                prefix: row.get(4)?,
                                exact: row.get(5)?,
                                suffix: row.get(6)?,
                            },
                            spine_fraction: row.get(7)?,
                            book_progression: row.get(8)?,
                        },
                        updated_at: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)
    }

    pub fn set_position(&mut self, id: BookId, locator: &LayeredLocator) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO positions (book_id, spine_href, spine_index, char_offset,
                        locator_version, quote_prefix, quote_exact, quote_suffix,
                        spine_fraction, book_progression, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10, strftime('%s','now'))
                 ON CONFLICT(book_id) DO UPDATE SET
                        spine_href=?2, spine_index=?3, char_offset=?4,
                        locator_version=?5, quote_prefix=?6, quote_exact=?7,
                        quote_suffix=?8, spine_fraction=?9, book_progression=?10,
                        updated_at=strftime('%s','now')",
                params![
                    id.0,
                    locator.spine_href,
                    locator.spine_index as i64,
                    locator.char_offset as i64,
                    locator.locator_version as i64,
                    locator.quote.prefix,
                    locator.quote.exact,
                    locator.quote.suffix,
                    locator.spine_fraction,
                    locator.book_progression,
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn add_annotation(
        &mut self,
        id: BookId,
        kind: AnnotationKind,
        start: &LayeredLocator,
        end: Option<&LayeredLocator>,
        text: Option<&str>,
        color: Option<&str>,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO annotations (book_id, kind,
                    start_spine_href, start_spine_index, start_char_offset,
                    start_locator_version, start_quote_prefix, start_quote_exact,
                    start_quote_suffix, start_spine_fraction, start_book_progression,
                    end_spine_href, end_spine_index, end_char_offset,
                    end_locator_version, end_quote_prefix, end_quote_exact,
                    end_quote_suffix, end_spine_fraction, end_book_progression,
                    note_text, color, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,
                         ?12,?13,?14,?15,?16,?17,?18,?19,?20,
                         ?21,?22, strftime('%s','now'), strftime('%s','now'))",
                params![
                    id.0,
                    kind.as_str(),
                    start.spine_href,
                    start.spine_index as i64,
                    start.char_offset as i64,
                    start.locator_version as i64,
                    start.quote.prefix,
                    start.quote.exact,
                    start.quote.suffix,
                    start.spine_fraction,
                    start.book_progression,
                    end.map(|e| e.spine_href.clone()),
                    end.map(|e| e.spine_index as i64),
                    end.map(|e| e.char_offset as i64),
                    end.map(|e| e.locator_version as i64),
                    end.map(|e| e.quote.prefix.clone()),
                    end.map(|e| e.quote.exact.clone()),
                    end.map(|e| e.quote.suffix.clone()),
                    end.map(|e| e.spine_fraction),
                    end.map(|e| e.book_progression),
                    text,
                    color,
                ],
            )
            .map_err(db_err)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn annotations(&self, id: BookId) -> Result<Vec<Annotation>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind,
                        start_spine_href, start_spine_index, start_char_offset,
                        start_locator_version, start_quote_prefix, start_quote_exact,
                        start_quote_suffix, start_spine_fraction, start_book_progression,
                        end_spine_href, end_spine_index, end_char_offset,
                        end_locator_version, end_quote_prefix, end_quote_exact,
                        end_quote_suffix, end_spine_fraction, end_book_progression,
                        note_text, color, created_at, updated_at
                 FROM annotations
                 WHERE book_id = ?1 AND deleted = 0
                 ORDER BY start_book_progression, id",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![id.0], |row| {
                let start = LayeredLocator {
                    spine_href: row.get(2)?,
                    spine_index: row.get::<_, i64>(3)? as usize,
                    char_offset: row.get::<_, i64>(4)? as u32,
                    locator_version: row.get::<_, i64>(5)? as u32,
                    quote: Quote {
                        prefix: row.get(6)?,
                        exact: row.get(7)?,
                        suffix: row.get(8)?,
                    },
                    spine_fraction: row.get(9)?,
                    book_progression: row.get(10)?,
                };
                let end = match row.get::<_, Option<String>>(11)? {
                    Some(href) => Some(LayeredLocator {
                        spine_href: href,
                        spine_index: row.get::<_, i64>(12)? as usize,
                        char_offset: row.get::<_, i64>(13)? as u32,
                        locator_version: row.get::<_, i64>(14)? as u32,
                        quote: Quote {
                            prefix: row.get(15)?,
                            exact: row.get(16)?,
                            suffix: row.get(17)?,
                        },
                        spine_fraction: row.get(18)?,
                        book_progression: row.get(19)?,
                    }),
                    None => None,
                };
                Ok(Annotation {
                    id: row.get(0)?,
                    kind: AnnotationKind::parse(&row.get::<_, String>(1)?),
                    start,
                    end,
                    text: row.get(20)?,
                    color: row.get(21)?,
                    created_at: row.get(22)?,
                    updated_at: row.get(23)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_err)
    }

    /// Soft-delete an annotation (kept for future sync; never hard-deleted).
    /// Recolor an annotation. `None` hands it back to the reader's theme
    /// color.
    pub fn set_annotation_color(&mut self, annotation_id: i64, color: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE annotations SET color = ?2, updated_at = strftime('%s','now')
                 WHERE id = ?1",
                params![annotation_id, color],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn delete_annotation(&mut self, annotation_id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE annotations SET deleted = 1, updated_at = strftime('%s','now')
                 WHERE id = ?1",
                params![annotation_id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    // ---- Reading settings ----

    /// Settings for a scope: `None` is the reader's default, `Some(id)` is
    /// that book's override. `Ok(None)` when the scope has never been set.
    pub fn reading_settings(&self, book: Option<BookId>) -> Result<Option<ReadingSettings>> {
        let scope = book.map_or(0, |b| b.0);
        self.conn
            .query_row(
                "SELECT base_font_px, line_height, justify, publisher_styles, theme
                 FROM reading_settings WHERE book_id = ?1",
                params![scope],
                |row| {
                    Ok(ReadingSettings {
                        base_font_px: row.get::<_, f64>(0)? as f32,
                        line_height: row.get::<_, f64>(1)? as f32,
                        justify: row.get::<_, i64>(2)? != 0,
                        publisher_styles: row.get::<_, i64>(3)? != 0,
                        theme: row
                            .get::<_, String>(4)
                            .map(|name| Theme::from_name(&name).unwrap_or_default())?,
                    })
                },
            )
            .optional()
            .map_err(db_err)
    }

    /// The settings a book should open with: its own override, else the
    /// reader's default, else the built-in defaults. Never fails — a
    /// database that can't answer falls back rather than blocking reading.
    pub fn effective_settings(&self, book: Option<BookId>) -> ReadingSettings {
        book.and_then(|id| self.reading_settings(Some(id)).ok().flatten())
            .or_else(|| self.reading_settings(None).ok().flatten())
            .unwrap_or_default()
    }

    pub fn set_reading_settings(
        &mut self,
        book: Option<BookId>,
        settings: &ReadingSettings,
    ) -> Result<()> {
        let scope = book.map_or(0, |b| b.0);
        self.conn
            .execute(
                "INSERT INTO reading_settings
                    (book_id, base_font_px, line_height, justify, publisher_styles,
                     theme, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6, strftime('%s','now'))
                 ON CONFLICT(book_id) DO UPDATE SET
                    base_font_px = excluded.base_font_px,
                    line_height = excluded.line_height,
                    justify = excluded.justify,
                    publisher_styles = excluded.publisher_styles,
                    theme = excluded.theme,
                    updated_at = excluded.updated_at",
                params![
                    scope,
                    settings.base_font_px as f64,
                    settings.line_height as f64,
                    settings.justify as i64,
                    settings.publisher_styles as i64,
                    settings.theme.name(),
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Drop a book's override so it follows the reader's default again.
    pub fn clear_reading_settings(&mut self, book: BookId) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM reading_settings WHERE book_id = ?1",
                params![book.0],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn add_opds_source(
        &mut self,
        url: &str,
        title: Option<&str>,
        auth_user: Option<&str>,
        auth_secret: Option<&str>,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO opds_sources (url, title, auth_user, auth_secret, added_at)
                 VALUES (?1, ?2, ?3, ?4, strftime('%s','now'))",
                params![url, title, auth_user, auth_secret],
            )
            .map_err(db_err)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn opds_sources(&self) -> Result<Vec<OpdsSource>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, url, title, auth_user, auth_secret
                 FROM opds_sources WHERE deleted = 0 ORDER BY id",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(OpdsSource {
                    id: row.get(0)?,
                    url: row.get(1)?,
                    title: row.get(2)?,
                    auth_user: row.get(3)?,
                    auth_secret: row.get(4)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_err)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
