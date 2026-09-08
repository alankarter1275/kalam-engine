//! **Internal to chapbook — no API stability.**
//!
//! The native half of the Android artifact: the smallest binding that gets
//! a real book onto a real screen, written against `chapbook-reader`
//! directly rather than against the C ABI — JNI is already a C ABI, and
//! stacking one on the other would be two boundaries back to back with
//! Rust in the middle converting both ways (`docs/STABILITY.md` has the
//! argument).
//!
//! It began as a spike, and the spike did its job: no font source, no
//! library directory, no way to draw into memory the host owns, no voice
//! on a platform with no terminal, nothing to call when the system asks
//! for memory back — every defect it surfaced is fixed in safe Rust
//! upstream of here, and this file was rewritten against the result.
//! Opening from a file descriptor, which is what an Android app actually
//! gets, is the one door it uses that a desktop never does.
//! `android/README.md` carries the platform notes.
//!
//! Off Android the crate is empty: the bindings call `libjnigraphics`,
//! which no other platform has, and a workspace build should not have to
//! care. That is also a hole in the test gate — nothing in `cargo test`
//! compiles this file, so `android/build-jni.sh` checks the two failures
//! that otherwise wait for a device.

#[cfg(target_os = "android")]
mod android;
