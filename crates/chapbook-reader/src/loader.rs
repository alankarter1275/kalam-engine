//! The loader thread: image-book units off the UI thread.
//!
//! `Publication::unit_bytes` may block for seconds (a cold PSE page is an
//! HTTP fetch; a PDF page is a CPU rasterization), and image decode isn't
//! free either. The session hands those to one worker thread and renders a
//! placeholder page until the decoded unit arrives; the shell is nudged to
//! redraw through a waker callback (or may poll `Session::poll_loaded`).
//!
//! EPUB chapters don't come through here: they are local zip reads plus a
//! cascade that must run on the session thread anyway.

use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Arc;

use chapbook_core::Publication;

/// What the worker loads from.
#[derive(Clone)]
pub(crate) enum LoadSource {
    /// Any image-per-page publication: unit bytes decode as an image.
    Comic(Arc<dyn Publication + Send + Sync>),
    /// A PDF: rendered to RGBA directly, with its text layer extracted.
    Pdf(Arc<chapbook_pdf::PdfBook>),
}

/// A unit fetched and decoded on the worker, ready for page building.
pub(crate) struct DecodedUnit {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// PDF text lines in natural (point) coordinates; empty for comics.
    pub text: Vec<chapbook_pdf::TextLine>,
    /// Natural page size the text coordinates live in (PDF points).
    pub natural: (f32, f32),
}

pub(crate) type LoadResult = (usize, Result<DecodedUnit, String>);

pub(crate) struct Loader {
    tx: mpsc::Sender<usize>,
    rx: mpsc::Receiver<LoadResult>,
    /// Units requested and not yet drained from `rx`.
    pending: HashSet<usize>,
}

impl Loader {
    /// Spawn the worker for one publication. `waker` is invoked (from the
    /// worker thread) after each result is queued.
    pub fn spawn(source: LoadSource, waker: Arc<dyn Fn() + Send + Sync>) -> Loader {
        let (tx, work_rx) = mpsc::channel::<usize>();
        let (result_tx, rx) = mpsc::channel::<LoadResult>();
        std::thread::Builder::new()
            .name("chapbook-loader".into())
            .spawn(move || {
                while let Ok(spine) = work_rx.recv() {
                    let result = load(&source, spine);
                    if result_tx.send((spine, result)).is_err() {
                        return;
                    }
                    waker();
                }
            })
            .expect("spawn loader thread");
        Loader {
            tx,
            rx,
            pending: HashSet::new(),
        }
    }

    /// Queue a unit if it isn't already in flight.
    pub fn request(&mut self, spine: usize) {
        if self.pending.insert(spine) {
            let _ = self.tx.send(spine);
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drain finished loads.
    pub fn drain(&mut self) -> Vec<LoadResult> {
        let results: Vec<LoadResult> = self.rx.try_iter().collect();
        for (spine, _) in &results {
            self.pending.remove(spine);
        }
        results
    }
}

fn load(source: &LoadSource, spine: usize) -> Result<DecodedUnit, String> {
    match source {
        LoadSource::Comic(book) => {
            let bytes = book.unit_bytes(spine).map_err(|e| e.to_string())?;
            let decoded = image::load_from_memory(&bytes)
                .map_err(|e| format!("decode page image: {e}"))?
                .to_rgba8();
            let (width, height) = decoded.dimensions();
            Ok(DecodedUnit {
                width,
                height,
                rgba: decoded.into_raw(),
                text: Vec::new(),
                natural: (width as f32, height as f32),
            })
        }
        LoadSource::Pdf(book) => {
            let rendered = book.render_page(spine).map_err(|e| e.to_string())?;
            let text = book.text_page(spine).unwrap_or_default();
            Ok(DecodedUnit {
                width: rendered.width,
                height: rendered.height,
                rgba: rendered.rgba,
                text,
                natural: rendered.natural,
            })
        }
    }
}
