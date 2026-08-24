//! Minimal reference viewer (winit + softbuffer): a thin shell over
//! `chapbook_reader::Session`, which owns the whole reading pipeline.
//! Exists to exercise chapbook end-to-end; not a polished product.
//!
//! Sources: an `.epub` or `.cbz` path, or an OPDS URL (page-streamed
//! comic). Keys: Right/PageDown/Space next page · Left/PageUp previous ·
//! n/p unit · +/- font size · t theme (light/sepia/dark) · c copy
//! selection · h highlight it · q/Escape quit. Mouse or touch: press-drag
//! over text selects; a tap clears. Touch tracks the first finger only.
//!
//! Image-book units (comic pages, PDF rasterizations) load on the
//! session's worker thread; the loader wakes this shell through the event
//! loop proxy and the placeholder page repaints when pixels arrive. EPUB
//! chapters remain synchronous (local zip + cascade).

use std::num::NonZeroU32;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::Session;

fn main() {
    let Some(source) = std::env::args().nth(1) else {
        eprintln!("usage: chapbook-viewer <book.epub|comic.cbz|doc.pdf|opds-url>");
        std::process::exit(2);
    };
    let session = match Session::open(&source) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("chapbook-viewer: {e}");
            std::process::exit(1);
        }
    };
    let event_loop = EventLoop::with_user_event().build().expect("event loop");
    // Loader wakeup: the worker thread pokes the event loop, which polls
    // finished loads and redraws (see `user_event`).
    let proxy = event_loop.create_proxy();
    let mut session = session;
    session.set_waker(move || {
        let _ = proxy.send_event(());
    });
    let mut app = App {
        session,
        window: None,
        surface: None,
        cursor: (0.0, 0.0),
        selecting: false,
        active_touch: None,
        clipboard: arboard::Clipboard::new()
            .map_err(|e| eprintln!("chapbook-viewer: no clipboard: {e}"))
            .ok(),
    };
    event_loop.run_app(&mut app).expect("event loop run");
    app.session.save_position();
}

type SbSurface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

struct App {
    session: Session,
    window: Option<Arc<Window>>,
    surface: Option<SbSurface>,
    /// Last cursor position in page CSS px.
    cursor: (f32, f32),
    selecting: bool,
    /// Finger driving a touch selection — winit reports touch as `Touch`
    /// events, not synthesized pointer events (GTK's gestures abstract
    /// that; here it's manual). First finger wins; others are ignored.
    active_touch: Option<u64>,
    /// Kept for the process's life: on X11 the clipboard is served by this
    /// handle's background thread, so dropping it drops what we copied.
    /// `None` when the platform has no clipboard (headless).
    clipboard: Option<arboard::Clipboard>,
}

impl App {
    /// Copy the selection to the system clipboard. Silent when nothing is
    /// selected or the selection covers no text (a comic page).
    fn copy_selection(&mut self) {
        let (Some(clipboard), Some(text)) = (self.clipboard.as_mut(), self.session.selected_text())
        else {
            return;
        };
        if let Err(e) = clipboard.set_text(text) {
            eprintln!("chapbook-viewer: copy failed: {e}");
        }
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let scale = window.scale_factor() as f32;
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.session.set_metrics(PageMetrics {
            size: Size::new(size.width as f32 / scale, size.height as f32 / scale),
            margins: EdgeSizes::uniform(40.0),
            dpi_scale: scale,
            rotation: Rotation::None,
        });
        let Some(pixmap) = self.session.render() else {
            return;
        };

        let page_count = self.session.page_count().max(1);
        window.set_title(&format!(
            "{} — {} {}/{} p {}/{}",
            self.session.title(),
            match self.session.kind() {
                chapbook_core::BookKind::Epub => "ch",
                chapbook_core::BookKind::Comic | chapbook_core::BookKind::Pdf => "pg",
            },
            self.session.spine() + 1,
            self.session.spine_len(),
            self.session.page() + 1,
            page_count,
        ));

        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        // Premultiplied RGBA → 0RGB u32; the page is opaque so no
        // demultiplication is needed.
        for (dst, px) in buffer.iter_mut().zip(pixmap.pixels()) {
            *dst =
                (u32::from(px.red()) << 16) | (u32::from(px.green()) << 8) | u32::from(px.blue());
        }
        let _ = buffer.present();
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler<()> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        if self.session.poll_loaded() {
            self.request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title(self.session.title().to_string())
                        .with_inner_size(winit::dpi::LogicalSize::new(600.0, 800.0)),
                )
                .expect("create window"),
        );
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.window = Some(window);
        self.surface = Some(surface);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.session.save_position();
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self
                    .window
                    .as_ref()
                    .map_or(1.0, |w| w.scale_factor() as f32);
                self.cursor = (position.x as f32 / scale, position.y as f32 / scale);
                if self.selecting {
                    self.session.selection_drag(self.cursor.0, self.cursor.1);
                    self.request_redraw();
                }
            }
            WindowEvent::Touch(Touch {
                id,
                phase,
                location,
                ..
            }) => {
                let scale = self
                    .window
                    .as_ref()
                    .map_or(1.0, |w| w.scale_factor() as f32);
                let (x, y) = (location.x as f32 / scale, location.y as f32 / scale);
                match phase {
                    TouchPhase::Started => {
                        if self.active_touch.is_none() {
                            self.active_touch = Some(id);
                            self.selecting = self.session.selection_begin(x, y);
                            self.request_redraw();
                        }
                    }
                    TouchPhase::Moved => {
                        if self.active_touch == Some(id) && self.selecting {
                            self.session.selection_drag(x, y);
                            self.request_redraw();
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        if self.active_touch == Some(id) {
                            self.active_touch = None;
                            if self.selecting {
                                self.session.selection_drag(x, y);
                                self.selecting = false;
                            }
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                match state {
                    ElementState::Pressed => {
                        self.selecting = self.session.selection_begin(self.cursor.0, self.cursor.1);
                        self.request_redraw();
                    }
                    ElementState::Released => {
                        self.selecting = false;
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                match event.logical_key {
                    Key::Named(NamedKey::ArrowRight)
                    | Key::Named(NamedKey::PageDown)
                    | Key::Named(NamedKey::Space) => self.session.next_page(),
                    Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::PageUp) => {
                        self.session.prev_page()
                    }
                    Key::Named(NamedKey::Escape) => {
                        if self.session.selected_range().is_some() {
                            self.session.selection_clear();
                            self.request_redraw();
                            return;
                        }
                        self.session.save_position();
                        event_loop.exit();
                        return;
                    }
                    Key::Character(ref c) => match c.as_str() {
                        "q" => {
                            self.session.save_position();
                            event_loop.exit();
                            return;
                        }
                        "n" => self.session.next_unit(),
                        "p" => self.session.prev_unit(),
                        "+" | "=" => self.session.adjust_font(2.0),
                        "-" => self.session.adjust_font(-2.0),
                        "t" => self.session.cycle_theme(),
                        "c" => {
                            self.copy_selection();
                            return;
                        }
                        "h" => {
                            // The stored highlight replaces the selection
                            // that made it.
                            self.session.add_highlight();
                            self.session.selection_clear();
                        }
                        _ => return,
                    },
                    _ => return,
                }
                self.request_redraw();
            }
            _ => {}
        }
    }
}
