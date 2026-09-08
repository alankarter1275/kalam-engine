//! Where the engine's diagnostics go, which is nowhere until a shell says.
//!
//! Everything chapbook reports about a degraded or failed operation — a
//! credential store that is locked, a library that would not open, a comic
//! page that failed to decode, a position that could not be saved — used to
//! go to `eprintln!`. On a desktop that is a terminal. On Android it is
//! `/dev/null` unless something redirects it, on iOS it is not where a
//! developer looks, and in a browser it does not exist. So the diagnostics
//! for exactly the failures a device shell hits were the ones nobody could
//! see, which is how the Android spike found this.
//!
//! The engine now uses the `log` crate, which was already in the dependency
//! graph via cosmic-text and fontdb, so gaining a voice cost nothing. A host
//! installs whatever backend it already has — `android_logger`, `oslog`,
//! `console_log`, `env_logger`, `tracing-log` — and chapbook's records
//! arrive with the emitting module as their target, so
//! `chapbook_reader` and `chapbook_library` can be filtered apart.
//!
//! # Levels, as chapbook uses them
//!
//! - `error` — something the reader asked for did not happen, or state was
//!   lost: a position that could not be saved, an annotation that could not
//!   be written, a page that failed to load.
//! - `warn` — degraded, but nothing was lost: no library could be opened,
//!   credentials are stored but locked right now, a cover was not kept.
//! - `info` — worth knowing and not a problem, such as which tier a
//!   restored reading position came back through.
//!
//! Nothing is logged per frame or per page turn; a session at rest is
//! silent.

use std::io::Write;

/// A logger that writes to stderr, for shells that want the old behaviour.
///
/// Provided because the alternative is every reference shell growing twenty
/// lines of the same thing, or taking `env_logger` for it. A device shell
/// should install its platform's backend instead of this.
struct Stderr {
    level: log::LevelFilter,
}

impl log::Log for Stderr {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // One line, prefixed by the crate that emitted it. Locking once
        // keeps a line from being interleaved with the loader thread's.
        let target = record.target().split("::").next().unwrap_or("chapbook");
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "{}: {}: {}",
            target.replace('_', "-"),
            record.level().as_str().to_ascii_lowercase(),
            record.args()
        );
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Send the engine's diagnostics to stderr at `warn` and above.
///
/// Call once, early. A second call is a no-op rather than an error, because
/// a shell should not have to know whether something else got there first.
pub fn log_to_stderr() {
    log_to_stderr_at(log::LevelFilter::Warn);
}

/// [`log_to_stderr`] with the threshold said out loud.
pub fn log_to_stderr_at(level: log::LevelFilter) {
    // A `'static` logger rather than a boxed one: `set_boxed_logger` lives
    // behind `log`'s `std` feature, and features are additive across a
    // workspace, so taking it here would turn it on for every crate that
    // shares this `log`. A `OnceLock` costs nothing and asks for nothing.
    static STDERR: std::sync::OnceLock<Stderr> = std::sync::OnceLock::new();
    let logger = STDERR.get_or_init(|| Stderr { level });
    // `set_logger` fails only when one is already installed, which is not a
    // condition worth reporting to a caller who just wants output.
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_becomes_a_crate_name_a_person_recognises() {
        // `log` targets are module paths; a reader of the output wants the
        // crate, and wants it spelled the way the crate is.
        let target = "chapbook_reader::loader";
        assert_eq!(
            target.split("::").next().unwrap().replace('_', "-"),
            "chapbook-reader"
        );
    }

    #[test]
    fn installing_twice_is_not_an_error() {
        // Two shells in one process, or a test harness that already
        // installed one. Neither should panic.
        log_to_stderr();
        log_to_stderr();
        log_to_stderr_at(log::LevelFilter::Debug);
    }
}
