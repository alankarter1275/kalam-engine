//! OPDS 2.0 (JSON) parsing — the secondary dialect, kept honest by the
//! fixture corpus. Handles the spec'd polymorphism: contributor and subject
//! are string-or-object, `rel` is string-or-array, `description` may hold
//! raw HTML, and images carry no rel (first image = cover).

use serde::Deserialize;

use crate::href::resolve_url;
use crate::model::{
    Entry, Feed, Group, Link, MediaType, OpdsVersion, Price, Series, Totals, REL_FACET, REL_IMAGE,
};
use crate::OpdsError;

pub fn parse_opds2(bytes: &[u8], base_url: &str) -> Result<Feed, OpdsError> {
    let raw: RawFeed =
        serde_json::from_slice(bytes).map_err(|e| OpdsError::Parse(format!("opds2: {e}")))?;
    Ok(convert_feed(raw, base_url))
}

/// A standalone 2.0 publication document → one-entry feed.
pub fn parse_opds2_publication(bytes: &[u8], base_url: &str) -> Result<Feed, OpdsError> {
    let raw: RawPublication =
        serde_json::from_slice(bytes).map_err(|e| OpdsError::Parse(format!("opds2: {e}")))?;
    let entry = convert_publication(raw, base_url);
    Ok(Feed {
        version: OpdsVersion::V2,
        title: entry.title.clone(),
        id: entry.identifier.clone(),
        links: Vec::new(),
        entries: vec![entry],
        groups: Vec::new(),
        totals: Totals::default(),
    })
}

fn convert_feed(raw: RawFeed, base: &str) -> Feed {
    let totals = Totals {
        total_results: raw.metadata.number_of_items,
        items_per_page: raw.metadata.items_per_page,
        start_index: None,
        current_page: raw.metadata.current_page,
    };
    let mut entries: Vec<Entry> = raw
        .publications
        .into_iter()
        .map(|p| convert_publication(p, base))
        .collect();
    // Navigation items become entries with a single catalog link, so a
    // browse UI treats both feed kinds uniformly.
    for nav in raw.navigation {
        let title = nav.title.clone().unwrap_or_default();
        entries.push(Entry {
            title,
            links: vec![convert_link(nav, base)],
            ..Entry::default()
        });
    }
    let groups = raw
        .groups
        .into_iter()
        .map(|g| Group {
            title: g.metadata.title,
            links: g.links.into_iter().map(|l| convert_link(l, base)).collect(),
            entries: g
                .publications
                .into_iter()
                .map(|p| convert_publication(p, base))
                .chain(g.navigation.into_iter().map(|nav| {
                    let title = nav.title.clone().unwrap_or_default();
                    Entry {
                        title,
                        links: vec![convert_link(nav, base)],
                        ..Entry::default()
                    }
                }))
                .collect(),
        })
        .collect();

    // 2.0 facets[] → facet links with group names, matching the 1.2 shape
    // (the `active` flag exists only in 1.x output; it stays false here).
    let mut links: Vec<Link> = raw
        .links
        .into_iter()
        .map(|l| convert_link(l, base))
        .collect();
    for facet_group in raw.facets {
        for l in facet_group.links {
            let mut link = convert_link(l, base);
            link.facet_group = Some(facet_group.metadata.title.clone());
            if link.rel.is_empty() {
                link.rel = vec![REL_FACET.to_string()];
            }
            links.push(link);
        }
    }

    Feed {
        version: OpdsVersion::V2,
        title: raw.metadata.title,
        id: raw.metadata.identifier,
        links,
        entries,
        groups,
        totals,
    }
}

fn convert_publication(raw: RawPublication, base: &str) -> Entry {
    let md = raw.metadata;
    let mut links: Vec<Link> = raw
        .links
        .into_iter()
        .map(|l| convert_link(l, base))
        .collect();
    // 2.0 images carry no rel — the first is the cover by convention.
    for (i, img) in raw.images.into_iter().enumerate() {
        let mut link = convert_link(img, base);
        if i == 0 && link.rel.is_empty() {
            link.rel = vec![REL_IMAGE.to_string()];
        }
        links.push(link);
    }
    Entry {
        id: md.identifier.clone().unwrap_or_default(),
        title: md.title,
        authors: md.author.into_iter().map(|c| c.name()).collect(),
        language: md.language.into_iter().next(),
        published: md.published,
        publisher: md.publisher.map(|c| c.name()),
        identifier: md.identifier,
        summary: None,
        content_html: md.description,
        series: md.belongs_to.and_then(|b| b.series).map(|s| Series {
            name: s.name,
            position: s.position,
        }),
        links,
    }
}

fn convert_link(raw: RawLink, base: &str) -> Link {
    let props = raw.properties.unwrap_or_default();
    let mut indirect = Vec::new();
    flatten_indirect(&props.indirect_acquisition, &mut indirect);
    Link {
        href: resolve_url(base, &raw.href),
        rel: raw.rel.map(|r| r.0).unwrap_or_default(),
        media_type: raw.media_type.as_deref().map(MediaType::parse),
        title: raw.title,
        facet_group: None,
        active_facet: false, // the active flag exists only in 1.x output
        count: props.number_of_items,
        pse_count: None, // PSE exists only in 1.x output on every known server
        pse_last_read: None,
        pse_last_read_date: None,
        price: props.price.map(|p| Price {
            currency: p.currency,
            value: format_price(p.value),
        }),
        indirect,
        availability: props.availability.and_then(|a| a.state),
        holds_total: props.holds.and_then(|h| h.total),
        copies_available: props.copies.and_then(|c| c.available),
    }
}

fn flatten_indirect(chain: &[RawIndirect], out: &mut Vec<String>) {
    for item in chain {
        out.push(MediaType::parse(&item.media_type).essence);
        flatten_indirect(&item.child, out);
    }
}

fn format_price(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

// ---- Raw serde shapes ----

#[derive(Deserialize)]
struct RawFeed {
    metadata: RawFeedMetadata,
    #[serde(default)]
    links: Vec<RawLink>,
    #[serde(default)]
    navigation: Vec<RawLink>,
    #[serde(default)]
    publications: Vec<RawPublication>,
    #[serde(default)]
    groups: Vec<RawGroup>,
    #[serde(default)]
    facets: Vec<RawFacetGroup>,
}

#[derive(Deserialize)]
struct RawFacetGroup {
    metadata: RawGroupMetadata,
    #[serde(default)]
    links: Vec<RawLink>,
}

#[derive(Deserialize)]
struct RawFeedMetadata {
    title: String,
    identifier: Option<String>,
    #[serde(rename = "numberOfItems")]
    number_of_items: Option<u64>,
    #[serde(rename = "itemsPerPage")]
    items_per_page: Option<u64>,
    #[serde(rename = "currentPage")]
    current_page: Option<u64>,
}

#[derive(Deserialize)]
struct RawGroup {
    metadata: RawGroupMetadata,
    #[serde(default)]
    links: Vec<RawLink>,
    #[serde(default)]
    navigation: Vec<RawLink>,
    #[serde(default)]
    publications: Vec<RawPublication>,
}

#[derive(Deserialize)]
struct RawGroupMetadata {
    title: String,
}

#[derive(Deserialize)]
struct RawPublication {
    metadata: RawPubMetadata,
    #[serde(default)]
    links: Vec<RawLink>,
    #[serde(default)]
    images: Vec<RawLink>,
}

#[derive(Deserialize)]
struct RawPubMetadata {
    title: String,
    identifier: Option<String>,
    #[serde(default)]
    author: OneOrMany<Contributor>,
    publisher: Option<Contributor>,
    #[serde(default)]
    language: OneOrMany<String>,
    published: Option<String>,
    /// May contain raw HTML.
    description: Option<String>,
    #[serde(rename = "belongsTo")]
    belongs_to: Option<RawBelongsTo>,
}

#[derive(Deserialize)]
struct RawBelongsTo {
    series: Option<RawSeries>,
}

#[derive(Deserialize)]
struct RawSeries {
    name: String,
    position: Option<f64>,
}

/// Contributor: bare string or object with `name`.
#[derive(Deserialize)]
#[serde(untagged)]
enum Contributor {
    Name(String),
    Object { name: String },
}

impl Contributor {
    fn name(self) -> String {
        match self {
            Contributor::Name(n) => n,
            Contributor::Object { name } => name,
        }
    }
}

/// One value or an array of values (`rel`, `author`, `language`, ...).
struct OneOrMany<T>(Vec<T>);

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        OneOrMany(Vec::new())
    }
}

impl<T> IntoIterator for OneOrMany<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for OneOrMany<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw<T> {
            One(T),
            Many(Vec<T>),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::One(v) => OneOrMany(vec![v]),
            Raw::Many(v) => OneOrMany(v),
        })
    }
}

#[derive(Deserialize)]
struct RawLink {
    href: String,
    #[serde(default)]
    rel: Option<OneOrMany<String>>,
    #[serde(rename = "type")]
    media_type: Option<String>,
    title: Option<String>,
    properties: Option<RawLinkProperties>,
}

#[derive(Deserialize, Default)]
struct RawLinkProperties {
    #[serde(rename = "numberOfItems")]
    number_of_items: Option<u64>,
    price: Option<RawPrice>,
    #[serde(rename = "indirectAcquisition", default)]
    indirect_acquisition: Vec<RawIndirect>,
    availability: Option<RawAvailability>,
    holds: Option<RawHolds>,
    copies: Option<RawCopies>,
}

#[derive(Deserialize)]
struct RawPrice {
    currency: String,
    value: f64,
}

#[derive(Deserialize)]
struct RawIndirect {
    #[serde(rename = "type")]
    media_type: String,
    #[serde(default)]
    child: Vec<RawIndirect>,
}

#[derive(Deserialize)]
struct RawAvailability {
    state: Option<String>,
}

#[derive(Deserialize)]
struct RawHolds {
    total: Option<u32>,
}

#[derive(Deserialize)]
struct RawCopies {
    available: Option<u32>,
}
