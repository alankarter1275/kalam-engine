//! Parse every wire-format fixture in `fixtures/opds/` (the crate's
//! INTEROP.md). The facet-attribute assertions are the regression canary for the
//! parser choice: if they fail, a lossy feed crate has crept back in.

use std::path::PathBuf;

use opds_client::{parse_atom, parse_opds2, parse_opds2_publication, Feed, OpdsVersion};

const BASE: &str = "https://cat.example.com/opds/feed/new?page=2";

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/opds")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Compact deterministic dump for insta snapshots.
fn dump(feed: &Feed) -> String {
    let mut out = format!(
        "version: {:?}\ntitle: {}\nid: {:?}\ntotals: {:?}\n",
        feed.version, feed.title, feed.id, feed.totals
    );
    for link in &feed.links {
        out.push_str(&format!("link {:?} -> {}", feed_rels(link), link.href));
        if let Some(t) = &link.media_type {
            out.push_str(&format!(" [{}]", t.essence));
        }
        if let Some(g) = &link.facet_group {
            out.push_str(&format!(
                " facet-group={g} active={} count={:?}",
                link.active_facet, link.count
            ));
        }
        out.push('\n');
    }
    for group in &feed.groups {
        out.push_str(&format!(
            "group: {} ({} entries)\n",
            group.title,
            group.entries.len()
        ));
    }
    for entry in &feed.entries {
        out.push_str(&format!(
            "entry: {} | id={} authors={:?} lang={:?} published={:?} series={:?}\n",
            entry.title, entry.id, entry.authors, entry.language, entry.published, entry.series
        ));
        for link in &entry.links {
            out.push_str(&format!("  {:?} -> {}", feed_rels(link), link.href));
            if let Some(p) = &link.price {
                out.push_str(&format!(" price={} {}", p.value, p.currency));
            }
            if !link.indirect.is_empty() {
                out.push_str(&format!(" indirect={:?}", link.indirect));
            }
            if let Some(a) = &link.availability {
                out.push_str(&format!(
                    " availability={a} holds={:?} copies={:?}",
                    link.holds_total, link.copies_available
                ));
            }
            if let Some(c) = link.pse_count {
                out.push_str(&format!(
                    " pse:count={c} lastRead={:?} lastReadDate={:?}",
                    link.pse_last_read, link.pse_last_read_date
                ));
            }
            out.push('\n');
        }
    }
    out
}

fn feed_rels(link: &opds_client::Link) -> Vec<&str> {
    link.rel.iter().map(String::as_str).collect()
}

// ---- OPDS 1.2 Atom ----

#[test]
fn acquisition_atom_facet_canary() {
    let feed = parse_atom(&fixture("acquisition.atom.xml"), BASE).unwrap();
    // THE canary: foreign-namespace link attributes must survive parsing.
    let groups = feed.facet_groups();
    assert_eq!(groups.len(), 2, "Language and Format facet groups");
    let (name, language) = &groups[0];
    assert_eq!(name, "Language");
    assert_eq!(language.len(), 2);
    assert!(language[0].active_facet, "opds:activeFacet lost");
    assert_eq!(language[0].count, Some(80), "thr:count lost");
    assert!(!language[1].active_facet);
    assert_eq!(groups[1].0, "Format");
}

#[test]
fn acquisition_atom_totals_and_pagination() {
    let feed = parse_atom(&fixture("acquisition.atom.xml"), BASE).unwrap();
    assert_eq!(feed.totals.total_results, Some(120));
    assert_eq!(feed.totals.items_per_page, Some(50));
    assert_eq!(feed.totals.start_index, Some(51));
    assert!(feed.next().unwrap().href.ends_with("/opds/feed/new?page=3"));
    // The back-rel is `previous`, not `prev`.
    assert!(feed.previous().unwrap().href.ends_with("/opds/feed/new"));
    assert!(feed.search().is_some());
}

#[test]
fn acquisition_atom_commercial_extensions() {
    let feed = parse_atom(&fixture("acquisition.atom.xml"), BASE).unwrap();
    let gopl = &feed.entries[0];
    assert_eq!(gopl.id, "urn:isbn:9780134190440");
    assert_eq!(gopl.authors, vec!["Alan Donovan", "Brian Kernighan"]);
    let buy = gopl
        .links
        .iter()
        .find(|l| l.has_rel("http://opds-spec.org/acquisition/buy"))
        .unwrap();
    let price = buy.price.as_ref().unwrap();
    assert_eq!(price.value, "39.99");
    assert_eq!(price.currency, "USD");
    assert_eq!(
        buy.indirect,
        vec![
            "application/vnd.readium.lcp.license.v1.0+json",
            "application/epub+zip"
        ],
        "nested indirectAcquisition chain, outermost first"
    );
    let borrow = gopl
        .links
        .iter()
        .find(|l| l.has_rel("http://opds-spec.org/acquisition/borrow"))
        .unwrap();
    assert_eq!(borrow.availability.as_deref(), Some("unavailable"));
    assert_eq!(borrow.holds_total, Some(12));
    assert_eq!(borrow.copies_available, Some(0));
}

#[test]
fn acquisition_atom_pse_link() {
    let feed = parse_atom(&fixture("acquisition.atom.xml"), BASE).unwrap();
    let comic = &feed.entries[1];
    // Opaque id with slashes — never normalized.
    assert_eq!(comic.id, "urn:example:chapter/demo/c12");
    let stream = comic.pse_stream().expect("stream link");
    assert!(
        stream.href.contains("{pageNumber}") && stream.href.contains("{maxWidth}"),
        "template braces must survive resolution: {}",
        stream.href
    );
    assert!(
        stream.href.starts_with("https://cat.example.com/"),
        "href resolved"
    );
    assert_eq!(stream.pse_count, Some(35));
    assert_eq!(stream.pse_last_read, Some(10));
    assert!(stream.pse_last_read_date.is_some());
    // Complete-entry follow target for lazy PSE.
    assert!(comic.complete_entry().is_some());

    let url = opds_client::pse_page_url(stream, 0, Some(1200));
    assert!(
        url.contains("page=0") && url.contains("width=1200"),
        "{url}"
    );
}

#[test]
fn navigation_atom_snapshot() {
    let feed = parse_atom(&fixture("navigation.atom.xml"), BASE).unwrap();
    insta::assert_snapshot!(dump(&feed));
}

#[test]
fn standalone_entry_atom() {
    let feed = parse_atom(&fixture("entry.atom.xml"), BASE).unwrap();
    assert_eq!(
        feed.entries.len(),
        1,
        "standalone entry parses as one-entry feed"
    );
    insta::assert_snapshot!(dump(&feed));
}

#[test]
fn entry_pse_atom_snapshot() {
    let feed = parse_atom(&fixture("entry-pse.atom.xml"), BASE).unwrap();
    let entry = &feed.entries[0];
    assert!(entry.pse_stream().is_some());
    insta::assert_snapshot!(dump(&feed));
}

/// The immutable golden: byte-identical input must keep producing this
/// parse. Do not regenerate the fixture.
#[test]
fn pse_feed_golden() {
    let feed = parse_atom(&fixture("pse-feed-golden.atom.xml"), BASE).unwrap();
    insta::assert_snapshot!(dump(&feed));
}

// ---- OPDS 2.0 JSON ----

#[test]
fn navigation_opds2_snapshot() {
    let feed = parse_opds2(&fixture("navigation.opds2.json"), BASE).unwrap();
    assert_eq!(feed.version, OpdsVersion::V2);
    assert!(!feed.groups.is_empty(), "groups[] parsed");
    insta::assert_snapshot!(dump(&feed));
}

#[test]
fn acquisition_opds2_counters_and_facets() {
    let feed = parse_opds2(&fixture("acquisition.opds2.json"), BASE).unwrap();
    assert_eq!(feed.totals.total_results, Some(120));
    assert_eq!(feed.totals.current_page, Some(2), "2.0 uses currentPage");
    assert_eq!(feed.totals.start_index, None, "startIndex is 1.2-only");
    let groups = feed.facet_groups();
    assert_eq!(groups.len(), 2);
    // The active flag exists only in 1.x output — 2.0 must not invent it.
    assert!(groups
        .iter()
        .all(|(_, links)| links.iter().all(|l| !l.active_facet)));
    insta::assert_snapshot!(dump(&feed));
}

#[test]
fn publication_opds2_polymorphism() {
    let feed = parse_opds2_publication(&fixture("publication.opds2.json"), BASE).unwrap();
    let publication = &feed.entries[0];
    // author: object + bare string forms.
    assert_eq!(publication.authors, vec!["Alan Donovan", "Brian Kernighan"]);
    // description is raw HTML.
    assert!(publication.content_html.as_deref().unwrap().contains("<p>"));
    // series via belongsTo (2.0-only field).
    let series = publication.series.as_ref().unwrap();
    assert_eq!(series.name, "Professional Computing");
    assert_eq!(series.position, Some(1.0));
    insta::assert_snapshot!(dump(&feed));
}

// ---- Auth document ----

#[test]
fn authentication_document() {
    let doc: opds_client::AuthDocument =
        serde_json::from_slice(&fixture("authentication.opds-auth.json")).unwrap();
    assert_eq!(doc.title, "Example Catalog");
    let basic = doc.basic_flow().expect("basic flow offered");
    assert_eq!(
        basic.labels.get("login").map(String::as_str),
        Some("Username")
    );
    assert!(doc.links.iter().any(|l| l.rel.as_deref() == Some("logo")));
}

// ---- OpenSearch ----

#[test]
fn opensearch_description() {
    let template = opds_client::opensearch_template(&fixture("opensearch.xml"), BASE).unwrap();
    assert!(template.contains("{searchTerms}"), "{template}");
    // The fixture's template is absolute — it must pass through untouched.
    assert_eq!(template, "https://example.com/opds/search?q={searchTerms}");
}

// ---- Media types ----

#[test]
fn media_type_essence_comparison() {
    use opds_client::MediaType;
    let t = MediaType::parse("application/atom+xml;profile=opds-catalog;kind=acquisition");
    assert!(t.is_atom());
    assert!(t.is_opds_catalog());
    assert!(!t.is_entry_document());
    assert_eq!(t.param("kind"), Some("acquisition"));
    let entry = MediaType::parse("application/atom+xml;type=entry;profile=opds-catalog");
    assert!(entry.is_entry_document());
    assert!(MediaType::parse("application/vnd.comicbook+zip").is_comic_archive());
    // Never compared by raw string equality: parameter order/spacing differ.
    let spaced = MediaType::parse("application/atom+xml; kind=acquisition; profile=opds-catalog");
    assert!(spaced.is_opds_catalog());
}

/// Entity references arrive from quick-xml as events of their own, split
/// out of the surrounding text. Dropping them silently deleted both the
/// character and the spaces around it — `Science &amp; Nature` came back
/// as `ScienceNature` — and ampersands are everywhere in catalogue titles.
#[test]
fn entities_survive_in_titles_and_summaries() {
    let xml = br#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Science &amp; Nature</title>
  <id>urn:test</id>
  <entry>
    <title>Ha&#39;penny &lt;draft&gt;</title>
    <id>urn:test:1</id>
    <summary>Tom &amp; Jerry, &quot;quoted&quot;</summary>
  </entry>
</feed>"#;
    let feed = parse_atom(xml, "https://example.invalid/").unwrap();
    assert_eq!(feed.title, "Science & Nature");
    assert_eq!(feed.entries[0].title, "Ha'penny <draft>");
    assert_eq!(
        feed.entries[0].summary.as_deref(),
        Some("Tom & Jerry, \"quoted\"")
    );
}
