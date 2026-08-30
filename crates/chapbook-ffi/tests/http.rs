//! The host-transport seam, driven through the C ABI the way a shell
//! drives it: callbacks installed with `cb_config_set_http_transport`, a
//! catalog opened with `cb_session_open_url`, and no socket anywhere.
//!
//! This is the FFI's half of the bring-your-own-HTTP promise. The Rust
//! half — that an injected `HttpClient` carries the whole OPDS flow — is
//! `opds-client`'s `injected_transport.rs`; what is being proven here is
//! the crossing itself: requests marshal out with their headers, the
//! response builder marshals bytes back in, failure text reaches
//! `cb_last_error_message`, and the finalizer runs exactly once no matter
//! which path consumed the transport.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chapbook_ffi::*;

const HOST: &str = "https://shelf.example.com";

fn lazy_feed() -> Vec<u8> {
    format!(
        r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:cat:comics</id><title>Comics</title>
  <entry><id>urn:c1</id><title>Test Comic</title>
    <link rel="alternate" href="{HOST}/entry" type="application/atom+xml;type=entry;profile=opds-catalog"/>
  </entry>
</feed>"#
    )
    .into_bytes()
}

fn complete_entry() -> Vec<u8> {
    format!(
        r#"<?xml version="1.0"?>
<entry xmlns="http://www.w3.org/2005/Atom" xmlns:pse="http://vaemendis.net/opds-pse/ns">
  <id>urn:c1</id><title>Test Comic</title>
  <link rel="http://vaemendis.net/opds-pse/stream"
        href="{HOST}/pages?page={{pageNumber}}&amp;width={{maxWidth}}"
        type="image/jpeg" pse:count="3"/>
</entry>"#
    )
    .into_bytes()
}

/// What the test observes from outside: requests that crossed the seam,
/// and whether the host's context was released.
#[derive(Default)]
struct Observed {
    requests: AtomicUsize,
    finalized: AtomicUsize,
}

/// The `user` pointer's referent — dropped by the finalizer, so the drop
/// count *is* the finalize count.
struct HostContext {
    observed: Arc<Observed>,
}

impl Drop for HostContext {
    fn drop(&mut self) {
        self.observed.finalized.fetch_add(1, Ordering::SeqCst);
    }
}

unsafe extern "C" fn finalize(user: *mut c_void) {
    drop(unsafe { Box::from_raw(user as *mut HostContext) });
}

/// A canned catalog server, as the get callback a host would write.
unsafe extern "C" fn serve(
    request: *const cb_http_request,
    response: *mut cb_http_response,
    user: *mut c_void,
) {
    let context = unsafe { &*(user as *const HostContext) };
    context.observed.requests.fetch_add(1, Ordering::SeqCst);

    let url = unsafe { CStr::from_ptr((*request).url) }.to_str().unwrap();
    let path = url.strip_prefix(HOST).unwrap_or("/");
    let (status, content_type, body): (u16, &str, Vec<u8>) = if path.starts_with("/feed") {
        (
            200,
            "application/atom+xml;profile=opds-catalog",
            lazy_feed(),
        )
    } else if path.starts_with("/entry") {
        (200, "application/atom+xml;type=entry", complete_entry())
    } else if path.starts_with("/pages") {
        let page = path
            .split("page=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .unwrap_or("?");
        (200, "image/jpeg", format!("JPEGDATA:{page}").into_bytes())
    } else {
        (404, "text/plain", b"not found".to_vec())
    };

    let content_type = CString::new(content_type).unwrap();
    unsafe {
        assert_eq!(
            cb_http_response_set_status(response, status),
            cb_status::CB_OK
        );
        assert_eq!(
            cb_http_response_set_content_type(response, content_type.as_ptr()),
            cb_status::CB_OK
        );
        // Two calls, to prove chunked appends concatenate.
        let (head, tail) = body.split_at(body.len() / 2);
        assert_eq!(
            cb_http_response_append_body(response, head.as_ptr(), head.len()),
            cb_status::CB_OK
        );
        assert_eq!(
            cb_http_response_append_body(response, tail.as_ptr(), tail.len()),
            cb_status::CB_OK
        );
    }
}

/// A transport whose network is down, reporting the way the header says.
unsafe extern "C" fn refuse(
    _request: *const cb_http_request,
    response: *mut cb_http_response,
    _user: *mut c_void,
) {
    let message = CString::new("the cable is unplugged").unwrap();
    unsafe {
        assert_eq!(
            cb_http_response_fail(response, message.as_ptr()),
            cb_status::CB_OK
        );
    }
}

/// A buggy transport that reports nothing at all.
unsafe extern "C" fn shrug(
    _request: *const cb_http_request,
    _response: *mut cb_http_response,
    _user: *mut c_void,
) {
}

fn cstr(value: &str) -> CString {
    CString::new(value).expect("no interior NUL")
}

fn fonts() -> *mut cb_font_source {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fonts");
    let dir = cstr(&dir.to_string_lossy());
    let family = cstr("Crimson Text");
    let fonts = unsafe { cb_font_source_embedded(dir.as_ptr(), family.as_ptr()) };
    assert!(!fonts.is_null());
    fonts
}

/// A config with its own library dir — the page stream requires one — and
/// the given callbacks installed over a fresh [`HostContext`].
fn config_with_transport(
    name: &str,
    get: cb_http_get_fn,
    observed: &Arc<Observed>,
) -> *mut cb_config {
    let config = unsafe { cb_config_new(fonts()) };
    assert!(!config.is_null());
    let dir = std::env::temp_dir().join(format!("chapbook-ffi-http-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("library dir is creatable");
    let dir = cstr(&dir.to_string_lossy());
    assert_eq!(
        unsafe { cb_config_set_library_dir(config, dir.as_ptr()) },
        cb_status::CB_OK
    );

    let context = Box::new(HostContext {
        observed: Arc::clone(observed),
    });
    assert_eq!(
        unsafe {
            cb_config_set_http_transport(
                config,
                get,
                None,
                Some(finalize),
                Box::into_raw(context) as *mut c_void,
            )
        },
        cb_status::CB_OK
    );
    config
}

fn last_error() -> String {
    let mut needed: usize = 0;
    unsafe { cb_last_error_message(std::ptr::null_mut(), 0, &mut needed) };
    let mut buf = vec![0u8; needed.max(1)];
    let rc =
        unsafe { cb_last_error_message(buf.as_mut_ptr() as *mut c_char, buf.len(), &mut needed) };
    assert_eq!(rc, cb_status::CB_OK);
    buf.pop();
    String::from_utf8(buf).expect("UTF-8 out")
}

#[test]
fn a_streamed_comic_opens_through_an_injected_transport() {
    let observed = Arc::new(Observed::default());
    let config = config_with_transport("opens", Some(serve), &observed);

    let url = cstr(&format!("{HOST}/feed"));
    let session = unsafe { cb_session_open_url(url.as_ptr(), config) };
    assert!(!session.is_null(), "open failed: {}", last_error());

    // The feed and its complete entry both crossed the host's callback.
    assert!(observed.requests.load(Ordering::SeqCst) >= 2);

    let mut needed: usize = 0;
    unsafe { cb_session_title(session, std::ptr::null_mut(), 0, &mut needed) };
    let mut buf = vec![0u8; needed];
    assert_eq!(
        unsafe {
            cb_session_title(
                session,
                buf.as_mut_ptr() as *mut c_char,
                buf.len(),
                &mut needed,
            )
        },
        cb_status::CB_OK
    );
    buf.pop();
    assert_eq!(String::from_utf8(buf).unwrap(), "Test Comic");

    let mut spine = 0usize;
    assert_eq!(
        unsafe { cb_session_spine_len(session, &mut spine) },
        cb_status::CB_OK
    );
    assert_eq!(spine, 3, "pse:count crossed intact");

    assert_eq!(
        observed.finalized.load(Ordering::SeqCst),
        0,
        "the transport outlives the open — sessions keep fetching pages"
    );
    unsafe { cb_session_close(session) };
    assert_eq!(
        observed.finalized.load(Ordering::SeqCst),
        1,
        "closing the last holder releases the host context, once"
    );
}

#[test]
fn a_transport_failure_reaches_the_caller_as_its_own_sentence() {
    let observed = Arc::new(Observed::default());
    let config = config_with_transport("refused", Some(refuse), &observed);

    let url = cstr(&format!("{HOST}/feed"));
    let session = unsafe { cb_session_open_url(url.as_ptr(), config) };
    assert!(session.is_null());
    assert!(
        last_error().contains("the cable is unplugged"),
        "the host's message should survive the crossing: {}",
        last_error()
    );
    // The failed open consumed the config, and the config the transport.
    assert_eq!(observed.finalized.load(Ordering::SeqCst), 1);
}

#[test]
fn a_transport_that_reports_nothing_is_named_as_the_bug() {
    let observed = Arc::new(Observed::default());
    let config = config_with_transport("silent", Some(shrug), &observed);

    let url = cstr(&format!("{HOST}/feed"));
    let session = unsafe { cb_session_open_url(url.as_ptr(), config) };
    assert!(session.is_null());
    assert!(
        last_error().contains("without reporting a status or a failure"),
        "got: {}",
        last_error()
    );
}

#[test]
fn a_declined_install_still_runs_the_finalizer() {
    // The ownership rule — "the transport owns `user` from this call on" —
    // must hold on the failure paths too, or a host leaks its context
    // exactly when things are already going wrong.
    let observed = Arc::new(Observed::default());
    let context = Box::new(HostContext {
        observed: Arc::clone(&observed),
    });
    let rc = unsafe {
        cb_config_set_http_transport(
            std::ptr::null_mut(),
            Some(serve),
            None,
            Some(finalize),
            Box::into_raw(context) as *mut c_void,
        )
    };
    assert_eq!(rc, cb_status::CB_ERR_NULL_ARGUMENT);
    assert_eq!(observed.finalized.load(Ordering::SeqCst), 1);

    // And a null get callback declines the same way.
    let observed = Arc::new(Observed::default());
    let config = unsafe { cb_config_new(fonts()) };
    let context = Box::new(HostContext {
        observed: Arc::clone(&observed),
    });
    let rc = unsafe {
        cb_config_set_http_transport(
            config,
            None,
            None,
            Some(finalize),
            Box::into_raw(context) as *mut c_void,
        )
    };
    assert_eq!(rc, cb_status::CB_ERR_NULL_ARGUMENT);
    assert_eq!(observed.finalized.load(Ordering::SeqCst), 1);
    unsafe { cb_config_free(config) };
}

#[test]
fn a_url_that_is_not_one_is_refused_before_the_network() {
    let observed = Arc::new(Observed::default());
    let config = config_with_transport("not-a-url", Some(serve), &observed);

    let url = cstr("ftp://shelf.example.com/feed");
    let session = unsafe { cb_session_open_url(url.as_ptr(), config) };
    assert!(session.is_null());
    assert!(
        last_error().contains("http:// or https://"),
        "{}",
        last_error()
    );
    assert_eq!(observed.requests.load(Ordering::SeqCst), 0);
    // Consumed-either-way applies here too.
    assert_eq!(observed.finalized.load(Ordering::SeqCst), 1);
}
