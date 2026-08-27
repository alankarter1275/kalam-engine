//! The log sink.
//!
//! The engine reports through the `log` crate and is silent until somebody
//! installs a backend. A Rust host installs the one it already has —
//! `android_logger`, `oslog`, `env_logger`. A C or Swift host has no such
//! option, and without this it gets exactly what the Android spike got
//! before the seam existed: every diagnostic about a degraded or failed
//! operation going nowhere, on the one platform where those failures
//! actually happen.
//!
//! So: one callback, installed once, replaceable, and clearable.
//!
//! The callback pointer lives in an atomic rather than behind the
//! `OnceLock` that `chapbook_core::log_to_stderr_at` uses, because
//! `log::set_logger` may only be called once per process and a host that
//! wants to change its sink — or drop it before unloading the library —
//! would otherwise be stuck with the first one forever.

use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::{cb_status, guard};

/// Severity, matching `log`'s own ordering so the numbers are not a second
/// thing to remember.
///
/// As chapbook uses them: `ERROR` is something the reader asked for that
/// did not happen, or state that was lost. `WARN` is degraded but nothing
/// lost. `INFO` is worth knowing and not a problem. Nothing is logged per
/// frame or per page turn — a session at rest is silent.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cb_log_level {
    CB_LOG_OFF = 0,
    CB_LOG_ERROR = 1,
    CB_LOG_WARN = 2,
    CB_LOG_INFO = 3,
    CB_LOG_DEBUG = 4,
    CB_LOG_TRACE = 5,
}

impl cb_log_level {
    fn filter(self) -> log::LevelFilter {
        match self {
            cb_log_level::CB_LOG_OFF => log::LevelFilter::Off,
            cb_log_level::CB_LOG_ERROR => log::LevelFilter::Error,
            cb_log_level::CB_LOG_WARN => log::LevelFilter::Warn,
            cb_log_level::CB_LOG_INFO => log::LevelFilter::Info,
            cb_log_level::CB_LOG_DEBUG => log::LevelFilter::Debug,
            cb_log_level::CB_LOG_TRACE => log::LevelFilter::Trace,
        }
    }

    fn of(level: log::Level) -> cb_log_level {
        match level {
            log::Level::Error => cb_log_level::CB_LOG_ERROR,
            log::Level::Warn => cb_log_level::CB_LOG_WARN,
            log::Level::Info => cb_log_level::CB_LOG_INFO,
            log::Level::Debug => cb_log_level::CB_LOG_DEBUG,
            log::Level::Trace => cb_log_level::CB_LOG_TRACE,
        }
    }
}

/// Receives one diagnostic.
///
/// `target` is the crate that emitted it — `chapbook_reader`,
/// `chapbook_library` — so a host can route or filter by subsystem. Both
/// strings are NUL-terminated, valid only for the duration of the call, and
/// must be copied if kept.
///
/// **It may fire on any thread**, including the loader thread the host has
/// never seen. It must not call back into this ABI, and on a platform where
/// logging touches UI it must hand the message to whatever the main loop
/// watches rather than doing the work inline.
pub type cb_log_fn = Option<
    extern "C" fn(
        level: cb_log_level,
        target: *const c_char,
        message: *const c_char,
        user: *mut c_void,
    ),
>;

/// The installed callback, as a raw address. Zero means none.
static LOG_FN: AtomicUsize = AtomicUsize::new(0);
/// The opaque pointer handed back to it.
static LOG_USER: AtomicUsize = AtomicUsize::new(0);

struct Callback;

impl log::Log for Callback {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        LOG_FN.load(Ordering::Acquire) != 0 && metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        let raw = LOG_FN.load(Ordering::Acquire);
        if raw == 0 || record.level() > log::max_level() {
            return;
        }
        // SAFETY: only ever stored from `cb_set_log_callback`, which takes
        // it as this exact function type.
        let callback: extern "C" fn(cb_log_level, *const c_char, *const c_char, *mut c_void) =
            unsafe { std::mem::transmute(raw) };
        let user = LOG_USER.load(Ordering::Acquire) as *mut c_void;

        // The crate, spelled the way the crate is — `chapbook_reader`
        // rather than the full module path, which is what a person
        // filtering logcat or Console actually types.
        let target = record.target().split("::").next().unwrap_or("chapbook");
        // A message with an interior NUL would be silently truncated on the
        // host side, which is worse than mangling the one byte.
        let Ok(target) = CString::new(target) else {
            return;
        };
        let message = CString::new(record.args().to_string().replace('\0', "?"))
            .unwrap_or_else(|_| CString::new("message could not cross as a C string").unwrap());

        callback(
            cb_log_level::of(record.level()),
            target.as_ptr(),
            message.as_ptr(),
            user,
        );
    }

    fn flush(&self) {}
}

/// Send the engine's diagnostics to `callback` at `max_level` and above.
///
/// Pass a null `callback` to stop delivery; the ABI keeps working and goes
/// quiet. `user` is stored and handed back untouched, and must stay valid
/// until the callback is replaced or cleared.
///
/// Call it before opening anything — the failures most worth seeing are the
/// ones during `cb_session_open_*`.
///
/// Only one sink exists per process, because `log` allows one logger per
/// process. Calling this again replaces it rather than adding a second.
/// Nothing is logged per frame or per page turn, so this is not on a hot
/// path.
#[no_mangle]
pub unsafe extern "C" fn cb_set_log_callback(
    callback: cb_log_fn,
    user: *mut c_void,
    max_level: cb_log_level,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let Some(callback) = callback else {
            // Clearing: stop delivery first, so no in-flight record can
            // reach a sink the host is about to invalidate.
            LOG_FN.store(0, Ordering::Release);
            LOG_USER.store(0, Ordering::Release);
            log::set_max_level(log::LevelFilter::Off);
            return cb_status::CB_OK;
        };
        // User first, so the callback never observes a stale one.
        LOG_USER.store(user as usize, Ordering::Release);
        LOG_FN.store(callback as usize, Ordering::Release);
        log::set_max_level(max_level.filter());

        // `set_logger` succeeds once per process and fails ever after. A
        // second call here is a host replacing its sink, which the atomics
        // above have already done, so the failure is not one.
        static LOGGER: Callback = Callback;
        let _ = log::set_logger(&LOGGER);
        cb_status::CB_OK
    })
}

/// Emit a diagnostic through whatever sink is installed.
///
/// Here so a host can put its own messages in the same stream as the
/// engine's, in the same order, without maintaining a second path — which
/// is the whole reason interleaved logs are worth having. `target` may be
/// null, and defaults to `host`.
#[no_mangle]
pub unsafe extern "C" fn cb_log(
    level: cb_log_level,
    target: *const c_char,
    message: *const c_char,
) -> cb_status {
    guard(cb_status::CB_ERR_PANIC, || {
        let level = match level {
            cb_log_level::CB_LOG_OFF => return cb_status::CB_OK,
            cb_log_level::CB_LOG_ERROR => log::Level::Error,
            cb_log_level::CB_LOG_WARN => log::Level::Warn,
            cb_log_level::CB_LOG_INFO => log::Level::Info,
            cb_log_level::CB_LOG_DEBUG => log::Level::Debug,
            cb_log_level::CB_LOG_TRACE => log::Level::Trace,
        };
        // SAFETY: the header's contract for both pointers.
        let Some(message) = (unsafe { crate::abi::str_in(message, "message") }) else {
            return cb_status::CB_ERR_NULL_ARGUMENT;
        };
        let target = if target.is_null() {
            "host"
        } else {
            // SAFETY: as above.
            match unsafe { crate::abi::str_in(target, "target") } {
                Some(target) => target,
                None => return cb_status::CB_ERR_INVALID_UTF8,
            }
        };
        log::logger().log(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{message}"))
                .build(),
        );
        cb_status::CB_OK
    })
}

/// Whether a sink is installed. Cheap; for a host deciding whether to
/// bother formatting something expensive.
#[no_mangle]
pub extern "C" fn cb_log_enabled() -> bool {
    guard(false, || LOG_FN.load(Ordering::Acquire) != 0)
}
