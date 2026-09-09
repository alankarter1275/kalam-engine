//! A window with nothing in it but a `ReaderView` — the widget as Kalam
//! will see it, without Kalam. For checking speed, memory, and that the
//! callbacks say the right things, on a real book.
//!
//! ```sh
//! cargo build --release -j 2 -p kalam-reader-demo
//! ./target/release/kalam-reader-demo book.epub
//! ```
//!
//! Keys — the widget's own: arrows / PageUp / PageDown / space turn pages,
//! `n` / `p` skip chapters, `b` goes back after a link, Escape clears a
//! selection. This window's: `t` cycles Kalam's four themes, `+`/`-`
//! change the font size, `[`/`]` the line height, `{`/`}` the column
//! width, `h` highlights the selection, `r` reloads every highlight from
//! this program's stand-in for Kalam's table, `x` removes the newest,
//! `q` quits. Mouse: drag to select, tap a word to "look it up"
//! (printed), tap a highlight to name it, tap the left or right third to
//! turn.
//!
//! Highlights work the way they will in Kalam: the widget captures one,
//! *this program* stores it (in a `Vec`, where Kalam has a table) and
//! hands the widget back its own id to paint under.
//!
//! Everything the widget reports goes to stderr, prefixed `demo:`, so a
//! run doubles as a trace of what Kalam would receive. The engine's own
//! timing lines (`chapbook-reader: info: laid out unit …`) are switched
//! on too, and the process's memory is printed after the first page and
//! at close, so one run is a complete performance report.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use kalam_reader::{HighlightColor, KalamPrefs, NewHighlight, ReaderOptions, ReaderView};

/// Kalam's `annotations` table, stood in for: rows keyed by an id this
/// program hands out. The widget never sees this; it sees ids.
type Shelf = Rc<RefCell<Vec<(i64, NewHighlight)>>>;

fn main() -> glib::ExitCode {
    // `info` rather than the default `warn`: the engine's timing lines
    // are the point of this program.
    chapbook_core::log_to_stderr_at(log::LevelFilter::Info);
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: kalam-reader-demo <book.epub>");
        return glib::ExitCode::from(2);
    };
    let host_fonts = std::env::args().any(|a| a == "--host-fonts");

    let app = gtk::Application::builder()
        .application_id("io.github.alankarter1275.KalamReaderDemo")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        let started = std::time::Instant::now();
        let options = ReaderOptions {
            host_fonts,
            ..ReaderOptions::default()
        };
        let view = match ReaderView::open(&path, KalamPrefs::default(), &options) {
            Ok(view) => view,
            Err(e) => {
                eprintln!("kalam-reader-demo: {e}");
                app.quit();
                return;
            }
        };
        eprintln!(
            "demo: opened \"{}\" ({} chapters) in {:?}; {}",
            view.title(),
            view.chapter_count(),
            started.elapsed(),
            memory_line(&view)
        );
        build_window(app, view, started);
    });
    app.run_with_args::<&str>(&[])
}

fn build_window(app: &gtk::Application, view: ReaderView, started: std::time::Instant) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(format!("{} — kalam-reader-demo", view.title()))
        .default_width(960)
        .default_height(800)
        .build();
    window.set_child(Some(view.widget()));

    // ---- What the widget reports ----
    {
        let first = Rc::new(Cell::new(true));
        let window = window.downgrade();
        let view_for_memory = view.clone();
        view.connect_position(move |pos| {
            if first.replace(false) {
                eprintln!(
                    "demo: first page on screen {:?} after launch; {}",
                    started.elapsed(),
                    memory_line(&view_for_memory)
                );
            }
            eprintln!(
                "demo: position chapter {}/{} page {}/{} fraction {:.3}",
                pos.chapter + 1,
                pos.chapter_count,
                pos.page + 1,
                pos.page_count,
                pos.fraction
            );
            if let Some(window) = window.upgrade() {
                window.set_title(Some(&format!(
                    "ch {}/{} · p {}/{} — kalam-reader-demo",
                    pos.chapter + 1,
                    pos.chapter_count,
                    pos.page + 1,
                    pos.page_count
                )));
            }
        });
    }
    view.connect_word(|word| match word.highlight {
        Some(id) => eprintln!("demo: tapped highlight #{id} (on {:?})", word.word),
        None => eprintln!(
            "demo: word tapped {:?} at ({:.0},{:.0}) — sentence: {:?}",
            word.word, word.rect.origin.x, word.rect.origin.y, word.sentence
        ),
    });
    view.connect_selection(|selected| match selected {
        Some(s) => eprintln!("demo: selected {:?}", s.text),
        None => eprintln!("demo: selection cleared"),
    });
    view.connect_external_link(|href| eprintln!("demo: external link {href}"));

    let shelf: Shelf = Rc::new(RefCell::new(Vec::new()));

    // ---- This window's keys (Kalam's controls, stood in for) ----
    {
        let view = view.clone();
        let shelf = shelf.clone();
        let window_weak = window.downgrade();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            let prefs = view.prefs();
            let name = keyval.name();
            match name.as_deref() {
                Some("t") => {
                    let theme = prefs.theme.next();
                    eprintln!("demo: theme {}", theme.name());
                    view.set_theme(theme);
                }
                Some("plus") | Some("equal") => {
                    view.set_font_px(prefs.font_px + 1.0);
                    eprintln!("demo: font {} px", view.prefs().font_px);
                }
                Some("minus") => {
                    view.set_font_px(prefs.font_px - 1.0);
                    eprintln!("demo: font {} px", view.prefs().font_px);
                }
                Some("bracketright") => {
                    view.set_line_height(prefs.line_height + 0.1);
                    eprintln!("demo: line height {:.1}", view.prefs().line_height);
                }
                Some("bracketleft") => {
                    view.set_line_height(prefs.line_height - 0.1);
                    eprintln!("demo: line height {:.1}", view.prefs().line_height);
                }
                Some("braceright") => {
                    view.set_column_px(prefs.column_px + 40.0);
                    eprintln!("demo: column {} px", view.prefs().column_px);
                }
                Some("braceleft") => {
                    view.set_column_px(prefs.column_px - 40.0);
                    eprintln!("demo: column {} px", view.prefs().column_px);
                }
                Some("h") => match view.capture_highlight(HighlightColor::Yellow) {
                    Some(h) => {
                        // What Kalam does: INSERT, take the row id, show it.
                        let id = shelf.borrow().len() as i64 + 1;
                        eprintln!(
                            "demo: stored highlight #{id} {:?} from offset {} to {}",
                            h.text, h.start.char_offset, h.end.char_offset
                        );
                        view.show_highlight(id, &h);
                        shelf.borrow_mut().push((id, h));
                    }
                    None => eprintln!("demo: nothing selected to highlight"),
                },
                Some("r") => {
                    // Kalam's AnnotationsReload: hand the whole table back.
                    let shelf = shelf.borrow();
                    view.set_highlights(shelf.iter().map(|(id, h)| (*id, h)));
                    eprintln!("demo: reloaded {} highlights from the shelf", shelf.len());
                }
                Some("x") => match shelf.borrow_mut().pop() {
                    Some((id, _)) => {
                        view.remove_highlight(id);
                        eprintln!("demo: removed highlight #{id}");
                    }
                    None => eprintln!("demo: no highlight to remove"),
                },
                Some("q") => {
                    view.flush();
                    if let Some(window) = window_weak.upgrade() {
                        window.close();
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        window.add_controller(key);
    }

    {
        let view = view.clone();
        window.connect_close_request(move |_| {
            // What Kalam would store on close: the cheap pair and the
            // durable locator (the one-time whole-book count happens
            // here, and its cost is printed).
            let pos = view.position();
            let started = std::time::Instant::now();
            let locator = match view.locator() {
                Some(l) => format!(
                    "offset {} in {} ({:.1}% of the book)",
                    l.char_offset,
                    l.spine_href,
                    l.book_progression * 100.0
                ),
                None => "none".to_string(),
            };
            eprintln!(
                "demo: closing at chapter {} fraction {:.3}; locator {locator} (took {:?}); {}",
                pos.chapter + 1,
                pos.fraction,
                started.elapsed(),
                memory_line(&view)
            );
            view.flush();
            glib::Propagation::Proceed
        });
    }

    window.present();
    view.widget().grab_focus();
}

/// The process's resident memory and the engine's share of it, in one
/// line. `VmRSS` from `/proc/self/status` is the same number `ps -o rss`
/// shows; the engine's cache is what the widget can account for, and the
/// gap between the two is GTK, the fonts and the binary itself.
fn memory_line(view: &ReaderView) -> String {
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        });
    let engine_kb = view.cache_bytes() / 1024;
    match rss {
        Some(kb) => format!(
            "memory {} MB resident, of which engine cache {} MB",
            kb / 1024,
            engine_kb / 1024
        ),
        None => format!("engine cache {} MB", engine_kb / 1024),
    }
}
