//! Browsing: the questions asked of a library that has outgrown one
//! screen.
//!
//! [`Library::books`] and [`Library::recent`] answered two of them with a
//! hardcoded query each, which was the right size for a shelf of a dozen.
//! Past that a reader wants to narrow — this collection, this series, the
//! ones I have not finished — and to choose the order, and the two fixed
//! queries had nowhere to put any of it. [`BookQuery`] is that one place;
//! `books` and `recent` survive as the two shapes worth naming.
//!
//! Search runs through the `book_search` FTS5 index rather than `LIKE`,
//! for the reason the v7 migration gives: `LIKE` folds case for ASCII
//! only, so a shelf holding Charlotte Brontë answered nothing to
//! "bronte".

use rusqlite::{params_from_iter, types::Value, OptionalExtension, Row};

use chapbook_core::Result;

use crate::db::db_err;
use crate::{BookId, BookRecord, Library};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollectionId(pub i64);

/// A collection as a list of collections shows them.
///
/// Carries the count because a list of shelf names that does not say how
/// many books are on each is a list of words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub id: CollectionId,
    pub name: String,
    pub added_at: i64,
    /// Books in it, not counting removed ones.
    pub books: usize,
}

/// A collection as a book names them: the handle and the label, which is
/// all a shelf needs to draw a chip and make it clickable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRef {
    pub id: CollectionId,
    pub name: String,
}

/// How far through a book the reader is, as a shelf groups it.
///
/// Derived rather than stored, except for the one part that has to be
/// stored: see the v7 migration on why `finished` is a column and not a
/// progress threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadingState {
    /// Never opened.
    Unread,
    /// Opened, not finished — the state a "continue reading" row wants.
    Reading,
    /// Reached the end at least once, whatever the position says now.
    Finished,
}

/// How [`Library::query`] orders its results.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Newest addition first: a stable listing.
    #[default]
    Added,
    /// The most recent thing that happened to this book, read or added.
    /// `COALESCE` is what makes an unopened book sort by when it arrived
    /// instead of falling to the bottom forever.
    ///
    /// The second term is not decoration. Every timestamp in this schema
    /// comes from `strftime('%s','now')`, which is whole seconds, so
    /// adding a book and opening it — or opening two books quickly — ties.
    /// On a tie the *read* is the more meaningful event, and without
    /// saying so the tiebreak falls to `id` and puts the book you were
    /// reading behind one you have never opened. Found the first time
    /// anything asked the library for books in reading order.
    Read,
    /// Alphabetical by title, case-folded.
    Title,
    /// By first author, then title. A book with no author sorts last
    /// rather than first: an empty name is not a name beginning with a
    /// space.
    Author,
    /// By series, then position within it, then title. Books in no series
    /// come last — they are not a series called nothing — and a book whose
    /// series names no position comes last within its own series.
    Series,
}

impl Sort {
    fn sql(self) -> &'static str {
        match self {
            Sort::Added => "b.added_at DESC, b.id DESC",
            Sort::Read => {
                "COALESCE(p.updated_at, b.added_at) DESC, (p.updated_at IS NOT NULL) DESC, b.id DESC"
            }
            Sort::Title => "b.title COLLATE NOCASE, b.id",
            Sort::Author => "first_author IS NULL, first_author COLLATE NOCASE, \
                             b.title COLLATE NOCASE, b.id",
            Sort::Series => "b.series IS NULL, b.series COLLATE NOCASE, \
                             b.series_index IS NULL, b.series_index, \
                             b.title COLLATE NOCASE, b.id",
        }
    }
}

/// What to list, and in what order.
///
/// Every field narrows independently and they compose: "unfinished books
/// in Sci-Fi, by series" is one query. `Default` is the whole shelf,
/// newest first.
#[derive(Debug, Clone, Default)]
pub struct BookQuery<'a> {
    /// Free text over title, authors and series. Matched by whole word
    /// with a prefix — "tolk mid" finds *The Hobbit* by J. R. R. Tolkien —
    /// and folded for case and diacritics, so "bronte" finds Brontë.
    pub search: Option<&'a str>,
    /// Only books in this collection.
    pub collection: Option<CollectionId>,
    /// Only books in this series, matched exactly (case-folded): the
    /// value comes from [`Library::series`] or a record, not from typing.
    pub series: Option<&'a str>,
    pub state: Option<ReadingState>,
    pub sort: Sort,
    /// How many to return. `None` is all of them.
    pub limit: Option<usize>,
    /// How many to skip — the second half of paging a long shelf.
    pub offset: usize,
}

/// The columns every [`BookRecord`] is built from, in the order
/// [`record_from_row`] reads them. One list so a column added to the
/// record cannot be added to one query and forgotten in the other.
pub(crate) const RECORD_COLUMNS: &str = "b.id, b.title, b.language, b.identifier, b.file_path,
     b.fingerprint, b.added_at, b.cover_path, b.series, b.series_index,
     b.finished_at, p.updated_at, p.book_progression";

pub(crate) fn record_from_row(row: &Row<'_>) -> rusqlite::Result<BookRecord> {
    Ok(BookRecord {
        id: BookId(row.get(0)?),
        title: row.get(1)?,
        authors: Vec::new(),
        language: row.get(2)?,
        identifier: row.get(3)?,
        file_path: std::path::PathBuf::from(row.get::<_, String>(4)?),
        fingerprint: row.get(5)?,
        added_at: row.get(6)?,
        cover_path: row
            .get::<_, Option<String>>(7)?
            .map(std::path::PathBuf::from),
        series: row.get(8)?,
        series_index: row.get(9)?,
        finished_at: row.get(10)?,
        last_read: row.get(11)?,
        progress: row.get(12)?,
        collections: Vec::new(),
    })
}

/// Turn what a reader typed into an FTS5 `MATCH` expression.
///
/// Two jobs, and the second is the load-bearing one. It makes the search
/// prefix-matched and conjunctive, which is what a search box is expected
/// to do. And it makes arbitrary typing *safe*: FTS5's query language
/// gives `-`, `*`, `:`, `^`, `(`, `"` and the bare words `AND`/`OR`/`NOT`
/// meanings of their own, so passing a reader's text through raw turns a
/// search for `Nineteen Eighty-Four` into a search for everything matching
/// "Nineteen" but not "Four", and a search for `"` into a syntax error
/// surfaced as a failed query. Quoting every token disarms all of it —
/// inside a string, only `"` is special, and it is doubled.
///
/// `None` means the text held nothing searchable (punctuation alone),
/// which matches no book rather than every book: a reader who typed
/// something and got the unfiltered shelf back would think the filter was
/// ignored, and it was.
fn match_expression(text: &str) -> Option<String> {
    let tokens: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        // The tokenizer folds case and diacritics on both sides, so the
        // only escaping left is FTS5's own doubled quote.
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect();
    (!tokens.is_empty()).then(|| tokens.join(" AND "))
}

impl Library {
    /// The shelf, narrowed and ordered.
    pub fn query(&self, query: &BookQuery<'_>) -> Result<Vec<BookRecord>> {
        let mut wheres = vec!["b.deleted = 0".to_string()];
        let mut binds: Vec<Value> = Vec::new();

        if let Some(text) = query.search.map(str::trim).filter(|t| !t.is_empty()) {
            let Some(expression) = match_expression(text) else {
                return Ok(Vec::new());
            };
            binds.push(Value::Text(expression));
            wheres.push(format!(
                "b.id IN (SELECT rowid FROM book_search WHERE book_search MATCH ?{})",
                binds.len()
            ));
        }
        if let Some(collection) = query.collection {
            binds.push(Value::Integer(collection.0));
            wheres.push(format!(
                "b.id IN (SELECT book_id FROM book_collections WHERE collection_id = ?{})",
                binds.len()
            ));
        }
        if let Some(series) = query.series {
            binds.push(Value::Text(series.to_string()));
            wheres.push(format!("b.series = ?{} COLLATE NOCASE", binds.len()));
        }
        match query.state {
            // "Opened but not finished" is the state, so a finished book
            // reopened is not quietly counted as still being read.
            Some(ReadingState::Reading) => {
                wheres.push("b.finished_at IS NULL AND p.updated_at IS NOT NULL".into());
            }
            Some(ReadingState::Unread) => {
                wheres.push("b.finished_at IS NULL AND p.updated_at IS NULL".into());
            }
            Some(ReadingState::Finished) => wheres.push("b.finished_at IS NOT NULL".into()),
            None => {}
        }

        binds.push(Value::Integer(query.limit.map_or(-1, |n| n as i64)));
        let limit = binds.len();
        binds.push(Value::Integer(query.offset as i64));
        let offset = binds.len();

        // One statement for the whole record. Position used to be a
        // second query per book, which a shelf turns from a nicety into
        // an N+1. The author subselect is here rather than as a join
        // because a join to a many-to-many needs DISTINCT, and DISTINCT
        // over a row that includes `first_author` would not dedupe.
        let sql = format!(
            "SELECT {RECORD_COLUMNS},
                    (SELECT a.name FROM authors a
                      JOIN book_authors ba ON ba.author_id = a.id
                     WHERE ba.book_id = b.id
                     ORDER BY ba.position LIMIT 1) AS first_author
             FROM books b
             LEFT JOIN positions p ON p.book_id = b.id
             WHERE {}
             ORDER BY {}
             LIMIT ?{limit} OFFSET ?{offset}",
            wheres.join(" AND "),
            query.sort.sql(),
        );

        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(binds), record_from_row)
            .map_err(db_err)?;
        let mut books: Vec<BookRecord> = rows.collect::<rusqlite::Result<_>>().map_err(db_err)?;
        self.attach_authors(&mut books)?;
        self.attach_collections(&mut books)?;
        Ok(books)
    }

    /// The series the shelf holds, with how many books are in each,
    /// alphabetically — a browse-by-series screen in one query.
    pub fn series(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT b.series, count(*) FROM books b
                  WHERE b.deleted = 0 AND b.series IS NOT NULL AND b.series <> ''
                  GROUP BY b.series COLLATE NOCASE
                  ORDER BY b.series COLLATE NOCASE",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_err)
    }

    // ---- Collections ----

    /// Make a collection, or return the one that already has this name.
    ///
    /// Idempotent on the name because that is what every caller wants: a
    /// shell adding a book to "Sci-Fi" should not have to ask first, and
    /// an import that recreates a reader's shelves should not fail on the
    /// second run.
    pub fn create_collection(&mut self, name: &str) -> Result<CollectionId> {
        let name = name.trim();
        if name.is_empty() {
            return Err(chapbook_core::ChapbookError::Library(
                "a collection needs a name".into(),
            ));
        }
        if let Some(existing) = self.collection_named(name)? {
            return Ok(existing);
        }
        self.conn
            .execute(
                "INSERT INTO collections (name, added_at, updated_at)
                 VALUES (?1, strftime('%s','now'), strftime('%s','now'))",
                params_from_iter([Value::Text(name.to_string())]),
            )
            .map_err(db_err)?;
        Ok(CollectionId(self.conn.last_insert_rowid()))
    }

    fn collection_named(&self, name: &str) -> Result<Option<CollectionId>> {
        self.conn
            .query_row(
                "SELECT id FROM collections
                  WHERE name = ?1 COLLATE NOCASE AND deleted = 0",
                params_from_iter([Value::Text(name.to_string())]),
                |row| row.get::<_, i64>(0).map(CollectionId),
            )
            .optional()
            .map_err(db_err)
    }

    pub fn rename_collection(&mut self, id: CollectionId, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(chapbook_core::ChapbookError::Library(
                "a collection needs a name".into(),
            ));
        }
        self.conn
            .execute(
                "UPDATE collections SET name = ?1, updated_at = strftime('%s','now')
                  WHERE id = ?2 AND deleted = 0",
                params_from_iter([Value::Text(name.to_string()), Value::Integer(id.0)]),
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Remove a collection. The books stay; only the grouping goes.
    ///
    /// Soft, like every other delete here. The membership rows are left
    /// alone rather than cleaned up: every read of them joins through
    /// `collections` and so cannot see a deleted one, and keeping them is
    /// what would let an undo be an undo.
    pub fn delete_collection(&mut self, id: CollectionId) -> Result<()> {
        self.conn
            .execute(
                "UPDATE collections SET deleted = 1, updated_at = strftime('%s','now')
                  WHERE id = ?1",
                params_from_iter([Value::Integer(id.0)]),
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Every collection, with its size, oldest first.
    pub fn collections(&self) -> Result<Vec<Collection>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.name, c.added_at,
                        (SELECT count(*) FROM book_collections bc
                           JOIN books b ON b.id = bc.book_id
                          WHERE bc.collection_id = c.id AND b.deleted = 0)
                   FROM collections c
                  WHERE c.deleted = 0
                  ORDER BY c.added_at, c.id",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Collection {
                    id: CollectionId(row.get(0)?),
                    name: row.get(1)?,
                    added_at: row.get(2)?,
                    books: row.get::<_, i64>(3)? as usize,
                })
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_err)
    }

    /// The collections one book is in.
    pub fn collections_of(&self, book: BookId) -> Result<Vec<CollectionRef>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.name FROM collections c
                   JOIN book_collections bc ON bc.collection_id = c.id
                  WHERE bc.book_id = ?1 AND c.deleted = 0
                  ORDER BY c.name COLLATE NOCASE",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter([Value::Integer(book.0)]), |row| {
                Ok(CollectionRef {
                    id: CollectionId(row.get(0)?),
                    name: row.get(1)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(db_err)
    }

    /// Put a book in a collection. Doing it twice is not an error.
    pub fn add_to_collection(&mut self, book: BookId, collection: CollectionId) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO book_collections (book_id, collection_id, added_at)
                 VALUES (?1, ?2, strftime('%s','now'))",
                params_from_iter([Value::Integer(book.0), Value::Integer(collection.0)]),
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Take a book out of a collection. Hard, unlike the rest: a
    /// membership has no state of its own worth keeping, and re-adding
    /// writes the same row back.
    pub fn remove_from_collection(&mut self, book: BookId, collection: CollectionId) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM book_collections WHERE book_id = ?1 AND collection_id = ?2",
                params_from_iter([Value::Integer(book.0), Value::Integer(collection.0)]),
            )
            .map_err(db_err)?;
        Ok(())
    }

    // ---- Reading state ----

    /// Record that the reader reached the end of this book, or take it
    /// back.
    ///
    /// Marking a book finished twice keeps the first timestamp: "when did
    /// I finish this" should not move because the reader turned to the
    /// last page again.
    pub fn set_finished(&mut self, id: BookId, finished: bool) -> Result<()> {
        let sql = if finished {
            "UPDATE books SET finished_at = COALESCE(finished_at, strftime('%s','now'))
              WHERE id = ?1 AND deleted = 0"
        } else {
            "UPDATE books SET finished_at = NULL WHERE id = ?1 AND deleted = 0"
        };
        self.conn
            .execute(sql, params_from_iter([Value::Integer(id.0)]))
            .map_err(db_err)?;
        Ok(())
    }

    // ---- Batch attachment ----

    /// Fill in authors for a whole page of records in one query.
    ///
    /// The per-book version was fine for `lib ls` and is a hundred round
    /// trips for a shelf of a hundred books.
    pub(crate) fn attach_authors(&self, books: &mut [BookRecord]) -> Result<()> {
        let Some(ids) = id_list(books) else {
            return Ok(());
        };
        let sql = format!(
            "SELECT ba.book_id, a.name FROM authors a
             JOIN book_authors ba ON ba.author_id = a.id
             WHERE ba.book_id IN ({ids})
             ORDER BY ba.book_id, ba.position"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_err)?;
        let mut by_book: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        for row in rows {
            let (book_id, name) = row.map_err(db_err)?;
            by_book.entry(book_id).or_default().push(name);
        }
        for book in books {
            book.authors = by_book.remove(&book.id.0).unwrap_or_default();
        }
        Ok(())
    }

    /// The same, for collections: one query for the page rather than one
    /// per book.
    pub(crate) fn attach_collections(&self, books: &mut [BookRecord]) -> Result<()> {
        let Some(ids) = id_list(books) else {
            return Ok(());
        };
        let sql = format!(
            "SELECT bc.book_id, c.id, c.name FROM collections c
             JOIN book_collections bc ON bc.collection_id = c.id
             WHERE bc.book_id IN ({ids}) AND c.deleted = 0
             ORDER BY bc.book_id, c.name COLLATE NOCASE"
        );
        let mut stmt = self.conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    CollectionRef {
                        id: CollectionId(row.get(1)?),
                        name: row.get(2)?,
                    },
                ))
            })
            .map_err(db_err)?;
        let mut by_book: std::collections::HashMap<i64, Vec<CollectionRef>> =
            std::collections::HashMap::new();
        for row in rows {
            let (book_id, collection) = row.map_err(db_err)?;
            by_book.entry(book_id).or_default().push(collection);
        }
        for book in books {
            book.collections = by_book.remove(&book.id.0).unwrap_or_default();
        }
        Ok(())
    }
}

/// `1,2,3` for an `IN` clause, or `None` when there is nothing to ask
/// about. Interpolated rather than bound because the count varies per
/// call and these are row ids the database just handed us, not input.
fn id_list(books: &[BookRecord]) -> Option<String> {
    if books.is_empty() {
        return None;
    }
    Some(
        books
            .iter()
            .map(|b| b.id.0.to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

#[cfg(test)]
mod tests {
    use super::match_expression;

    #[test]
    fn every_token_is_quoted_and_prefixed() {
        assert_eq!(
            match_expression("tolk mid").as_deref(),
            Some("\"tolk\"* AND \"mid\"*")
        );
    }

    /// The characters that make FTS5's query language a query language.
    /// A reader typing a real title must not be composing an expression.
    #[test]
    fn fts_syntax_in_a_title_is_not_syntax() {
        assert_eq!(
            match_expression("Nineteen Eighty-Four").as_deref(),
            Some("\"Nineteen\"* AND \"Eighty\"* AND \"Four\"*"),
            "the hyphen must not become NOT"
        );
        assert_eq!(
            match_expression("cats OR dogs").as_deref(),
            Some("\"cats\"* AND \"OR\"* AND \"dogs\"*"),
            "a bare OR is a word here, not an operator"
        );
        for input in ["\"", "*", "^foo", "a:b", "(x)", "-"] {
            let expression = match_expression(input);
            assert!(
                expression
                    .as_deref()
                    .is_none_or(|e| !e.contains(input) || e.starts_with('"')),
                "{input:?} came back as {expression:?}"
            );
        }
    }

    #[test]
    fn punctuation_alone_is_not_a_search() {
        assert_eq!(match_expression("!!!"), None);
        assert_eq!(match_expression("  "), None);
    }
}
