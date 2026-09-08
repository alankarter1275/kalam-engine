//! StreamedComic against a local server: lazy-PSE resolution (feed →
//! complete entry → stream), 0-based page templating, disk caching, and
//! the Publication surface.
//!
//! Driven over the bundled `ureq` transport, so it needs that feature. The
//! transport-free half of the story — that a caller can supply its own
//! `HttpClient` and everything still works — is `opds-client`'s
//! `injected_transport.rs`.
#![cfg(feature = "ureq")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chapbook_core::{BookKind, Publication};
use chapbook_opds::{OpdsClient, StreamedComic};

fn lazy_feed(base: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:cat:comics</id><title>Comics</title>
  <entry><id>urn:c1</id><title>Test Comic</title>
    <link rel="alternate" href="{base}/entry" type="application/atom+xml;type=entry;profile=opds-catalog"/>
  </entry>
</feed>"#
    )
}

fn complete_entry(base: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<entry xmlns="http://www.w3.org/2005/Atom" xmlns:pse="http://vaemendis.net/opds-pse/ns">
  <id>urn:c1</id><title>Test Comic</title>
  <author><name>Testsuke Example</name></author>
  <link rel="http://vaemendis.net/opds-pse/stream"
        href="{base}/pages?page={{pageNumber}}&amp;width={{maxWidth}}"
        type="image/jpeg" pse:count="3" pse:lastRead="2"/>
</entry>"#
    )
}

/// Serves the lazy feed, the complete entry, and 3 pages whose bytes embed
/// the page number; counts page fetches to prove the disk cache works.
fn spawn_server(page_hits: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served_base = base.clone();
    std::thread::spawn(move || {
        for _ in 0..16 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim_end().is_empty() {
                    break;
                }
            }
            let (content_type, body) = if path.starts_with("/feed") {
                ("application/atom+xml", lazy_feed(&served_base).into_bytes())
            } else if path.starts_with("/entry") {
                (
                    "application/atom+xml;type=entry",
                    complete_entry(&served_base).into_bytes(),
                )
            } else if path.starts_with("/pages") {
                page_hits.fetch_add(1, Ordering::SeqCst);
                let page = path
                    .split("page=")
                    .nth(1)
                    .and_then(|s| s.split('&').next())
                    .unwrap_or("?");
                ("image/jpeg", format!("JPEGDATA:{page}").into_bytes())
            } else {
                ("text/plain", b"not found".to_vec())
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    base
}

#[test]
fn lazy_open_pages_and_cache() {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = spawn_server(hits.clone());
    let cache_root = std::env::temp_dir().join(format!("chapbook-pse-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache_root);

    let comic = StreamedComic::open(
        OpdsClient::with_ureq(),
        &format!("{base}/feed"),
        &cache_root,
    )
    .unwrap();
    assert_eq!(comic.kind(), BookKind::Comic);
    assert_eq!(comic.metadata().title.as_deref(), Some("Test Comic"));
    assert_eq!(comic.spine().len(), 3);
    // pse:lastRead is 1-based; resume is 0-based.
    assert_eq!(comic.resume_page(), Some(1));
    // 0-based template substitution.
    assert!(
        comic.spine()[0].href.contains("page=0"),
        "{}",
        comic.spine()[0].href
    );

    // Fetch every page; bytes prove which page the server saw.
    for i in 0..3 {
        let data = comic.unit_bytes(i).unwrap();
        assert_eq!(data, format!("JPEGDATA:{i}").into_bytes());
    }
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    // Second reads come from the disk cache: no new requests.
    for i in 0..3 {
        comic.unit_bytes(i).unwrap();
    }
    assert_eq!(hits.load(Ordering::SeqCst), 3, "cache must absorb re-reads");
    // And a fresh instance reuses the same cache.
    let comic2 = StreamedComic::open(
        OpdsClient::with_ureq(),
        &format!("{base}/feed"),
        &cache_root,
    )
    .unwrap();
    assert_eq!(comic2.unit_bytes(1).unwrap(), b"JPEGDATA:1");
    assert_eq!(hits.load(Ordering::SeqCst), 3);

    assert!(comic.unit_bytes(3).is_err());
    let _ = std::fs::remove_dir_all(&cache_root);
}
