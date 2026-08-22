//! Local bookshelf persistence: imported books and their metadata, reading
//! positions ([`chapbook_core::Locator`]), bookmarks/highlights/notes, and
//! saved OPDS sources. SQLite via rusqlite (bundled, WAL mode), with
//! `schema_version`-pragma migrations.
//!
//! Implemented in milestone M7.
