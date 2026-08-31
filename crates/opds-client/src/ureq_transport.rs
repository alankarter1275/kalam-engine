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

#[cfg(feature = "write")]
use crate::http::HttpMethod;
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
        convert(call.call().map_err(HttpError::new)?)
    }

    #[cfg(feature = "write")]
    fn send(
        &self,
        method: HttpMethod,
        request: HttpRequest,
        body: Option<Vec<u8>>,
    ) -> Result<HttpResponse, HttpError> {
        // ureq 3 types the builder by whether the verb carries a body, so
        // there is no one generic `request(method, url)` to call and the
        // match is the API, not a style choice.
        let response = match method {
            HttpMethod::Post => {
                let mut call = self.agent.post(&request.url);
                for (name, value) in &request.headers {
                    call = call.header(name, value);
                }
                match body {
                    Some(body) => call.send(&body[..]),
                    None => call.send_empty(),
                }
            }
            HttpMethod::Put => {
                let mut call = self.agent.put(&request.url);
                for (name, value) in &request.headers {
                    call = call.header(name, value);
                }
                match body {
                    Some(body) => call.send(&body[..]),
                    None => call.send_empty(),
                }
            }
            HttpMethod::Delete => {
                let mut call = self.agent.delete(&request.url);
                for (name, value) in &request.headers {
                    call = call.header(name, value);
                }
                call.call()
            }
        };
        convert(response.map_err(HttpError::new)?)
    }
}

/// One ureq response to the trait's shape.
fn convert(response: ureq::http::Response<ureq::Body>) -> Result<HttpResponse, HttpError> {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let headers = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != "content-type")
        .filter_map(|(name, value)| {
            // A header whose bytes are not text is one this crate has no
            // way to use; dropping it beats a lossy conversion.
            Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
        })
        .collect();
    // `into_body().into_reader()` rather than a borrowed reader: the body
    // outlives this function inside `HttpResponse`.
    let body: Box<dyn Read + Send> = Box::new(response.into_body().into_reader());
    Ok(HttpResponse {
        status,
        content_type,
        headers,
        body,
    })
}
