//! Win32 reference viewer — a Windows-only shell.
//!
//! The third toolkit shell, and the one that answers a question the other
//! two do not: what a `Session` looks like under a message loop that the
//! shell itself pumps. winit and GTK both own the loop and call back into
//! this repository; `GetMessageW`/`DispatchMessageW` is the shell's own
//! `while`, and everything a window does arrives as a `u32` in a callback
//! with no state attached. That is the shape a downstream Windows app
//! embedding chapbook is actually working in — under WinUI, WPF or MFC,
//! chapbook is a child HWND and somebody else's frame is around it — so
//! this file is written as the piece that would be lifted out.
//!
//! Off Windows the crate is this file and nothing else: no dependencies,
//! a stub `main`, and a workspace build that does not have to know the
//! difference.

#[cfg(windows)]
mod uia;
#[cfg(windows)]
mod win32;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    win32::run()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "chapbook-viewer-win32 is a Windows-only shell: it is written against \
         the Win32 message loop, which this target does not have. The winit \
         shell (chapbook-viewer) is portable."
    );
    std::process::ExitCode::from(2)
}
