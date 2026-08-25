//! The `ureq` implementation of [`HttpClient`], behind the default `ureq`
//! feature.
//!
//! This is the desktop answer and nothing more. It brings `ureq`, `rustls`
//! and a bundled root store with it; a caller that wants the host's
//! networking instead turns the feature off and passes its own transport,
//! and nothing else in this crate changes.
//!
//! (The module is `ureq_transport`, not `ureq`, because a crate-root
//! `mod ureq` would make `use ureq::...` ambiguous with the extern crate.)

use std::io::Read;

use crate::http::{HttpClient, HttpError, HttpRequest, HttpResponse};

/// [`HttpClient`] over a blocking `ureq` agent.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqHttp {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            // Hand 4xx/5xx back as responses: the trait requires it, and a
            // 401 body is the Authentication Document.
            .http_status_as_error(false)
            .build();
        UreqHttp {
            agent: config.into(),
        }
    }

    /// Wrap an agent the caller configured — proxy, timeouts, a different
    /// TLS backend or root store. `http_status_as_error(false)` is required
    /// of it, per [`HttpClient`]'s contract.
    pub fn with_agent(agent: ureq::Agent) -> Self {
        UreqHttp { agent }
    }
}

impl HttpClient for UreqHttp {
    fn get(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut call = self.agent.get(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let response = call.call().map_err(HttpError::new)?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        // `into_body().into_reader()` rather than a borrowed reader: the
        // body outlives this function inside `HttpResponse`.
        let body: Box<dyn Read + Send> = Box::new(response.into_body().into_reader());
        Ok(HttpResponse {
            status,
            content_type,
            body,
        })
    }
}
