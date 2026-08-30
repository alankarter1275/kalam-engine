//! The catalog model both OPDS dialects parse into.
//!
//! Deliberately close to the wire: the two encodings are not
//! informationally equivalent (the crate's INTEROP.md §1), so fields carry
//! dialect-specific data as `Option`s rather than pretending to a common
//! denominator. Hrefs are resolved against the request URL at parse time
//! but never strict-URI-parsed (PSE templates contain literal braces).

/// Media type compared by parsed essence + parameters, never raw string
/// equality (wild servers emit parameters without spaces, in any order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaType {
    pub raw: String,
    /// Lowercased `type/subtype`.
    pub essence: String,
    /// Lowercased parameter name → value, in written order.
    pub params: Vec<(String, String)>,
}

impl MediaType {
    pub fn parse(raw: &str) -> Self {
        let mut parts = raw.split(';');
        let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let params = parts
            .filter_map(|p| {
                let (k, v) = p.split_once('=')?;
                Some((
                    k.trim().to_ascii_lowercase(),
                    v.trim().trim_matches('"').to_string(),
                ))
            })
            .collect();
        MediaType {
            raw: raw.to_string(),
            essence,
            params,
        }
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_atom(&self) -> bool {
        self.essence == "application/atom+xml"
    }

    pub fn is_opds_catalog(&self) -> bool {
        (self.is_atom() && self.param("profile") == Some("opds-catalog"))
            || self.essence == "application/opds+json"
    }

    /// An Atom complete-entry document (the lazy-PSE follow target).
    pub fn is_entry_document(&self) -> bool {
        self.is_atom() && self.param("type") == Some("entry")
    }

    pub fn is_epub(&self) -> bool {
        self.essence == "application/epub+zip"
    }

    pub fn is_comic_archive(&self) -> bool {
        matches!(
            self.essence.as_str(),
            "application/vnd.comicbook+zip" | "application/x-cbz" | "application/x-cbr"
        )
    }
}

/// A catalog link with every extension attribute the interop doc requires
/// preserved (the atom_syndication trap: these live in foreign-namespace
/// attributes that lossy parsers drop).
#[derive(Debug, Clone, Default)]
pub struct Link {
    /// Resolved against the request URL; may contain literal `{template}`
    /// tokens (PSE) — treat as an opaque string.
    pub href: String,
    /// One or more rels (2.0 allows arrays).
    pub rel: Vec<String>,
    pub media_type: Option<MediaType>,
    pub title: Option<String>,
    // Facets (1.2)
    pub facet_group: Option<String>,
    pub active_facet: bool,
    /// `thr:count` (1.2) / `properties.numberOfItems` (2.0).
    pub count: Option<u64>,
    // Page streaming (OPDS-PSE, 1.x only)
    pub pse_count: Option<u32>,
    /// 1-based last read page.
    pub pse_last_read: Option<u32>,
    pub pse_last_read_date: Option<String>,
    // Commercial/lending extensions
    pub price: Option<Price>,
    /// Indirect acquisition chain, outermost first (media type essences).
    pub indirect: Vec<String>,
    pub availability: Option<String>,
    pub holds_total: Option<u32>,
    pub copies_available: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    pub currency: String,
    pub value: String,
}

pub const REL_FACET: &str = "http://opds-spec.org/facet";
pub const REL_PSE_STREAM: &str = "http://vaemendis.net/opds-pse/stream";
pub const REL_ACQ_PREFIX: &str = "http://opds-spec.org/acquisition";
pub const REL_IMAGE: &str = "http://opds-spec.org/image";
pub const REL_THUMBNAIL: &str = "http://opds-spec.org/image/thumbnail";

impl Link {
    pub fn has_rel(&self, rel: &str) -> bool {
        self.rel.iter().any(|r| r == rel)
    }

    pub fn is_facet(&self) -> bool {
        self.has_rel(REL_FACET)
    }

    pub fn is_pse_stream(&self) -> bool {
        self.has_rel(REL_PSE_STREAM)
    }

    pub fn is_acquisition(&self) -> bool {
        self.rel
            .iter()
            .any(|r| r == REL_ACQ_PREFIX || r.starts_with("http://opds-spec.org/acquisition"))
    }

    pub fn is_open_access(&self) -> bool {
        self.has_rel("http://opds-spec.org/acquisition/open-access")
    }

    /// The complete-entry link to follow for lazy PSE / full metadata.
    pub fn is_complete_entry(&self) -> bool {
        self.has_rel("alternate")
            && self
                .media_type
                .as_ref()
                .is_some_and(MediaType::is_entry_document)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Entry {
    /// Opaque — comic-server ids contain slashes and dots; never normalize.
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub language: Option<String>,
    /// `dcterms:issued` (may be date-only) / 2.0 `published` (RFC 3339).
    pub published: Option<String>,
    pub publisher: Option<String>,
    pub identifier: Option<String>,
    /// Plain-text summary.
    pub summary: Option<String>,
    /// HTML content/description (raw HTML; sanitize before display).
    pub content_html: Option<String>,
    /// Series membership (2.0 `belongsTo.series` only).
    pub series: Option<Series>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub position: Option<f64>,
}

impl Entry {
    pub fn cover(&self) -> Option<&Link> {
        self.links.iter().find(|l| l.has_rel(REL_IMAGE))
    }

    pub fn thumbnail(&self) -> Option<&Link> {
        self.links.iter().find(|l| l.has_rel(REL_THUMBNAIL))
    }

    pub fn acquisitions(&self) -> impl Iterator<Item = &Link> {
        self.links.iter().filter(|l| l.is_acquisition())
    }

    pub fn pse_stream(&self) -> Option<&Link> {
        self.links.iter().find(|l| l.is_pse_stream())
    }

    pub fn complete_entry(&self) -> Option<&Link> {
        self.links.iter().find(|l| l.is_complete_entry())
    }

    /// Navigation target for nav-feed entries (an entry whose only useful
    /// link is a catalog feed).
    pub fn navigation(&self) -> Option<&Link> {
        self.links.iter().find(|l| {
            l.media_type
                .as_ref()
                .is_some_and(|t| t.is_opds_catalog() && !t.is_entry_document())
                && !l.is_facet()
        })
    }
}

/// 2.0 `groups[]`; 1.2 approximates groups with entries carrying
/// `rel="collection"` links (kept on the entries, not normalized).
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub title: String,
    pub links: Vec<Link>,
    pub entries: Vec<Entry>,
}

/// Feed totals: OpenSearch elements in 1.2; metadata counters in 2.0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub total_results: Option<u64>,
    pub items_per_page: Option<u64>,
    /// 1-based start index (1.2 only).
    pub start_index: Option<u64>,
    /// Current page number (2.0 only).
    pub current_page: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpdsVersion {
    /// OPDS 1.x Atom — the canonical dialect.
    V1,
    /// OPDS 2.0 JSON.
    V2,
}

#[derive(Debug, Clone)]
pub struct Feed {
    pub version: OpdsVersion,
    pub title: String,
    pub id: Option<String>,
    pub links: Vec<Link>,
    pub entries: Vec<Entry>,
    pub groups: Vec<Group>,
    pub totals: Totals,
}

impl Feed {
    fn feed_link(&self, rel: &str) -> Option<&Link> {
        self.links.iter().find(|l| l.has_rel(rel))
    }

    pub fn next(&self) -> Option<&Link> {
        self.feed_link("next")
    }

    /// The back-rel is `previous`, not `prev` (interop doc §2); accept both.
    pub fn previous(&self) -> Option<&Link> {
        self.feed_link("previous")
            .or_else(|| self.feed_link("prev"))
    }

    pub fn search(&self) -> Option<&Link> {
        self.feed_link("search")
    }

    /// Facet links grouped by `opds:facetGroup`, in feed order.
    pub fn facet_groups(&self) -> Vec<(String, Vec<&Link>)> {
        let mut groups: Vec<(String, Vec<&Link>)> = Vec::new();
        for link in self.links.iter().filter(|l| l.is_facet()) {
            let name = link.facet_group.clone().unwrap_or_default();
            match groups.iter_mut().find(|(n, _)| *n == name) {
                Some((_, links)) => links.push(link),
                None => groups.push((name, vec![link])),
            }
        }
        groups
    }
}

/// OPDS Authentication Document (`application/opds-authentication+json`),
/// returned by well-behaved servers alongside 401 — render a native login
/// dialog from it instead of a raw failure.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AuthDocument {
    pub id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub authentication: Vec<AuthFlow>,
    #[serde(default)]
    pub links: Vec<AuthLink>,
}

pub const AUTH_BASIC: &str = "http://opds-spec.org/auth/basic";

impl AuthDocument {
    /// The HTTP Basic flow, when offered (the one chapbook supports;
    /// recognize-and-decline others gracefully).
    pub fn basic_flow(&self) -> Option<&AuthFlow> {
        self.authentication.iter().find(|f| f.kind == AUTH_BASIC)
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AuthFlow {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AuthLink {
    pub rel: Option<String>,
    pub href: String,
    #[serde(rename = "type")]
    pub media_type: Option<String>,
}
