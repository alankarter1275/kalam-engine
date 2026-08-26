//! **Internal to chapbook — no API stability.** A spike, deliberately.
//!
//! This is `docs/FFI.md`'s Android ladder: the smallest binding that gets a
//! real book onto a real screen, written against `chapbook-reader` directly
//! rather than against the C ABI, because the point of the exercise is to
//! find out what the C ABI must carry before it is written down and frozen
//! at the Contract tier.
//!
//! Rungs 1 through 4 produced a list of API defects — no font source, no
//! library directory, no way to draw into memory the host owns, no voice on
//! a platform with no terminal, nothing to call when the system asks for
//! memory back. All of them are now fixed in safe Rust, upstream of here,
//! and this file was rewritten against the result. Rung 5 opens a book from
//! a file descriptor, which is what an Android app actually gets.
//!
//! Everything here is still expected to be deleted or rewritten. What is
//! meant to survive is what it turned up, which lives in `docs/FFI.md`.
//!
//! Off Android the crate is empty: the bindings call `libjnigraphics`,
//! which no other platform has, and a workspace build should not have to
//! care. That is also a hole in the test gate — nothing in `cargo test`
//! compiles this file, so `android/build-jni.sh` checks the two failures
//! that otherwise wait for a device.

#[cfg(target_os = "android")]
mod android;
