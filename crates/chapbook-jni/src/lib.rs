//! **Internal to chapbook — no API stability.** A spike, deliberately.
//!
//! This is rung 2 of `docs/FFI.md`'s Android ladder: the smallest binding
//! that gets a real book onto a real screen, written against
//! `chapbook-reader` directly rather than against the C ABI, because the
//! point of the exercise is to find out what the C ABI must carry before
//! it is written down and frozen at the Contract tier.
//!
//! Everything here is expected to be deleted or rewritten. What is meant
//! to survive is the list of defects it turns up, which lives in
//! `docs/FFI.md`.
//!
//! Off Android the crate is empty: the bindings call `libjnigraphics`,
//! which no other platform has, and a workspace build should not have to
//! care.

#[cfg(target_os = "android")]
mod android;
