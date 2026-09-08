//! The smallest shell that is still a correct one.
//!
//!     cargo run -p chapbook-viewer --example minimal -- <book>
//!
//! No selection, no links, no clipboard, no GPU, no touch, no title bar
//! bookkeeping — `chapbook-viewer` itself has all of that. What is left is
//! the part every shell must get right, and nothing else, so that it can
//! be copied and grown into: open, metrics, turn, wake, draw, save.
//!
//! Read `docs/SHELLS.md` alongside it; the six numbered comments below are
//! its five steps plus the loader rule.

use std::num::NonZeroU32;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use chapbook_reader::chapbook_core::{EdgeSizes, PageMetrics, Rotation, Size};
use chapbook_reader::Session;

fn main() {
    let Some(source) = std::env::args().nth(1) else {
        eprintln!("usage: minimal <book.epub|comic.cbz|doc.pdf|opds-url>");
        std::process::exit(2);
    };

    // 1. Open. This also imports or matches the book in the library,
    //    which is what gives step 5 somewhere to put a position.
    let mut session = match Session::open(&source, chapbook_core::FontSource::host()) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("minimal: {e}");
            std::process::exit(1);
        }
    };

    let event_loop = EventLoop::with_user_event().build().expect("event loop");

    // 6. The loader rule. Image-book units decode on the session's worker
    //    thread; this shell never touches `unit_bytes` itself. The waker
    //    runs *on that thread*, so all it may do is poke the event loop —
    //    the real work happens in `user_event`, back here.
    let proxy = event_loop.create_proxy();
    session.set_waker(move || {
        let _ = proxy.send_event(());
    });

    let mut app = App {
        session,
        window: None,
        surface: None,
    };
    event_loop.run_app(&mut app).expect("event loop run");

    // 5. Leave a bookmark. Reopening this book comes back here.
    app.session.save_position();
}

struct App {
    session: Session,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
}

impl App {
    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return; // Minimized.
        };

        // 2. Metrics, in CSS px and in reading orientation. Setting them
        //    every frame is fine: unchanged metrics return immediately,
        //    and a change relayouts while keeping the reader's place.
        self.session.set_metrics(PageMetrics {
            size: Size::new(size.width as f32 / scale, size.height as f32 / scale),
            margins: EdgeSizes::uniform(40.0),
            dpi_scale: scale,
            rotation: Rotation::None,
        });

        // 4. Draw. `render()` is the whole pipeline plus the bundled CPU
        //    backend; `None` means "nothing to show yet" — a page still
        //    decoding — not an error. A shell that owns its own rasterizer
        //    takes `frame()` instead and reads `intent` and `damage`.
        let Some(pixmap) = self.session.render() else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        // Premultiplied RGBA -> 0RGB u32. The page is opaque, so there is
        // nothing to demultiply.
        for (dst, px) in buffer.iter_mut().zip(pixmap.pixels()) {
            *dst =
                (u32::from(px.red()) << 16) | (u32::from(px.green()) << 8) | u32::from(px.blue());
        }
        let _ = buffer.present();
    }
}

impl ApplicationHandler<()> for App {
    /// The loader finished something. Drain it and redraw if it landed.
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
        self.surface =
            Some(softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface"));
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                // Saving here too: the close button is the exit path
                // shells forget, and losing the reader's place is the
                // bug they notice.
                self.session.save_position();
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // 3. Input becomes Session calls. The turn reports whether
                //    it moved — take that answer rather than deriving one
                //    from `page()`, which resets to 0 when a turn crosses
                //    into the next unit. `docs/SHELLS.md` §3.
                let moved = match event.logical_key {
                    Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::Space) => {
                        self.session.next_page()
                    }
                    Key::Named(NamedKey::ArrowLeft) => self.session.prev_page(),
                    Key::Named(NamedKey::Escape) => {
                        self.session.save_position();
                        event_loop.exit();
                        return;
                    }
                    _ => return,
                };
                if moved {
                    self.request_redraw();
                }
            }
            _ => {}
        }
    }
}
