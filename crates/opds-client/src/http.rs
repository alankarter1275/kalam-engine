//! The transport seam: everything this crate knows about *how* bytes are
//! fetched, which is deliberately almost nothing.
//!
//! OPDS is a format, not a networking stack. Bundling one costs more than it
//! saves as soon as the caller is not a desktop process: an iOS app that
//! reaches the network outside `URLSession` gives up background transfer,
//! the system trust store, App Transport Security, per-app VPN and the
//! cellular-data toggle; an Android app gives up WorkManager; a WASM build
//! has no sockets at all and must use `fetch`. So the caller supplies an
//! [`HttpClient`] and the crate supplies the OPDS.
//!
//! `UreqHttp` is one implementation, behind the default `ureq`
//! feature. Turning that feature off drops `ureq`, `rustls` and the bundled
//! root store entirely, and the crate still does everything except open a
//! socket.

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

/// One outgoing request. GET is the only method OPDS browsing needs.
///
/// Owned rather than borrowed on purpose: an implementation is as likely to
/// be a thin shim over a foreign runtime — `URLSession`, OkHttp, `fetch` —
/// as it is to be Rust all the way down, and lifetimes do not survive that
/// trip.
pub struct HttpRequest {
    pub url: String,
    /// Header name/value pairs, already assembled. Send them as given:
    /// `Accept` carries exactly one media type and never a q-value, because
    /// real catalog servers negotiate by naive substring match (interop doc
    /// §1). Rewriting or merging headers will break servers in the wild.
    pub headers: Vec<(String, String)>,
}

impl HttpRequest {
    pub fn new(url: impl Into<String>) -> Self {
        HttpRequest {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// One response, with its body still unread.
pub struct HttpResponse {
    /// The real status, including 4xx and 5xx — see [`HttpClient::get`].
    pub status: u16,
    /// The `Content-Type` header verbatim, parameters and all. This is
    /// authoritative over whatever the caller asked for or a link advertised.
    pub content_type: Option<String>,
    pub body: Box<dyn Read + Send>,
}

/// A transport failure: the request never produced a response. A server that
/// answered with 404 or 500 did *not* fail — that is an [`HttpResponse`].
#[derive(Debug)]
pub struct HttpError(String);

impl HttpError {
    pub fn new(message: impl fmt::Display) -> Self {
        HttpError(message.to_string())
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

/// A blocking HTTP transport.
///
/// Blocking by contract, not by accident: OPDS page streaming backs a
/// `Publication` whose reads happen on a loader thread, and an async
/// runtime here would infect that. Implementations that wrap an async or
/// callback-driven stack should block internally.
///
/// `Send + Sync` because a streamed comic is shared across threads.
///
/// ## What an implementation must do
///
/// - **Return 4xx and 5xx as responses, not errors.** A 401's body is the
///   OPDS Authentication Document, which is what lets a shell put up a
///   native login dialog instead of a generic failure. A transport that
///   collapses error statuses makes that impossible.
/// - **Follow redirects, including cross-host ones.** Catalogs relocate
///   acquisitions onto CDNs.
/// - **Send the given headers unaltered**, per [`HttpRequest::headers`].
/// - **Not retry on its own.** Auth retry is the caller's flow.
///
/// ## What belongs here rather than in the credential
///
/// [`OpdsClient::set_authorization`](crate::OpdsClient::set_authorization)
/// covers every scheme that reduces to one constant header — Basic, bearer
/// tokens, per-user API keys. The two that do not are **per-request
/// signing**, where the value depends on the method, path or body, and
/// **cookie sessions**, where the state is a jar rather than a header. Both
/// need to see the whole request, so both are this trait's job, not the
/// credential store's. Keeping that line means the credential stays opaque
/// bytes all the way out to a host.
pub trait HttpClient: Send + Sync {
    fn get(&self, request: HttpRequest) -> Result<HttpResponse, HttpError>;

    /// Fetch straight to a file, returning the HTTP status.
    ///
    /// The default streams [`get`](HttpClient::get)'s body through a
    /// sibling temp file and renames it into place, so a reader never sees
    /// a half-written book; on a non-2xx status it writes nothing and just
    /// reports the status.
    ///
    /// Override it when the host owns a download facility worth having —
    /// an iOS background `URLSession` or Android's `WorkManager` continue a
    /// transfer after the process is suspended, which no amount of Rust can
    /// reproduce. An override must keep the same promise: `dest` either
    /// ends up complete or is not created.
    fn download(&self, request: HttpRequest, dest: &Path) -> Result<u16, HttpError> {
        let mut response = self.get(request)?;
        if !(200..300).contains(&response.status) {
            return Ok(response.status);
        }
        let tmp = dest.with_extension("part");
        let copy = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&tmp)?;
            std::io::copy(&mut response.body, &mut file)?;
            file.sync_all()
        })();
        if let Err(e) = copy {
            let _ = std::fs::remove_file(&tmp);
            return Err(HttpError::new(format!("write {}: {e}", tmp.display())));
        }
        std::fs::rename(&tmp, dest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            HttpError::new(format!("rename to {}: {e}", dest.display()))
        })?;
        Ok(response.status)
    }
}

/// A shared transport is a transport.
///
/// The reason this exists rather than being an inconvenience the caller
/// works around: a host that owns its networking owns *one* of it. An iOS
/// app has a single background `URLSession` whose whole value is that
/// transfers outlive the process; handing out clones of it is wrong and
/// handing out a second one is worse. Meanwhile chapbook builds a fresh
/// `OpdsClient` per authentication attempt, so without this the retry path
/// would have to construct a second transport to re-send one request.
impl<T: HttpClient + ?Sized> HttpClient for Arc<T> {
    fn get(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        (**self).get(request)
    }

    fn download(&self, request: HttpRequest, dest: &Path) -> Result<u16, HttpError> {
        (**self).download(request, dest)
    }
}
