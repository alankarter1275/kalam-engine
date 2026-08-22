//! OPDS catalog client: browse, search, paginate, and download from ebook
//! catalogs (Standard Ebooks, Project Gutenberg, Calibre servers, ...).
//!
//! OPDS 1.2 feeds are Atom, parsed with `atom_syndication` (which preserves
//! the `opds:` extension elements — prices, indirect acquisition, facets);
//! OPDS 2.0 is JSON via serde. HTTP is a blocking `ureq` agent with rustls —
//! an ereader does not need an async runtime. HTTP Basic auth on 401.
//!
//! Implemented in milestone M6.
