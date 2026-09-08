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
//! width, `h` highlights the selection, `q` quits. Mouse: drag to select,
//! tap a word to "look it up" (printed), tap the left or right third to
//! turn.
//!
//! Everything the widget reports goes to stderr, prefixed `demo:`, so a
//! run doubles as a trace of what Kalam would receive.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use kalam_reader::{HighlightColor, KalamPrefs, ReaderOptions, ReaderView};

fn main() -> glib::ExitCode {
    chapbook_core::log_to_stderr();
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
            "demo: opened \"{}\" ({} chapters) in {:?}",
            view.title(),
            view.chapter_count(),
            started.elapsed()
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
        view.connect_position(move |pos| {
            if first.replace(false) {
                eprintln!("demo: first page on screen {:?} after launch", started.elapsed());
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
    view.connect_word(|word| {
        eprintln!(
            "demo: word tapped {:?} at ({:.0},{:.0}) — sentence: {:?}",
            word.word, word.rect.origin.x, word.rect.origin.y, word.sentence
        );
    });
    view.connect_selection(|selected| match selected {
        Some(s) => eprintln!("demo: selected {:?}", s.text),
        None => eprintln!("demo: selection cleared"),
    });
    view.connect_external_link(|href| eprintln!("demo: external link {href}"));

    // ---- This window's keys (Kalam's controls, stood in for) ----
    {
        let view = view.clone();
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
                Some("h") => match view.add_highlight(HighlightColor::Yellow) {
                    Some(h) => eprintln!(
                        "demo: highlight #{} {:?} from offset {} to {}",
                        h.id, h.text, h.start.char_offset, h.end.char_offset
                    ),
                    None => eprintln!("demo: nothing selected to highlight"),
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
                "demo: closing at chapter {} fraction {:.3}; locator {locator} (took {:?})",
                pos.chapter + 1,
                pos.fraction,
                started.elapsed()
            );
            view.flush();
            glib::Propagation::Proceed
        });
    }

    window.present();
    view.widget().grab_focus();
}
