//! Chapbook's binding to [`opds_client`]: OPDS-PSE streamed comics as
//! [`Publication`](chapbook_core::Publication)s, plus the error conversion
//! into chapbook's own error type.
//!
//! The OPDS itself — the Atom and JSON parsers, the feed model, auth flows,
//! search, downloads — lives in `opds-client`, which knows nothing about
//! chapbook and is headed for a life of its own. Everything in this crate
//! is the part that would not travel: the `Publication` impl and the error
//! seam.
//!
//! `opds-client`'s whole surface is re-exported here, so a consumer inside
//! chapbook depends on this crate and gets both halves.
//!
//! ## Transport
//!
//! `opds-client` opens no sockets of its own; the caller injects an
//! [`HttpClient`]. Chapbook's shells are desktop processes, so they use
//! [`OpdsClient::with_ureq`]. A shell on a platform whose networking must
//! belong to the host — iOS `URLSession` for background transfer and system
//! trust, Android `WorkManager`, WASM `fetch` — passes its own instead, and
//! nothing here changes.

pub use opds_client::*;

#[cfg(feature = "progression")]
pub mod progression;
mod pse;

pub use pse::StreamedComic;

/// Convert an [`OpdsError`] into chapbook's error type.
///
/// A plain function rather than a `From` impl: with `OpdsError` now owned by
/// the standalone `opds-client` and `ChapbookError` by `chapbook-core`,
/// neither type is local to this crate, so the impl would be an orphan.
pub fn to_chapbook_error(e: OpdsError) -> chapbook_core::ChapbookError {
    match e {
        OpdsError::Network(msg) => chapbook_core::ChapbookError::Network(msg),
        other => chapbook_core::ChapbookError::Opds(other.to_string()),
    }
}
