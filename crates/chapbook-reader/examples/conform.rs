//! Run the shell conformance harness against a book and print the report.
//!
//!     cargo run -p chapbook-reader --example conform -- <book> [w h] \
//!         [--fonts <dir> <family>]
//!
//! The harness itself is `chapbook_reader::conformance`, meant to run in
//! a shell author's own test suite against their own `Session`. This is
//! the same thing from a terminal, for pointing at a book that is
//! misbehaving on a device and asking which rule it breaks.
//!
//! Exits 1 if any check failed, so it can gate a script.
//!
//! kalam: nothing here writes anywhere (upstream's `--library <dir>` is
//! gone with the library). `--fonts` swaps the host's faces for a fixed
//! set, which is what makes a report mean the same thing on a platform
//! where the host has none.

use std::time::Duration;

use chapbook_reader::chapbook_core::{EdgeSizes, FontSource, PageMetrics, Rotation, Size};
use chapbook_reader::conformance::Harness;
use chapbook_reader::{Session, SessionConfig};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // `--fonts <dir> <family>`: a fixed source instead of the host's.
    let fonts = args.iter().position(|a| a == "--fonts").map(|at| {
        let mut it = args.drain(at..at + 3).skip(1);
        (
            it.next().expect("--fonts takes a dir"),
            it.next().expect("--fonts takes a family"),
        )
    });

    let Some(source) = args.first().cloned() else {
        eprintln!("usage: conform <book.epub> [width height] [--fonts <dir> <family>]");
        std::process::exit(2);
    };
    let size = match args.get(1).zip(args.get(2)) {
        Some((w, h)) => Size::new(w.parse().unwrap(), h.parse().unwrap()),
        None => Size::new(600.0, 800.0),
    };

    let report = Harness::new(move || {
        let source_fonts = match &fonts {
            Some((dir, family)) => FontSource::embedded(dir, family),
            None => FontSource::host(),
        };
        let config = SessionConfig::new(source_fonts);
        match Session::open_with(source.as_str(), config) {
            Ok(session) => session,
            Err(e) => {
                eprintln!("conform: {e}");
                std::process::exit(1);
            }
        }
    })
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
