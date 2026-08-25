//! The viewer proper, compiled on Linux only.
//!
//! GTK4 reference viewer: the second shell over `chapbook_reader::Session`,
//! proving the session boundary — everything book-shaped lives there; this
//! file is GTK plumbing only.
//!
//! Sources: an `.epub` or `.cbz` path, or an OPDS URL (page-streamed
//! comic). Keys match the winit shell: Right/PageDown/space next page ·
//! Left/PageUp previous · n/p unit · +/- font size · t theme · c copy
//! selection · h highlight it · b back · q/Escape quit. Mouse press-drag
//! over text selects; a press on a link follows it.
//!
//! Rendering: the session rasterizes with tiny-skia at device pixels; the
//! draw func converts premultiplied RGBA → cairo ARGB32 and paints it at
//! 1/scale so logical (CSS px) coordinates match GTK's.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::Session;

pub fn run() -> glib::ExitCode {
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

    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    window.set_child(Some(&area));

    // ---- Paint ----
    {
        let session = session.clone();
        let window = window.downgrade();
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
        });
    }

    // ---- Keyboard ----
    {
        let session = session.clone();
        let area = area.downgrade();
        let window_weak = window.downgrade();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            let mut s = session.borrow_mut();
            match keyval.name().as_deref() {
                Some("Right") | Some("Page_Down") | Some("space") => {
                    s.next_page();
                }
                Some("Left") | Some("Page_Up") => {
                    s.prev_page();
                }
                Some("n") => {
                    s.next_unit();
                }
                Some("p") => {
                    s.prev_unit();
                }
                Some("plus") | Some("equal") => s.adjust_font(2.0),
                Some("minus") => s.adjust_font(-2.0),
                Some("t") => s.cycle_theme(),
                Some("b") => {
                    s.back();
                }
                Some("h") => {
                    // The stored highlight replaces the selection that
                    // made it.
                    s.add_highlight();
                    s.selection_clear();
                }
                Some("Escape") if s.selected_range().is_some() => s.selection_clear(),
                Some("c") => {
                    // gdk owns the clipboard for us; nothing to redraw.
                    if let (Some(window), Some(text)) = (window_weak.upgrade(), s.selected_text()) {
                        window.clipboard().set_text(&text);
                    }
                    return glib::Propagation::Stop;
                }
                Some("q") | Some("Escape") => {
                    s.save_position();
                    drop(s);
                    if let Some(window) = window_weak.upgrade() {
                        window.close();
                    }
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            }
            drop(s);
            if let Some(area) = area.upgrade() {
                area.queue_draw();
            }
            glib::Propagation::Stop
        });
        window.add_controller(key);
    }

    // ---- Selection (press-drag) ----
    {
        let session = session.clone();
        let area_weak = area.downgrade();
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        {
            let session = session.clone();
            let area_weak = area_weak.clone();
            drag.connect_drag_begin(move |_, x, y| {
                let mut s = session.borrow_mut();
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
                drop(s);
                if let Some(area) = area_weak.upgrade() {
                    area.queue_draw();
                }
            });
        }
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
        area.add_controller(drag);
    }

    // ---- Background loads (comic/PDF pages) ----
    // GTK's main context has no cheap cross-thread waker for a non-Send
    // session; a 100ms poll is only live while the app runs and is a
    // no-op channel check when nothing is loading.
    {
        let session = session.clone();
        let area_weak = area.downgrade();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if session.borrow_mut().poll_loaded() {
                if let Some(area) = area_weak.upgrade() {
                    area.queue_draw();
                }
            }
            gtk::glib::ControlFlow::Continue
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
}
