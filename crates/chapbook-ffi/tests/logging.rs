//! The log sink, tested against the thing it exists for.
//!
//! The rung-3 finding on Android was not "there is no logging API" — it was
//! that **the engine's own diagnostics went nowhere**, on the one platform
//! where the failures they describe actually happen. So the test that
//! matters is not that a host can log through the ABI; it is that a record
//! emitted deep inside `chapbook-reader` arrives at a C callback with the
//! emitting crate's name attached.
//!
//! Its own file because `log` allows one logger per process and this
//! installs it. Sharing a binary with the other suites would make the
//! delivery assertions depend on test ordering — and within this file the
//! same hazard applies, since cargo runs tests in parallel threads of one
//! process, so they serialize behind `SINK_LOCK` the way the library tests
//! serialize behind their `ENV_LOCK`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::Mutex;

use chapbook_ffi::*;

static CAPTURED: Mutex<Vec<(i32, String, String)>> = Mutex::new(Vec::new());

/// The sink is one per process, so only one test may own it at a time.
static SINK_LOCK: Mutex<()> = Mutex::new(());

/// Take the sink, tolerating a previous test having panicked while holding
/// it — a poisoned lock here means another test failed, and reporting that
/// one as a lock error would bury it.
fn own_the_sink() -> std::sync::MutexGuard<'static, ()> {
    SINK_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

extern "C" fn capture(
    level: cb_log_level,
    target: *const c_char,
    message: *const c_char,
    user: *mut c_void,
) {
    // The `user` pointer must arrive back exactly as it was handed over.
    assert_eq!(user as usize, 0xC0FFEE, "user pointer round-trips");
    // SAFETY: the ABI documents both as NUL-terminated and valid for the
    // duration of this call, which is what a host relies on.
    let (target, message) = unsafe {
        (
            CStr::from_ptr(target).to_string_lossy().into_owned(),
            CStr::from_ptr(message).to_string_lossy().into_owned(),
        )
    };
    CAPTURED
        .lock()
        .expect("not poisoned")
        .push((level as i32, target, message));
}

fn install() {
    assert_eq!(
        unsafe {
            cb_set_log_callback(
                Some(capture),
                0xC0FFEE as *mut c_void,
                cb_log_level::CB_LOG_DEBUG,
            )
        },
        cb_status::CB_OK
    );
}

fn drain() -> Vec<(i32, String, String)> {
    std::mem::take(&mut *CAPTURED.lock().expect("not poisoned"))
}

#[test]
fn the_sink_carries_the_engines_own_diagnostics_and_not_just_the_hosts() {
    let _sink = own_the_sink();
    let _ = drain();
    install();

    // A host message, to prove the stream is shared and ordered.
    let target = CString::new("demo").unwrap();
    let message = CString::new("about to open something that will not open").unwrap();
    assert_eq!(
        unsafe { cb_log(cb_log_level::CB_LOG_INFO, target.as_ptr(), message.as_ptr()) },
        cb_status::CB_OK
    );
    assert!(cb_log_enabled());

    // Now make the *engine* complain. A library directory that cannot be
    // opened is a `warn!` from inside chapbook-reader, and is exactly the
    // class of message that used to vanish on a device: not fatal, easy to
    // miss, and the reason a reader silently stops remembering anything.
    let fonts = {
        let dir = CString::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/fonts")
                .to_string_lossy()
                .into_owned(),
        )
        .unwrap();
        let family = CString::new("Crimson Text").unwrap();
        unsafe { cb_font_source_embedded(dir.as_ptr(), family.as_ptr()) }
    };
    let config = unsafe { cb_config_new(fonts) };
    // A path, not a directory: opening a library there must fail. It has to
    // be a file that *exists*, and this crate's own manifest is the one such
    // path every host agrees on — `/etc/hostname` was here first, and on
    // Windows there is no such file, so `Library::open` cheerfully created
    // `C:\etc\hostname\`, warned about nothing, and left this test asserting
    // that a warning it never provoked had arrived.
    let not_a_dir = CString::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("Cargo.toml")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    unsafe { cb_config_set_library_dir(config, not_a_dir.as_ptr()) };
    let book = CString::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/epub/illustrated.epub")
            .to_string_lossy()
            .into_owned(),
    )
    .unwrap();
    let session = unsafe { cb_session_open_path(book.as_ptr(), config) };
    if !session.is_null() {
        unsafe { cb_session_close(session) };
    }

    let records = drain();
    assert!(
        records
            .iter()
            .any(|(_, target, message)| target == "demo" && message.contains("will not open")),
        "the host's own message came through: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|(_, target, _)| target.starts_with("chapbook")),
        "a record from inside the engine reached the sink, tagged with its \
         crate — this is the whole rung-3 finding: {records:?}"
    );
}

#[test]
fn clearing_the_sink_stops_delivery() {
    let _sink = own_the_sink();
    let _ = drain();
    install();
    assert_eq!(
        unsafe { cb_set_log_callback(None, std::ptr::null_mut(), cb_log_level::CB_LOG_OFF) },
        cb_status::CB_OK
    );
    assert!(!cb_log_enabled(), "no sink is installed");

    let target = CString::new("demo").unwrap();
    let message = CString::new("into the void").unwrap();
    // The `user` assertion inside `capture` would fire if this were
    // delivered after the clear, so silence here is load-bearing.
    assert_eq!(
        unsafe {
            cb_log(
                cb_log_level::CB_LOG_ERROR,
                target.as_ptr(),
                message.as_ptr(),
            )
        },
        cb_status::CB_OK
    );
    assert!(drain().is_empty(), "nothing is delivered after a clear");
}

#[test]
fn the_level_filter_is_honoured() {
    let _sink = own_the_sink();
    let _ = drain();
    assert_eq!(
        unsafe {
            cb_set_log_callback(
                Some(capture),
                0xC0FFEE as *mut c_void,
                cb_log_level::CB_LOG_WARN,
            )
        },
        cb_status::CB_OK
    );
    let target = CString::new("demo").unwrap();
    let below = CString::new("chatter").unwrap();
    let above = CString::new("trouble").unwrap();
    unsafe {
        cb_log(cb_log_level::CB_LOG_DEBUG, target.as_ptr(), below.as_ptr());
        cb_log(cb_log_level::CB_LOG_ERROR, target.as_ptr(), above.as_ptr());
    }
    let records = drain();
    assert!(
        records.iter().all(|(_, _, m)| m != "chatter"),
        "debug is below the warn threshold: {records:?}"
    );
    assert!(
        records.iter().any(|(_, _, m)| m == "trouble"),
        "error is above it: {records:?}"
    );
}

#[test]
fn a_null_message_is_a_code_and_not_a_crash() {
    let _sink = own_the_sink();
    let _ = drain();
    install();
    assert_eq!(
        unsafe {
            cb_log(
                cb_log_level::CB_LOG_INFO,
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        cb_status::CB_ERR_NULL_ARGUMENT
    );
}
