//! Walk a live OPDS catalog end to end and print what came back.
//!
//! Point it at any server; [`mocklib`] is the convenient one, because it
//! serves every surface at once over a few hundred entries.
//!
//! Fixtures pin the shapes we have seen; this walks the surfaces in one
//! flow against something that answers like a server: pagination and
//! facets over a few hundred entries, an OpenSearch description with
//! refinement parameters, a 401 with an Authentication Document, PSE
//! comics, a download, and a position round trip.
//!
//! It is an example rather than a test because it needs a server running,
//! which CI does not have. `cargo clippy --all-targets` still compiles it,
//! so it cannot rot.
//!
//! ```sh
//! cd ~/wksp/mocklib && go run ./cmd/mocklib -addr :8096 -auth reader:secret
//! CATALOG=http://localhost:8096 CATALOG_AUTH=reader:secret \
//!   cargo run -p chapbook-opds --features progression --example live_catalog
//! ```
//!
//! [`mocklib`]: https://github.com/ophymx/mocklib

use chapbook_opds::{Feed, Link, OpdsClient, OpdsError, REL_FACET, REL_PSE_STREAM};

fn main() {
    let base = std::env::var("CATALOG").unwrap_or_else(|_| "http://localhost:8080".into());
    let mut client = OpdsClient::with_ureq();
    if let Ok(credential) = std::env::var("CATALOG_AUTH") {
        let (user, password) = credential
            .split_once(':')
            .expect("CATALOG_AUTH is user:password");
        client.set_basic_auth(user, password);
    }

    let root = navigation(&client, &base);
    let all = acquisition(&client, &base);
    search(&client, &base, &root);
    opds2(&client, &base);
    comics(&client, &base);
    challenge(&base);
    download(&client, &all);

    #[cfg(feature = "progression")]
    position(&client, &all);
}

fn navigation(client: &OpdsClient, base: &str) -> Feed {
    let root = client
        .fetch(&format!("{base}/opds/"))
        .expect("catalog root");
    println!(
        "ROOT {:?} — {} nav entries, {} links",
        root.title,
        root.entries.len(),
        root.links.len()
    );
    for entry in &root.entries {
        println!("  {} -> {}", entry.title, first_href(&entry.links));
    }
    root
}

fn acquisition(client: &OpdsClient, base: &str) -> Feed {
    let all = client
        .fetch(&format!("{base}/opds/feed/all"))
        .expect("acquisition feed");
    println!(
        "\nALL {} entries, totals {:?}",
        all.entries.len(),
        all.totals
    );
    let facets: Vec<&Link> = all.links.iter().filter(|l| l.has_rel(REL_FACET)).collect();
    let groups: std::collections::BTreeSet<&str> = facets
        .iter()
        .filter_map(|f| f.facet_group.as_deref())
        .collect();
    println!("  {} facets in groups {groups:?}", facets.len());
    println!(
        "  counted facets: {}",
        facets.iter().filter(|f| f.count.is_some()).count()
    );
    println!("  next page: {:?}", rel_href(&all.links, "next"));

    let entry = all.entries.first().expect("at least one publication");
    println!("\nENTRY {:?} ({})", entry.title, entry.id);
    for link in &entry.links {
        println!("  {:?} -> {}", link.rel, link.href);
    }
    all
}

fn search(client: &OpdsClient, base: &str, root: &Feed) {
    // The interesting half is a description document whose template
    // carries refinement parameters beside the query. A client that leaves
    // them unfilled gets 200 and an empty feed.
    match client.search(root, &format!("{base}/opds/"), "harbour") {
        Ok(feed) => println!(
            "\nSEARCH {} entries, totals {:?}",
            feed.entries.len(),
            feed.totals
        ),
        Err(err) => println!("\nSEARCH failed: {err}"),
    }
}

fn opds2(client: &OpdsClient, base: &str) {
    match client.fetch_opds2(&format!("{base}/opds/feed/all?version=2")) {
        Ok(feed) => println!(
            "\nOPDS2 {} entries, totals {:?}",
            feed.entries.len(),
            feed.totals
        ),
        Err(err) => println!("\nOPDS2 failed: {err}"),
    }
    match client.fetch_opds2(&format!("{base}/opds/feed/showcase?version=2")) {
        Ok(feed) => {
            println!("SHOWCASE {} groups", feed.groups.len());
            for group in &feed.groups {
                println!("  {:?}: {} entries", group.title, group.entries.len());
            }
        }
        Err(err) => println!("SHOWCASE failed: {err}"),
    }
}

fn comics(client: &OpdsClient, base: &str) {
    let comics = client
        .fetch(&format!("{base}/opds/feed/comics"))
        .expect("comics feed");
    println!("\nCOMICS {} entries", comics.entries.len());
    let mut fetched = false;
    for comic in &comics.entries {
        let Some(stream) = comic.links.iter().find(|l| l.has_rel(REL_PSE_STREAM)) else {
            continue;
        };
        println!(
            "  {} — {:?} pages, lastRead {:?}",
            comic.title, stream.pse_count, stream.pse_last_read
        );
        if !fetched {
            fetched = true;
            match client.fetch_pse_page(stream, 0, Some(600)) {
                Ok((bytes, content_type)) => {
                    println!("    page 0: {} bytes, {content_type:?}", bytes.len())
                }
                Err(err) => println!("    page 0 failed: {err}"),
            }
        }
    }
}

fn challenge(base: &str) {
    // A fresh client with no credentials: the 401 has to arrive carrying
    // the Authentication Document, not as a bare status.
    let anonymous = OpdsClient::with_ureq();
    match anonymous.fetch(&format!("{base}/opds/feed/all")) {
        Ok(_) => println!("\nANON: server did not challenge (running without -auth)"),
        Err(OpdsError::AuthRequired(Some(doc))) => println!(
            "\nANON: 401 with auth document {:?}, {} flow(s)",
            doc.title,
            doc.authentication.len()
        ),
        Err(OpdsError::AuthRequired(None)) => println!("\nANON: 401 with no auth document"),
        Err(err) => println!("\nANON: {err}"),
    }
}

fn download(client: &OpdsClient, all: &Feed) {
    let entry = all.entries.first().expect("at least one publication");
    let Some(acquisition) = entry.links.iter().find(|l| {
        l.rel
            .iter()
            .any(|r| r.starts_with(chapbook_opds::REL_ACQ_PREFIX))
    }) else {
        println!("\nDOWNLOAD: no acquisition link");
        return;
    };
    let dest = std::env::temp_dir().join("chapbook-live-catalog-download");
    match client.download(&acquisition.href, &dest) {
        Ok(()) => {
            let len = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            println!("\nDOWNLOAD {len} bytes of {:?}", entry.title);
            std::fs::remove_file(&dest).ok();
        }
        Err(err) => println!("\nDOWNLOAD failed: {err}"),
    }
}

/// A position out through the mapping, into the server, and back — the
/// claim that a `LayeredLocator` serializes straight into Progression 1.0,
/// checked against something that stores and re-serves it.
#[cfg(feature = "progression")]
fn position(client: &OpdsClient, all: &Feed) {
    use chapbook_core::{LayeredLocator, Quote, LOCATOR_VERSION};
    use chapbook_opds::progression::{from_progression, to_progression, Device, REL_PROGRESSION};

    let entry = all.entries.first().expect("at least one publication");
    let Some(link) = entry.links.iter().find(|l| l.has_rel(REL_PROGRESSION)) else {
        println!("\nPROGRESSION: no service link (running without -auth)");
        return;
    };
    println!("\nPROGRESSION service {}", link.href);

    let device = Device {
        id: "urn:uuid:5f2b1c8e-mocklib-example".into(),
        name: "chapbook".into(),
    };
    let locator = LayeredLocator {
        spine_href: "OEBPS/chapter4.xhtml".into(),
        spine_index: 3,
        char_offset: 1200,
        locator_version: LOCATOR_VERSION,
        quote: Quote {
            prefix: "the harbour was ".into(),
            exact: String::new(),
            suffix: "quiet that morning".into(),
        },
        spine_fraction: 0.25,
        book_progression: 0.42,
    };
    let now = "2026-08-30T12:00:00Z";
    let doc = to_progression(&locator, device, now, Some("Chapter Four".into()));
    println!("  PUT references {:?}", doc.references);
    match client.put_progression(&link.href, &doc) {
        Ok(update) => println!("  PUT -> {}", update_name(&update)),
        Err(err) => {
            println!("  PUT failed: {err}");
            return;
        }
    }

    // Two devices on the same book: the older point must not win.
    let stale = chapbook_opds::progression::Progression {
        modified: "2020-01-01T00:00:00Z".into(),
        progression: 0.01,
        ..doc.clone()
    };
    match client.put_progression(&link.href, &stale) {
        Ok(update) => println!("  stale PUT -> {}", update_name(&update)),
        Err(err) => println!("  stale PUT failed: {err}"),
    }

    match client.fetch_progression(&link.href) {
        Ok(Some(stored)) => {
            let back = from_progression(&stored);
            println!(
                "  GET -> {:.2} at {:?}, quote {:?}",
                back.progression,
                back.spine_href,
                back.quote.as_ref().map(|q| (&q.prefix, &q.exact))
            );
            assert_eq!(
                back.spine_href.as_deref(),
                Some(locator.spine_href.as_str())
            );
            assert_eq!(
                back.quote.as_ref().map(|q| q.exact.as_str()),
                Some(locator.quote.suffix.as_str()),
                "a point's anchoring text is the run that starts at it"
            );
            assert_eq!(back.progression, locator.book_progression);
            println!("  round trip through the server: ok");
        }
        Ok(None) => println!("  GET -> no progression recorded"),
        Err(err) => println!("  GET failed: {err}"),
    }
}

#[cfg(feature = "progression")]
fn update_name(update: &chapbook_opds::progression::ProgressionUpdate) -> String {
    use chapbook_opds::progression::ProgressionUpdate::*;
    match update {
        Stored(_) => "stored".into(),
        Created(_) => "created".into(),
        Refused(refusal) => format!("refused {} ({:?})", refusal.status, refusal.reason),
    }
}

fn first_href(links: &[Link]) -> &str {
    links.first().map(|l| l.href.as_str()).unwrap_or("-")
}

fn rel_href<'a>(links: &'a [Link], rel: &str) -> Option<&'a str> {
    links
        .iter()
        .find(|l| l.has_rel(rel))
        .map(|l| l.href.as_str())
}
