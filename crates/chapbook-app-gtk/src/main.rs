//! The chapbook desktop application — a Linux-only GTK4 shell.
//!
//! Every decision that is not a widget lives in `chapbook-app`; this crate
//! is GTK plumbing over that model. GTK4 is reached through `gtk4-sys`,
//! which probes for `gtk4.pc` with `pkg-config` — a system library and a
//! system tool — so like the reference viewer this compiles on Linux and
//! nowhere else: `gtk4` is a target-gated dependency, and this file is all
//! that is left of the crate elsewhere.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod page_area;

#[cfg(target_os = "linux")]
fn main() -> gtk4::glib::ExitCode {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "chapbook-app-gtk is a Linux-only application: it needs GTK4, which \
         this target does not build."
    );
    std::process::ExitCode::from(2)
}
