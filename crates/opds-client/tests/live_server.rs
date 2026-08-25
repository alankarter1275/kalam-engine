//! The bundled `ureq` transport against a live (local) HTTP server: mid-flow
//! 401 with an Authentication Document, Basic-auth retry, and atomic
//! downloads.
//!
//! The protocol flow itself is covered transport-free in
//! `injected_transport.rs`; what this adds is that `UreqHttp` honors the
//! contract in `src/http.rs` — chiefly that it hands 401 back as a response
//! with its body intact rather than as an error.
#![cfg(feature = "ureq")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;

use opds_client::{OpdsClient, OpdsError};

const FEED: &str = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:cat:private</id><title>Private Shelf</title>
  <entry><id>urn:b1</id><title>Book One</title>
    <link rel="http://opds-spec.org/acquisition/open-access" href="/dl/b1.epub" type="application/epub+zip"/>
  </entry>
</feed>"#;

const AUTH_DOC: &str = r#"{
  "title": "Private Shelf",
  "authentication": [{"type": "http://opds-spec.org/auth/basic",
                      "labels": {"login": "User", "password": "Pass"}}]
}"#;

/// One-thread HTTP server: requires `Authorization: Basic dXNlcjpwdw==`
/// (user:pw) for every path; serves a feed and a download.
fn spawn_server(requests_to_serve: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for _ in 0..requests_to_serve {
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
            let mut authorized = false;
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if lower == "authorization: basic dxnlcjpwdw==" {
                    authorized = true;
                }
                if let Some(v) = lower.strip_prefix("content-length: ") {
                    content_length = v.parse().unwrap_or(0);
                }
            }
            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).unwrap();
            }

            let (status, content_type, body): (&str, &str, Vec<u8>) = if !authorized {
                (
                    "401 Unauthorized",
                    "application/opds-authentication+json",
                    AUTH_DOC.as_bytes().to_vec(),
                )
            } else if path.starts_with("/dl/") {
                (
                    "200 OK",
                    "application/epub+zip",
                    b"PK\x03\x04fake-epub-bytes".to_vec(),
                )
            } else {
                (
                    "200 OK",
                    "application/atom+xml;profile=opds-catalog",
                    FEED.as_bytes().to_vec(),
                )
            };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    format!("http://{addr}")
}

#[test]
fn mid_flow_401_surfaces_auth_document_then_basic_retry_succeeds() {
    let base = spawn_server(3);
    let mut client = OpdsClient::with_ureq();

    // 1. Unauthenticated: 401 with a parsed Authentication Document.
    let err = client.fetch(&format!("{base}/opds/")).unwrap_err();
    let OpdsError::AuthRequired(Some(doc)) = err else {
        panic!("expected AuthRequired with document, got {err:?}");
    };
    assert_eq!(doc.title, "Private Shelf");
    assert!(doc.basic_flow().is_some());

    // 2. With credentials: the same fetch succeeds.
    client.set_basic_auth("user", "pw");
    let feed = client.fetch(&format!("{base}/opds/")).unwrap();
    assert_eq!(feed.title, "Private Shelf");
    assert_eq!(feed.entries.len(), 1);

    // 3. Download an acquisition: temp file + atomic rename, no .part left.
    let dir = std::env::temp_dir().join(format!("opds-client-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("b1.epub");
    let url = &feed.entries[0].links[0].href;
    assert!(url.starts_with(&base), "acquisition href resolved: {url}");
    client.download(url, &dest).unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"));
    assert!(
        !dir.join("b1.part").exists() && !dest.with_extension("part").exists(),
        "temp file must be renamed away"
    );
    std::fs::remove_dir_all(&dir).ok();
}
