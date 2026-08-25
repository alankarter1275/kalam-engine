//! Run the shell conformance harness against a book and print the report.
//!
//!     cargo run -p chapbook-reader --example conform -- <book> [w h]
//!
//! The harness itself is `chapbook_reader::conformance`, meant to run in
//! a shell author's own test suite against their own `Session`. This is
//! the same thing from a terminal, for pointing at a book that is
//! misbehaving on a device and asking which rule it breaks.
//!
//! Exits 1 if any check failed, so it can gate a script.
//!
//! Note that this reads and writes the real library, because
//! `PositionSurvivesARestart` has to: it moves the saved reading position
//! for the book you point it at. Set `CHAPBOOK_LIBRARY_DIR` to a scratch
//! directory if that matters.

use std::time::Duration;

use chapbook_reader::chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::conformance::Harness;
use chapbook_reader::Session;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(source) = args.first().cloned() else {
        eprintln!("usage: conform <book.epub|comic.cbz|doc.pdf|opds-url> [width height]");
        std::process::exit(2);
    };
    let size = match args.get(1).zip(args.get(2)) {
        Some((w, h)) => Size::new(w.parse().unwrap(), h.parse().unwrap()),
        None => Size::new(600.0, 800.0),
    };

    let report =
        Harness::new(
            move || match Session::open(&source, chapbook_core::FontSource::host()) {
                Ok(session) => session,
                Err(e) => {
                    eprintln!("conform: {e}");
                    std::process::exit(1);
                }
            },
        )
        .metrics(PageMetrics {
            size,
            margins: EdgeSizes::uniform(32.0),
            dpi_scale: 1.0,
            rotation: Rotation::None,
        })
        .budget(Duration::from_secs(30))
        .run();

    print!("{report}");
    let failed = report.failures().count();
    if failed > 0 {
        eprintln!("\n{failed} check(s) failed");
        std::process::exit(1);
    }
}
