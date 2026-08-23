//! OPDS catalog client: browse, search, paginate, and download from ebook
//! catalogs (self-hosted catalog and comic servers, library-lending stacks).
//!
//! The binding contract is `docs/OPDS-INTEROP.md` — observed wild-server
//! behavior, exercised by the fixture corpus in `fixtures/opds/`. The load-
//! bearing decisions, so nobody re-litigates them at implementation time:
//!
//! - **OPDS 1.2 Atom is the canonical dialect**; 2.0 JSON (serde) is a
//!   secondary parser kept honest by fixtures + the vendored schemas. The
//!   two encodings are not informationally equivalent — never assume a
//!   field survives a version switch.
//! - **Feeds are parsed at the XML level with namespace-aware `quick-xml`**,
//!   NOT `atom_syndication`/`feed-rs`: both silently drop the foreign-
//!   namespace `<link>` attributes (`opds:facetGroup`, `opds:activeFacet`,
//!   `thr:count`, `pse:count`, `pse:lastRead`…) that facets and page
//!   streaming live in.
//! - Every href resolves against the request URL; hrefs are never
//!   strict-URI-parsed (PSE templates contain literal `{pageNumber}`
//!   braces); received feeds are never schema-validated as a gate; media
//!   types compare by parsed essence + parameters, not string equality.
//! - One media type per `Accept` header, no q-values (wild servers match by
//!   substring). Pagination back-rel is `previous`; totals via OpenSearch
//!   elements (1.2) or `numberOfItems`/`currentPage` (2.0). Both RFC 3339
//!   and date-only date shapes parse everywhere.
//! - Auth: HTTP Basic on 401 at *any* point in a flow, plus the OPDS
//!   Authentication Document (`application/opds-authentication+json`) for a
//!   native login dialog. Catalog URLs may embed per-user API keys: opaque,
//!   never logged or normalized.
//! - HTTP is a blocking `ureq` agent (rustls); no conditional requests and
//!   no Range resume to count on — downloads go to a temp file and rename
//!   atomically; redirects (incl. cross-host) are followed.
//! - OPDS-PSE page streaming (`{pageNumber}` 0-based, `pse:lastRead`
//!   1-based, lazy stream links behind the complete-entry `alternate`) feeds
//!   the comic milestone's remote `Publication` (see
//!   `Publication::unit_bytes`'s blocking-fetch contract).
//!
//! Implemented in milestone M6.

mod atom;
mod client;
mod href;
mod model;
mod opds2;

pub use atom::parse_atom;
pub use client::{opensearch_template, pse_page_url, OpdsClient};
pub use href::resolve_url;
pub use model::{
    AuthDocument, AuthFlow, AuthLink, Entry, Feed, Group, Link, MediaType, OpdsVersion, Price,
    Series, Totals, AUTH_BASIC,
};
pub use opds2::{parse_opds2, parse_opds2_publication};

/// Client/parse errors. `AuthRequired` carries the server's Authentication
/// Document when it sent one — enough to render a native login dialog.
#[derive(Debug, thiserror::Error)]
pub enum OpdsError {
    #[error("authentication required")]
    AuthRequired(Option<Box<AuthDocument>>),
    #[error("HTTP status {0}")]
    Http(u16),
    #[error("network error: {0}")]
    Network(String),
    #[error("parse error: {0}")]
    Parse(String),
}

impl From<OpdsError> for chapbook_core::ChapbookError {
    fn from(e: OpdsError) -> Self {
        match e {
            OpdsError::Network(msg) => chapbook_core::ChapbookError::Network(msg),
            other => chapbook_core::ChapbookError::Opds(other.to_string()),
        }
    }
}
