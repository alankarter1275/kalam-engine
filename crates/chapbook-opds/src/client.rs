//! The blocking HTTP client: fetch/sniff/parse feeds, Basic auth with
//! Authentication Document surfacing, atomic downloads, PSE page urls,
//! OpenSearch. Behavior contract: docs/OPDS-INTEROP.md.

use std::io::Read;
use std::path::Path;

use crate::atom::parse_atom;
use crate::model::{AuthDocument, Feed, Link, MediaType};
use crate::opds2::{parse_opds2, parse_opds2_publication};
use crate::OpdsError;

pub struct OpdsClient {
    agent: ureq::Agent,
    /// Precomputed `Authorization: Basic ...` header value.
    basic_auth: Option<String>,
}

impl Default for OpdsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl OpdsClient {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            // Read 4xx/5xx bodies ourselves: a 401 carries the
            // Authentication Document.
            .http_status_as_error(false)
            .build();
        OpdsClient {
            agent: config.into(),
            basic_auth: None,
        }
    }

    /// Set credentials for this catalog. Any request may 401 mid-flow; the
    /// caller prompts (using the Authentication Document when present) and
    /// retries after calling this.
    pub fn set_basic_auth(&mut self, username: &str, password: &str) {
        let raw = format!("{username}:{password}");
        self.basic_auth = Some(format!("Basic {}", base64(raw.as_bytes())));
    }

    /// Fetch and parse a catalog feed (or standalone entry/publication).
    ///
    /// One media type per Accept header — wild servers negotiate by naive
    /// substring match and mis-handle q-values (interop doc §1). The
    /// response is sniffed by Content-Type, not by what we asked for.
    pub fn fetch(&self, url: &str) -> Result<Feed, OpdsError> {
        let (body, content_type) = self.get(url, "application/atom+xml")?;
        parse_payload(&body, &content_type, url)
    }

    /// Explicitly probe the OPDS 2.0 encoding of a catalog.
    pub fn fetch_opds2(&self, url: &str) -> Result<Feed, OpdsError> {
        let (body, content_type) = self.get(url, "application/opds+json")?;
        parse_payload(&body, &content_type, url)
    }

    /// Run a search: prefer the feed's templated/OpenSearch machinery.
    /// 1.x: `rel=search` points at an OpenSearch description document whose
    /// template carries `{searchTerms}`. 2.0: the search href is templated
    /// directly.
    pub fn search(&self, feed: &Feed, base_url: &str, query: &str) -> Result<Feed, OpdsError> {
        let link = feed
            .search()
            .ok_or_else(|| OpdsError::Parse("feed has no search link".into()))?;
        let is_opensearch = link
            .media_type
            .as_ref()
            .is_some_and(|t| t.essence == "application/opensearchdescription+xml");
        let template = if is_opensearch {
            let (body, _) = self.get(&link.href, "application/opensearchdescription+xml")?;
            opensearch_template(&body, &link.href)?
        } else {
            link.href.clone()
        };
        let encoded = urlencode(query);
        let url = template.replace("{searchTerms}", &encoded);
        let _ = base_url; // hrefs were already resolved at parse time
        self.fetch(&url)
    }

    /// Download an acquisition to `dest`: temp file + atomic rename. No
    /// Range resume is assumed — an interrupted download restarts.
    pub fn download(&self, url: &str, dest: &Path) -> Result<(), OpdsError> {
        let mut request = self.agent.get(url);
        if let Some(auth) = &self.basic_auth {
            request = request.header("Authorization", auth);
        }
        let mut response = request
            .call()
            .map_err(|e| OpdsError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        if status == 401 {
            return Err(OpdsError::AuthRequired(None));
        }
        if !(200..300).contains(&status) {
            return Err(OpdsError::Http(status));
        }
        let tmp = dest.with_extension("part");
        {
            let mut file = std::fs::File::create(&tmp)
                .map_err(|e| OpdsError::Network(format!("create {}: {e}", tmp.display())))?;
            let mut reader = response.body_mut().as_reader();
            std::io::copy(&mut reader, &mut file)
                .map_err(|e| OpdsError::Network(format!("download: {e}")))?;
        }
        std::fs::rename(&tmp, dest)
            .map_err(|e| OpdsError::Network(format!("rename to {}: {e}", dest.display())))?;
        Ok(())
    }

    /// Fetch one comic page from a PSE stream link (0-based page number;
    /// `pse:lastRead` is 1-based — do not mix them up). Returns the bytes
    /// and the response Content-Type, which is authoritative over the
    /// link's advisory `type`.
    pub fn fetch_pse_page(
        &self,
        stream: &Link,
        page_number: u32,
        max_width: Option<u32>,
    ) -> Result<(Vec<u8>, Option<String>), OpdsError> {
        let url = pse_page_url(stream, page_number, max_width);
        self.get(&url, "image/*")
    }

    fn get(&self, url: &str, accept: &str) -> Result<(Vec<u8>, Option<String>), OpdsError> {
        let mut request = self.agent.get(url).header("Accept", accept);
        if let Some(auth) = &self.basic_auth {
            request = request.header("Authorization", auth);
        }
        let mut response = request
            .call()
            .map_err(|e| OpdsError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .read_to_end(&mut body)
            .map_err(|e| OpdsError::Network(format!("read body: {e}")))?;

        if status == 401 {
            let auth_doc = content_type
                .as_deref()
                .filter(|t| t.contains("opds-authentication"))
                .and_then(|_| serde_json::from_slice::<AuthDocument>(&body).ok());
            return Err(OpdsError::AuthRequired(auth_doc.map(Box::new)));
        }
        if !(200..300).contains(&status) {
            return Err(OpdsError::Http(status));
        }
        Ok((body, content_type))
    }
}

/// Substitute a PSE stream template. `{pageNumber}` is 0-based;
/// `{maxWidth}` is requested but servers may ignore it.
pub fn pse_page_url(stream: &Link, page_number: u32, max_width: Option<u32>) -> String {
    stream
        .href
        .replace("{pageNumber}", &page_number.to_string())
        .replace("{maxWidth}", &max_width.unwrap_or(1600).to_string())
}

fn parse_payload(body: &[u8], content_type: &Option<String>, url: &str) -> Result<Feed, OpdsError> {
    let essence = content_type
        .as_deref()
        .map(MediaType::parse)
        .map(|t| t.essence)
        .unwrap_or_default();
    if essence.contains("json") {
        // A publication document has `metadata` but no feed-shaped members;
        // try feed first, fall back to publication.
        parse_opds2(body, url).or_else(|_| parse_opds2_publication(body, url))
    } else if essence.contains("xml") || essence.is_empty() {
        parse_atom(body, url)
    } else {
        Err(OpdsError::Parse(format!(
            "unexpected content type {essence:?} for catalog at {url}"
        )))
    }
}

/// Extract the Atom-result template from an OpenSearch description document.
pub fn opensearch_template(xml: &[u8], base_url: &str) -> Result<String, OpdsError> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut fallback: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Empty(e)) | Ok(quick_xml::events::Event::Start(e))
                if e.local_name().as_ref() == "Url" =>
            {
                let mut template = None;
                let mut kind = None;
                for attr in e.attributes().flatten() {
                    let key = attr.key.local_name();
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    match key.as_ref() {
                        "template" => template = Some(value),
                        "type" => kind = Some(value),
                        _ => {}
                    }
                }
                if let Some(template) = template {
                    let resolved = crate::href::resolve_url(base_url, &template);
                    let is_atom = kind.as_deref().is_some_and(|k| k.contains("atom"));
                    if is_atom {
                        return Ok(resolved);
                    }
                    fallback.get_or_insert(resolved);
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(OpdsError::Parse(format!("opensearch: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    fallback.ok_or_else(|| OpdsError::Parse("opensearch document has no Url template".into()))
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
