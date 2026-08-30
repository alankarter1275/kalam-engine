//! GTK4 reference viewer — a Linux-only shell.
//!
//! GTK4 is reached through `gtk4-sys`, which probes for `gtk4.pc` with
//! `pkg-config`. That is a system library and a system tool, and neither is
//! present on a stock macOS or Windows box, so a workspace build there used
//! to fail in this crate before it ran a single test. The viewer therefore
//! compiles on Linux and nowhere else: `gtk4` is a target-gated dependency,
//! and this file is all that is left of the crate elsewhere.

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
        "chapbook-viewer-gtk is a Linux-only shell: it needs GTK4, which this \
         target does not build. The winit shell (chapbook-viewer) is portable."
    );
    std::process::ExitCode::from(2)
}
