//! Minimal reference viewer. Exists to exercise the pipeline end-to-end:
//! open an EPUB, paginate, render pages, turn pages with the keyboard,
//! persist the reading position. Not a polished product.
//!
//! Contract (see `Publication::unit_bytes`): book I/O may block for seconds
//! and fail with network errors — remote-backed publications are planned.
//! All `unit_bytes`/`resource` calls belong on a loader thread, never in the
//! winit event loop.
//!
//! Implemented in milestone M4 (rendering) and M7 (library integration).

fn main() {
    eprintln!("chapbook-viewer: not yet implemented (lands in milestone M4)");
    std::process::exit(1);
}
