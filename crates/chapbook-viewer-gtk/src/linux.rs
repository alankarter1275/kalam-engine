//! The viewer proper, compiled on Linux only.
//!
//! GTK4 reference viewer: the second shell over `chapbook_reader::Session`,
//! proving the session boundary — everything book-shaped lives there; this
//! file is GTK plumbing only.
//!
//! Sources: an `.epub` or `.cbz` path, or an OPDS URL (page-streamed
//! comic).
//!
//! Input goes through `chapbook_core::input` rather than being spelled out
//! here: this file translates a GDK keyval into a `Key` and a click into a
//! point, and `KeyMap`/`TapZones` decide what either one means. So the
//! bindings are the engine's defaults — arrows/PageUp/PageDown/space turn
//! pages, n/p skip units, +/- size the text, t cycles the theme, b and
//! Backspace go back — and a Kobo's bezel buttons would already work if
//! GDK delivered them. Tapping the left or right third turns a page too.
//!
//! What stays this shell's own is what is not an `Action`: c copies the
//! selection, h highlights it, q/Escape quit. Mouse press-drag over text
//! selects; a press on a link follows it.
//!
//! Rendering: the session rasterizes with tiny-skia at device pixels; the
//! draw func converts premultiplied RGBA → cairo ARGB32 and paints it at
//! 1/scale so logical (CSS px) coordinates match GTK's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use chapbook_core::{ActionOutcome, EdgeSizes, Key, KeyMap, PageMetrics, Rotation, Size, TapZones};
use chapbook_reader::Session;

use crate::page_area::PageArea;

pub fn run() -> glib::ExitCode {
    // See chapbook-viewer: the engine reports through `log`.
    chapbook_core::log_to_stderr();
    let Some(source) = std::env::args().nth(1) else {
        eprintln!("usage: chapbook-viewer-gtk <book.epub|comic.cbz|doc.pdf|opds-url>");
        return glib::ExitCode::from(2);
    };
    let config = chapbook_reader::SessionConfig::new(chapbook_core::FontSource::host())
        .with_credentials(std::sync::Arc::new(chapbook_core::EnvCredentials));
    let session = match Session::open_with(&source, config) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("chapbook-viewer-gtk: {e}");
            return glib::ExitCode::from(1);
        }
    };
    let session = Rc::new(RefCell::new(session));

    let app = gtk::Application::builder()
        .application_id("com.ophymx.chapbook")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| build_ui(app, session.clone()));
    app.run_with_args::<&str>(&[])
}

fn build_ui(app: &gtk::Application, session: Rc<RefCell<Session>>) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(session.borrow().title())
        .default_width(600)
        .default_height(800)
        .build();

    // The page widget carries the accessible text surface; to everything
    // below it is just a DrawingArea.
    let area = PageArea::new(session.clone());
    area.set_hexpand(true);
    area.set_vexpand(true);
    window.set_child(Some(&area));

    // ---- Paint ----
    {
        let session = session.clone();
        let window = window.downgrade();
        let page_weak = area.downgrade();
        area.set_draw_func(move |area, ctx, width, height| {
            if width <= 0 || height <= 0 {
                return;
            }
            let scale = area.scale_factor() as f32;
            let mut s = session.borrow_mut();
            s.set_metrics(PageMetrics {
                size: Size::new(width as f32, height as f32),
                margins: EdgeSizes::uniform(40.0),
                dpi_scale: scale,
                rotation: Rotation::None,
            });
            let Some(pixmap) = s.render() else { return };
            if let Some(window) = window.upgrade() {
                let count = s.page_count().max(1);
                window.set_title(Some(&format!(
                    "{} — {} {}/{} p {}/{}",
                    s.title(),
                    match s.kind() {
                        chapbook_core::BookKind::Epub => "ch",
                        chapbook_core::BookKind::Comic | chapbook_core::BookKind::Pdf => "pg",
                    },
                    s.spine() + 1,
                    s.spine_len(),
                    s.page() + 1,
                    count,
                )));
            }
            drop(s);

            // Premultiplied RGBA → cairo ARGB32 (BGRA in little-endian).
            let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
            let mut data = pixmap.take();
            for px in data.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
            let stride = pw * 4;
            let Ok(surface) =
                cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, pw, ph, stride)
            else {
                return;
            };
            // The pixmap is device pixels; paint at 1/scale so it maps to
            // the widget's logical size.
            ctx.scale(1.0 / scale as f64, 1.0 / scale as f64);
            let _ = ctx.set_source_surface(&surface, 0.0, 0.0);
            let _ = ctx.paint();

            // Every content change funnels through a draw, so this is the
            // one place assistive technology needs to be told — from an
            // idle rather than inside the draw vfunc, and page_changed
            // itself no-ops unless the page's text actually moved.
            let page = page_weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(page) = page.upgrade() {
                    page.page_changed();
                }
            });
        });
    }

    // ---- Keyboard ----
    {
        let session = session.clone();
        let area = area.downgrade();
        let window_weak = window.downgrade();
        // This shell has no chrome, so there is nothing to hand
        // `ToggleMenu` to. Unbinding it is the honest version of leaving
        // `m` bound to a key press that does nothing.
        let mut keys = KeyMap::default();
        keys.unbind(Key::Char('m'));
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            let mut s = session.borrow_mut();
            let name = keyval.name();
            // Shell-owned keys first, and they are exactly the ones that
            // are not reading actions: the clipboard, the annotation
            // store and the window belong to the app. Everything the
            // engine can do for itself falls through to the key map.
            let outcome = match name.as_deref() {
                Some("h") => {
                    // The stored highlight replaces the selection that
                    // made it.
                    s.add_highlight();
                    s.selection_clear();
                    ActionOutcome::Changed
                }
                Some("Escape") if s.selected_range().is_some() => {
                    s.selection_clear();
                    ActionOutcome::Changed
                }
                Some("c") => {
                    // gdk owns the clipboard for us; nothing to redraw.
                    if let (Some(window), Some(text)) = (window_weak.upgrade(), s.selected_text()) {
                        window.clipboard().set_text(&text);
                    }
                    return glib::Propagation::Stop;
                }
                // kalam: two reader-override toggles the reference viewer
                // never exposed. Kalam's adapter will set both permanently;
                // here they let a tester see the difference on a real book.
                //
                // `f` forces the reader's typeface over the publisher's
                // (`ReadingSettings::font_family`, which wins even against
                // `body { font-family }` — the fix for books that embed a
                // Symbol-encoded font and come out looking Greek).
                // `s` drops the publisher's stylesheets altogether
                // (`publisher_styles`), the blunter instrument.
                Some("f") => {
                    let mut settings = s.settings().clone();
                    settings.font_family = match settings.font_family {
                        Some(_) => None,
                        None => Some("serif".to_string()),
                    };
                    let title = match &settings.font_family {
                        Some(name) => format!("font: {name}"),
                        None => "font: publisher's".to_string(),
                    };
                    eprintln!("chapbook-viewer-gtk: {title}");
                    s.set_settings(settings, chapbook_reader::SettingsScope::ThisBook);
                    ActionOutcome::Changed
                }
                Some("s") => {
                    let mut settings = s.settings().clone();
                    settings.publisher_styles = !settings.publisher_styles;
                    eprintln!(
                        "chapbook-viewer-gtk: publisher styles {}",
                        if settings.publisher_styles { "on" } else { "off" }
                    );
                    s.set_settings(settings, chapbook_reader::SettingsScope::ThisBook);
                    ActionOutcome::Changed
                }
                Some("q") | Some("Escape") => {
                    s.save_position();
                    drop(s);
                    if let Some(window) = window_weak.upgrade() {
                        window.close();
                    }
                    return glib::Propagation::Stop;
                }
                name => match name.and_then(engine_key).and_then(|k| keys.action(k)) {
                    Some(action) => s.apply(action),
                    None => return glib::Propagation::Proceed,
                },
            };
            drop(s);
            // Repaint only if something changed. That is half of what
            // the outcome carries, and it is why the end of the book no
            // longer redraws the same page on every press.
            if outcome.needs_redraw() {
                if let Some(area) = area.upgrade() {
                    area.queue_draw();
                }
            }
            // The other half. GTK's propagation flag is the same bit
            // Android's `onKeyDown` returns, so this is where a bound key
            // the engine declined — Back at the bottom of its stack —
            // reaches whatever is behind this handler.
            if outcome.consumed() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key);
    }

    // ---- Links, selection and tap zones (press-drag) ----
    {
        let area_weak = area.downgrade();
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        // Set when a press starts a selection rather than following a
        // link. A press that then never moves is a tap, and `drag_end`
        // asks the engine what a tap at that point means.
        let tap = Rc::new(Cell::new(false));
        // Thirds, with an inert middle: this shell has no menu, so the
        // band that would open one is bound to nothing rather than to an
        // action `apply` would refuse. The direction comes from the book
        // via the session, not from `TapZones::default`, which is `Ltr`
        // and cannot know better.
        let zones = TapZones {
            middle: None,
            ..TapZones::new(session.borrow().reading_direction())
        };
        {
            let session = session.clone();
            let area_weak = area_weak.clone();
            let tap = tap.clone();
            drag.connect_drag_begin(move |_, x, y| {
                let mut s = session.borrow_mut();
                tap.set(false);
                // A press on a link follows it rather than starting a
                // selection there.
                if let Some(href) = s.link_at(x as f32, y as f32) {
                    if s.follow_link(&href) {
                        drop(s);
                        if let Some(area) = area_weak.upgrade() {
                            area.queue_draw();
                        }
                        return;
                    }
                }
                s.selection_begin(x as f32, y as f32);
                tap.set(true);
                drop(s);
                if let Some(area) = area_weak.upgrade() {
                    area.queue_draw();
                }
            });
        }
        {
            let session = session.clone();
            let area_weak = area_weak.clone();
            drag.connect_drag_update(move |gesture, dx, dy| {
                if let Some((sx, sy)) = gesture.start_point() {
                    session
                        .borrow_mut()
                        .selection_drag((sx + dx) as f32, (sy + dy) as f32);
                    if let Some(area) = area_weak.upgrade() {
                        area.queue_draw();
                    }
                }
            });
        }
        {
            let session = session.clone();
            let area_weak = area_weak.clone();
            drag.connect_drag_end(move |gesture, dx, dy| {
                // A press that wandered, or that caught text on the way,
                // was a selection and the drag handlers already have it.
                // The slop is a mouse's, not a finger's — a touchscreen
                // shell wants its platform's own threshold here.
                const SLOP: f64 = 4.0;
                if !tap.replace(false) || dx.abs() > SLOP || dy.abs() > SLOP {
                    return;
                }
                let Some((x, y)) = gesture.start_point() else {
                    return;
                };
                let mut s = session.borrow_mut();
                if s.selected_range().is_some() {
                    return;
                }
                // The press anchored an empty selection; drop it before
                // turning, so the anchor does not outlive the page.
                s.selection_clear();
                // The metrics come back from the session rather than from
                // this file's own bookkeeping, which is what lets
                // `action_at` undo a rotation the shell never tracked.
                let Some(metrics) = s.metrics() else { return };
                let Some(action) = zones.action_at(x as f32, y as f32, &metrics) else {
                    return;
                };
                let outcome = s.apply(action);
                drop(s);
                if outcome.needs_redraw() {
                    if let Some(area) = area_weak.upgrade() {
                        area.queue_draw();
                    }
                }
            });
        }
        area.add_controller(drag);
    }

    // ---- Background loads (comic/PDF pages) ----
    //
    // This used to be a 100ms timer, on the grounds that GTK's main
    // context has no cheap cross-thread wakeup for a session that is not
    // `Send`. The session does not have to be: only the *sender* crosses
    // threads. The loader pushes an empty message, the receiving half runs
    // on the main context through `spawn_future_local`, and there — on the
    // thread that owns everything — it is free to touch the session and
    // the widget.
    //
    // So the shell now sleeps when the book does, instead of waking ten
    // times a second to ask a channel whether anything happened.
    {
        let (wake_tx, mut wake_rx) = futures_channel::mpsc::unbounded::<()>();
        session.borrow_mut().set_waker(move || {
            // Failure means the receiver is gone, which means the window
            // is closing. Nothing to do about it and nothing to say.
            let _ = wake_tx.unbounded_send(());
        });

        let session = session.clone();
        let area_weak = area.downgrade();
        gtk::glib::spawn_future_local(async move {
            use futures_util::StreamExt;
            while wake_rx.next().await.is_some() {
                let mut session = session.borrow_mut();
                let redraw = session.poll_loaded();
                // The engine logs this failure too, and a harness with no
                // error UI cannot do much better than say it twice. The
                // point is which side is speaking: the engine's line goes
                // to whoever installed a log backend, this one is the
                // *shell* having been told, which is what a real one needs
                // in order to put it on the page instead of in a terminal.
                for event in session.drain_events() {
                    if let chapbook_reader::SessionEvent::UnitFailed { spine, message } = event {
                        eprintln!(
                            "chapbook-viewer-gtk: page {} will not load: {message}",
                            spine + 1
                        );
                    }
                }
                drop(session);
                if redraw {
                    if let Some(area) = area_weak.upgrade() {
                        area.queue_draw();
                    }
                }
            }
        });
    }

    // ---- Persistence on close ----
    {
        let session = session.clone();
        window.connect_close_request(move |_| {
            session.borrow_mut().save_position();
            glib::Propagation::Proceed
        });
    }

    window.present();
    // Focus the page, not the window shell around it: a screen reader
    // speaks the focused object, and the page is the object with text.
    area.grab_focus();
}

/// A GDK keyval name in the engine's key vocabulary, or `None` for a key
/// no reader binds.
///
/// The whole of this shell's keyboard translation. What each key *does* is
/// `KeyMap`'s answer, not this function's, which is the split that lets a
/// binding be written once instead of once per shell.
///
/// `Key::TurnPrev`/`TurnNext` have no case here on purpose: a desktop has
/// no bezel buttons to deliver, and inventing a GDK name for one would be
/// a guess. A port to hardware that has them adds two lines here and
/// changes nothing else.
fn engine_key(name: &str) -> Option<Key> {
    Some(match name {
        "Right" => Key::ArrowRight,
        "Left" => Key::ArrowLeft,
        "Up" => Key::ArrowUp,
        "Down" => Key::ArrowDown,
        "Page_Down" => Key::PageDown,
        "Page_Up" => Key::PageUp,
        "space" => Key::Space,
        "BackSpace" => Key::Backspace,
        // GDK names the punctuation; the map binds the character.
        "plus" => Key::Char('+'),
        "equal" => Key::Char('='),
        "minus" => Key::Char('-'),
        // Anything else is a key map's `Char`, if it is one character at
        // all — every other GDK name ("Shift_L", "F11") is several.
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}
