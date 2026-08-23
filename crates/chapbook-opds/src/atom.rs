//! OPDS 1.2 Atom parsing — the canonical dialect — with namespace-aware
//! quick-xml, preserving the foreign-namespace link attributes
//! (`opds:facetGroup`, `opds:activeFacet`, `thr:count`, `pse:*`) that lossy
//! feed crates drop (docs/OPDS-INTEROP.md §0).

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::NamespaceResolver;
use quick_xml::name::ResolveResult;
use quick_xml::NsReader;

use crate::href::resolve_url;
use crate::model::{Entry, Feed, Link, MediaType, OpdsVersion, Price, Totals};
use crate::OpdsError;

const NS_ATOM: &str = "http://www.w3.org/2005/Atom";
const NS_OPDS: &str = "http://opds-spec.org/2010/catalog";
const NS_THR: &str = "http://purl.org/syndication/thread/1.0";
const NS_PSE: &str = "http://vaemendis.net/opds-pse/ns";
const NS_OPENSEARCH: &str = "http://a9.com/-/spec/opensearch/1.1/";
const NS_DCTERMS: &str = "http://purl.org/dc/terms/";

/// A namespace-resolved attribute: (namespace, local name, value).
type Attr = (String, String, String);

/// Parse an OPDS 1.x document: an acquisition/navigation `<feed>`, or a
/// standalone complete `<entry>` (which comes back as a one-entry feed).
pub fn parse_atom(bytes: &[u8], base_url: &str) -> Result<Feed, OpdsError> {
    let mut reader = NsReader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut feed = Feed {
        version: OpdsVersion::V1,
        title: String::new(),
        id: None,
        links: Vec::new(),
        entries: Vec::new(),
        groups: Vec::new(),
        totals: Totals::default(),
    };

    let mut in_entry: Option<Entry> = None;
    let mut open_link: Option<Link> = None;
    let mut in_author = false;
    let mut standalone_entry = false;
    let mut text = String::new();
    let mut depth = 0usize;
    let mut depth_of_open_link = 0usize;

    let mut buf = Vec::new();
    loop {
        let (ns_res, event) = reader
            .read_resolved_event_into(&mut buf)
            .map_err(|e| OpdsError::Parse(format!("atom: {e}")))?;
        let ns: String = ns_str(&ns_res).to_string();
        match event {
            Event::Start(e) => {
                depth += 1;
                let local = e.local_name().as_ref().to_string();
                let attrs = collect_attrs(reader.resolver_mut(), &e);
                text.clear();
                if depth == 1 && ns == NS_ATOM && local == "entry" {
                    standalone_entry = true;
                    in_entry = Some(Entry::default());
                } else if ns == NS_ATOM && local == "link" {
                    open_link = Some(link_from_attrs(&attrs, base_url));
                    depth_of_open_link = depth;
                } else {
                    handle_open(
                        &ns,
                        &local,
                        &attrs,
                        &mut in_entry,
                        &mut open_link,
                        &mut in_author,
                    );
                }
            }
            Event::Empty(e) => {
                let local = e.local_name().as_ref().to_string();
                let attrs = collect_attrs(reader.resolver_mut(), &e);
                if ns == NS_ATOM && local == "link" {
                    let link = link_from_attrs(&attrs, base_url);
                    match (&mut open_link, &mut in_entry) {
                        // links never nest; an open link means this is odd
                        // input — attach to whatever context is innermost.
                        (Some(_), _) => {}
                        (None, Some(entry)) => entry.links.push(link),
                        (None, None) => feed.links.push(link),
                    }
                } else {
                    handle_open(
                        &ns,
                        &local,
                        &attrs,
                        &mut in_entry,
                        &mut open_link,
                        &mut in_author,
                    );
                }
            }
            Event::End(e) => {
                let local = e.local_name().as_ref().to_string();
                handle_close(
                    &ns,
                    &local,
                    &text,
                    &mut feed,
                    &mut in_entry,
                    &mut open_link,
                    &mut in_author,
                );
                if local == "link" && ns == NS_ATOM && depth == depth_of_open_link {
                    if let Some(link) = open_link.take() {
                        match &mut in_entry {
                            Some(entry) => entry.links.push(link),
                            None => feed.links.push(link),
                        }
                    }
                }
                if ns == NS_ATOM && local == "entry" && !standalone_entry {
                    if let Some(entry) = in_entry.take() {
                        feed.entries.push(entry);
                    }
                }
                text.clear();
                depth = depth.saturating_sub(1);
            }
            Event::Text(t) => {
                text.push_str(&t.xml_content(quick_xml::XmlVersion::Implicit1_0));
            }
            Event::CData(t) => {
                text.push_str(&t.into_inner());
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if standalone_entry {
        if let Some(entry) = in_entry.take() {
            feed.title = entry.title.clone();
            feed.entries.push(entry);
        }
    }
    Ok(feed)
}

fn ns_str<'a>(ns: &'a ResolveResult) -> &'a str {
    match ns {
        ResolveResult::Bound(n) => n.as_ref(),
        _ => "",
    }
}

fn collect_attrs(resolver: &NamespaceResolver, e: &BytesStart) -> Vec<Attr> {
    e.attributes()
        .flatten()
        .map(|attr| {
            let (ns, local) = resolver.resolve_attribute(attr.key);
            let value = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|v| v.to_string())
                .unwrap_or_default();
            (ns_str(&ns).to_string(), local.as_ref().to_string(), value)
        })
        .collect()
}

fn attr<'a>(attrs: &'a [Attr], ns: &str, local: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(a_ns, a_local, _)| a_ns == ns && a_local == local)
        .map(|(_, _, v)| v.as_str())
}

fn link_from_attrs(attrs: &[Attr], base_url: &str) -> Link {
    let mut link = Link::default();
    for (ns, local, value) in attrs {
        match (ns.as_str(), local.as_str()) {
            ("", "rel") => link.rel = vec![value.clone()],
            ("", "href") => link.href = resolve_url(base_url, value),
            ("", "type") => link.media_type = Some(MediaType::parse(value)),
            ("", "title") => link.title = Some(value.clone()),
            (NS_OPDS, "facetGroup") => link.facet_group = Some(value.clone()),
            (NS_OPDS, "activeFacet") => link.active_facet = value.eq_ignore_ascii_case("true"),
            (NS_THR, "count") => link.count = value.parse().ok(),
            (NS_PSE, "count") => link.pse_count = value.parse().ok(),
            (NS_PSE, "lastRead") => link.pse_last_read = value.parse().ok(),
            (NS_PSE, "lastReadDate") => link.pse_last_read_date = Some(value.clone()),
            _ => {}
        }
    }
    link
}

fn handle_open(
    ns: &str,
    local: &str,
    attrs: &[Attr],
    in_entry: &mut Option<Entry>,
    open_link: &mut Option<Link>,
    in_author: &mut bool,
) {
    match (ns, local) {
        (NS_ATOM, "entry") => *in_entry = Some(Entry::default()),
        (NS_ATOM, "author") => *in_author = true,
        // Acquisition-link extension elements (children of an open <link>).
        (NS_OPDS, "price") => {
            if let Some(link) = open_link {
                link.price = Some(Price {
                    currency: attr(attrs, "", "currencycode")
                        .unwrap_or_default()
                        .to_string(),
                    value: String::new(), // filled from element text on close
                });
            }
        }
        (NS_OPDS, "indirectAcquisition") => {
            if let Some(link) = open_link {
                if let Some(t) = attr(attrs, "", "type") {
                    link.indirect.push(MediaType::parse(t).essence);
                }
            }
        }
        (NS_OPDS, "availability") => {
            if let Some(link) = open_link {
                link.availability = attr(attrs, "", "status").map(str::to_string);
            }
        }
        (NS_OPDS, "holds") => {
            if let Some(link) = open_link {
                link.holds_total = attr(attrs, "", "total").and_then(|v| v.parse().ok());
            }
        }
        (NS_OPDS, "copies") => {
            if let Some(link) = open_link {
                link.copies_available = attr(attrs, "", "available").and_then(|v| v.parse().ok());
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_close(
    ns: &str,
    local: &str,
    text: &str,
    feed: &mut Feed,
    in_entry: &mut Option<Entry>,
    open_link: &mut Option<Link>,
    in_author: &mut bool,
) {
    if let Some(link) = open_link.as_mut() {
        if ns == NS_OPDS && local == "price" {
            if let Some(price) = &mut link.price {
                price.value = text.trim().to_string();
            }
            return;
        }
    }
    match (ns, local, in_entry.as_mut()) {
        (NS_ATOM, "author", _) => *in_author = false,
        (NS_ATOM, "name", Some(entry)) if *in_author => {
            entry.authors.push(text.trim().to_string());
        }
        (NS_ATOM, "id", Some(entry)) => entry.id = text.trim().to_string(),
        (NS_ATOM, "id", None) => feed.id = Some(text.trim().to_string()),
        (NS_ATOM, "title", Some(entry)) if entry.title.is_empty() => {
            entry.title = text.trim().to_string();
        }
        (NS_ATOM, "title", None) if feed.title.is_empty() => {
            feed.title = text.trim().to_string();
        }
        (NS_ATOM, "summary", Some(entry)) => entry.summary = some_text(text),
        (NS_ATOM, "content", Some(entry)) => entry.content_html = some_text(text),
        (NS_DCTERMS, "language", Some(entry)) => entry.language = some_text(text),
        (NS_DCTERMS, "issued", Some(entry)) => entry.published = some_text(text),
        (NS_DCTERMS, "publisher", Some(entry)) => entry.publisher = some_text(text),
        (NS_DCTERMS, "identifier", Some(entry)) => entry.identifier = some_text(text),
        (NS_OPENSEARCH, "totalResults", None) => {
            feed.totals.total_results = text.trim().parse().ok();
        }
        (NS_OPENSEARCH, "itemsPerPage", None) => {
            feed.totals.items_per_page = text.trim().parse().ok();
        }
        (NS_OPENSEARCH, "startIndex", None) => {
            feed.totals.start_index = text.trim().parse().ok();
        }
        _ => {}
    }
}

fn some_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
