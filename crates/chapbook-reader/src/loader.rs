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

// Only the comic arm holds a publication as a trait object; PDFs keep
// their concrete type.
#[cfg(feature = "_comic")]
use chapbook_core::Publication;

/// What the worker loads from.
#[derive(Clone)]
pub(crate) enum LoadSource {
    /// Any image-per-page publication: unit bytes decode as an image.
    #[cfg(feature = "_comic")]
    Comic(Arc<dyn Publication + Send + Sync>),
    /// A PDF: rendered to RGBA directly, with its text layer extracted.
    #[cfg(feature = "pdf")]
    Pdf(Arc<chapbook_pdf::PdfBook>),
}

/// A unit fetched and decoded on the worker, ready for page building.
pub(crate) struct DecodedUnit {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Extracted text lines in natural (point) coordinates; empty for
    /// comics, which have no text layer to recover.
    pub text: Vec<chapbook_core::TextLine>,
    /// Natural page size the text coordinates live in (PDF points).
    pub natural: (f32, f32),
}

pub(crate) type LoadResult = (usize, Result<DecodedUnit, String>);

pub(crate) struct Loader {
    /// `None` only while [`Loader::drop`] is closing the channel to tell
    /// the worker to finish.
    tx: Option<mpsc::Sender<usize>>,
    rx: mpsc::Receiver<LoadResult>,
    /// Units requested and not yet drained from `rx`.
    pending: HashSet<usize>,
    /// Kept, not detached — see [`Loader::drop`].
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Loader {
    /// Spawn the worker for one publication. `waker` is invoked (from the
    /// worker thread) after each result is queued.
    pub fn spawn(source: LoadSource, waker: Arc<dyn Fn() + Send + Sync>) -> Loader {
        let (tx, work_rx) = mpsc::channel::<usize>();
        let (result_tx, rx) = mpsc::channel::<LoadResult>();
        let worker = std::thread::Builder::new()
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
            tx: Some(tx),
            rx,
            pending: HashSet::new(),
            worker: Some(worker),
        }
    }
    /// Queue a unit if it isn't already in flight.
    pub fn request(&mut self, spine: usize) {
        if self.pending.insert(spine) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(spine);
            }
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

impl Drop for Loader {
    /// Wait for the worker to finish before the session is gone.
    ///
    /// The handle is kept rather than detached, and this is the reason.
    /// The worker owns the [`LoadSource`], and for a streamed comic that
    /// owns the HTTP transport — which for a host across the C ABI owns
    /// the host's own context and calls its `finalize` when dropped.
    /// Detached, the sequence was: close the session, return to the host,
    /// and only *then* run the finalizer, on a thread the host cannot see.
    /// A host that freed its context once `cb_session_close` returned —
    /// which is what the header's wording invites — had a use-after-free,
    /// and a page fetch could still be in flight through a transport it
    /// had already torn down.
    ///
    /// It showed up first as a test failing about one run in five, which
    /// is the only way this class of bug ever announces itself.
    ///
    /// The cost is real and worth stating: dropping a session blocks until
    /// the current fetch finishes, which on a slow network is seconds.
    /// That is the honest meaning of "closed" — the alternative is a host
    /// that can never know when its own context is dead.
    fn drop(&mut self) {
        // Hang up first: the worker is parked in `recv`, and closing the
        // channel is what wakes it. Joining before this deadlocks.
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            // A panicked worker has already released everything it owned,
            // so there is nothing left to wait for and nothing to report
            // that the panic itself did not.
            let _ = worker.join();
        }
    }
}

fn load(source: &LoadSource, spine: usize) -> Result<DecodedUnit, String> {
    match source {
        #[cfg(feature = "_comic")]
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
        #[cfg(feature = "pdf")]
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
