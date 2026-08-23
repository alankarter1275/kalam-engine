//! Minimal reference viewer. Exists to exercise the pipeline end-to-end:
//! open an EPUB, paginate, render pages, turn pages with the keyboard.
//! Not a polished product.
//!
//! Keys: Right/PageDown/Space next page · Left/PageUp previous ·
//! n/p chapter · +/- font size · q/Escape quit.
//!
//! Contract (see `Publication::unit_bytes`): book I/O may block and fail
//! with network errors for remote-backed publications. This v1 harness only
//! opens local EPUBs and loads chapters synchronously on the event loop —
//! acceptable for local zips, to be replaced by a loader thread when remote
//! books land (M6+). Library integration (position persistence) lands in M7.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use chapbook_core::{EdgeSizes, PageMetrics, Publication, ReadingSettings, Size};
use chapbook_epub::Book;
use chapbook_layout::ChapterLayout;
use chapbook_render_tinyskia::tiny_skia::Pixmap;
use chapbook_render_tinyskia::Renderer;

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: chapbook-viewer <book.epub>");
        std::process::exit(2);
    };
    let book = match Book::open(&path) {
        Ok(book) => book,
        Err(e) => {
            eprintln!("chapbook-viewer: {e}");
            std::process::exit(1);
        }
    };
    let title = book
        .metadata()
        .title
        .clone()
        .unwrap_or_else(|| "chapbook".to_string());

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        title,
        book,
        window: None,
        surface: None,
        renderer: Renderer::new(),
        fonts: chapbook_layout::system_font_system(),
        settings: ReadingSettings::default(),
        metrics: None,
        layouts: HashMap::new(),
        images: HashMap::new(),
        registered_fonts: std::collections::HashSet::new(),
        spine: 0,
        page: 0,
    };
    event_loop.run_app(&mut app).expect("event loop run");
}

type SbSurface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

struct App {
    title: String,
    book: Book,
    window: Option<Arc<Window>>,
    surface: Option<SbSurface>,
    renderer: Renderer,
    fonts: cosmic_text::FontSystem,
    settings: ReadingSettings,
    /// Current page metrics; `None` until the window reports a size.
    metrics: Option<PageMetrics>,
    /// Chapter layouts cached for the current metrics+settings.
    layouts: HashMap<usize, ChapterLayout>,
    /// Decoded images per chapter (parallel to `layouts`).
    images: HashMap<usize, chapbook_paint::ImageStore>,
    /// @font-face families already loaded into the font system.
    registered_fonts: std::collections::HashSet<String>,
    spine: usize,
    page: usize,
}

impl App {
    fn layout_chapter(&mut self, spine: usize) -> Option<&ChapterLayout> {
        let metrics = self.metrics?;
        if !self.layouts.contains_key(&spine) {
            let href = self.book.spine_item(spine).ok()?.href.clone();
            let bytes = self.book.unit_bytes(spine).ok()?;
            let mut doc = chapbook_dom::parse_xhtml(&bytes, &href).ok()?;
            let css: Vec<(String, String)> = doc
                .stylesheet_sources()
                .iter()
                .filter_map(|s| match s {
                    chapbook_dom::StylesheetSource::Inline(t) => Some((t.clone(), href.clone())),
                    chapbook_dom::StylesheetSource::External(rel) => {
                        self.book.resource(&href, rel).ok().map(|r| {
                            (
                                String::from_utf8_lossy(&r.data).into_owned(),
                                chapbook_epub::resolve_href(&href, rel),
                            )
                        })
                    }
                })
                .collect();

            // Register the chapter's @font-face fonts once per family.
            for face in chapbook_layout::extract_font_faces(&css) {
                if !self.registered_fonts.insert(face.family.clone()) {
                    continue;
                }
                for src in &face.sources {
                    if let Ok(res) = self.book.resource(&face.base, src) {
                        if chapbook_layout::register_font(&mut self.fonts, &face.family, res.data) {
                            break;
                        }
                    }
                }
            }
            let images = chapbook_layout::collect_images(&doc, |img_href| {
                self.book.resource(&href, img_href).ok().map(|r| r.data)
            });

            let sheets: Vec<String> = css.iter().map(|(text, _)| text.clone()).collect();
            let mut engine = chapbook_style::StyleEngine::new(&metrics, &self.settings);
            engine.set_author_sheets(&sheets);
            engine.style_document(&mut doc);
            let layout =
                chapbook_layout::paginate(&doc, &sheets, &metrics, &mut self.fonts, &images);
            self.layouts.insert(spine, layout);
            self.images.insert(spine, images);
        }
        self.layouts.get(&spine)
    }

    /// Locator of the current page, for restoring position across relayout.
    fn current_locator(&self) -> u32 {
        self.layouts
            .get(&self.spine)
            .and_then(|l| l.char_map.get(self.page).copied())
            .unwrap_or(0)
    }

    fn invalidate_layouts(&mut self, keep_position: bool) {
        let locator = self.current_locator();
        self.layouts.clear();
        self.images.clear();
        if keep_position {
            if let Some(layout) = self.layout_chapter(self.spine) {
                self.page = layout.page_of(locator);
            }
        } else {
            self.page = 0;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn page_count(&mut self, spine: usize) -> usize {
        self.layout_chapter(spine).map_or(0, |l| l.pages.len())
    }

    fn next_page(&mut self) {
        let count = self.page_count(self.spine);
        if self.page + 1 < count {
            self.page += 1;
        } else if self.spine + 1 < self.book.spine().len() {
            self.spine += 1;
            self.page = 0;
        }
    }

    fn prev_page(&mut self) {
        if self.page > 0 {
            self.page -= 1;
        } else if self.spine > 0 {
            self.spine -= 1;
            self.page = self.page_count(self.spine).saturating_sub(1);
        }
    }

    fn next_chapter(&mut self) {
        if self.spine + 1 < self.book.spine().len() {
            self.spine += 1;
            self.page = 0;
        }
    }

    fn prev_chapter(&mut self) {
        if self.spine > 0 {
            self.spine -= 1;
            self.page = 0;
        }
    }

    fn adjust_font(&mut self, delta: f32) {
        self.settings.base_font_px = (self.settings.base_font_px + delta).clamp(10.0, 40.0);
        self.invalidate_layouts(true);
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

        // Page metrics follow the window: CSS px = device px / scale.
        let metrics = PageMetrics {
            size: Size::new(size.width as f32 / scale, size.height as f32 / scale),
            margins: EdgeSizes::uniform(40.0),
            dpi_scale: scale,
        };
        let metrics_changed = self.metrics != Some(metrics);
        if metrics_changed {
            let keep = self.metrics.is_some();
            self.metrics = Some(metrics);
            if keep {
                let locator = self.current_locator();
                self.layouts.clear();
                self.images.clear();
                if let Some(layout) = self.layout_chapter(self.spine) {
                    self.page = layout.page_of(locator);
                }
            } else {
                self.layouts.clear();
                self.images.clear();
            }
        }

        let (dl, page_count) = {
            let spine = self.spine;
            let page_idx = self.page;
            let Some(layout) = self.layout_chapter(spine) else {
                return;
            };
            let page_idx = page_idx.min(layout.pages.len().saturating_sub(1));
            let Some(page) = layout.pages.get(page_idx) else {
                return;
            };
            (
                chapbook_paint::build_display_list(page, chapbook_core::Rgba::WHITE),
                layout.pages.len(),
            )
        };
        self.page = self.page.min(page_count.saturating_sub(1));

        let Some(mut pixmap) = Pixmap::new(size.width, size.height) else {
            return;
        };
        let empty = chapbook_paint::ImageStore::default();
        let images = self.images.get(&self.spine).unwrap_or(&empty);
        self.renderer
            .render(&dl, &mut self.fonts, images, scale, &mut pixmap);

        window.set_title(&format!(
            "{} — ch {}/{} p {}/{}",
            self.title,
            self.spine + 1,
            self.book.spine().len(),
            self.page + 1,
            page_count
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
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title(self.title.clone())
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                match event.logical_key {
                    Key::Named(NamedKey::ArrowRight)
                    | Key::Named(NamedKey::PageDown)
                    | Key::Named(NamedKey::Space) => self.next_page(),
                    Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::PageUp) => {
                        self.prev_page()
                    }
                    Key::Named(NamedKey::Escape) => {
                        event_loop.exit();
                        return;
                    }
                    Key::Character(ref c) => match c.as_str() {
                        "q" => {
                            event_loop.exit();
                            return;
                        }
                        "n" => self.next_chapter(),
                        "p" => self.prev_chapter(),
                        "+" | "=" => self.adjust_font(2.0),
                        "-" => self.adjust_font(-2.0),
                        _ => return,
                    },
                    _ => return,
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}
