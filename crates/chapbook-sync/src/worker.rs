//! The thread a shell asks to sync on.
//!
//! Same shape as `chapbook-reader`'s loader, for the same reason: every
//! call in [`SyncEngine`] can block for as long as a network wants it to,
//! and `CredentialStore` lookups happen underneath — which is exactly why
//! the credential contract says a store must never prompt. A shell posts
//! a command, gets on with painting, and drains events when it is nudged.
//!
//! No schedule lives here either. The worker syncs when told.

use std::sync::mpsc;
use std::sync::Arc;

use chapbook_library::BookId;

use crate::engine::{BookReport, SyncEngine};

/// What to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncCommand {
    /// Reconcile one book.
    Book(BookId),
    /// Reconcile every book with a service to talk to.
    All,
}

/// What happened, for a shell that wants to say so.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// A book reconciled. Read [`BookReport::position`] and
    /// [`BookReport::annotations`] for what actually moved.
    Book(Box<BookReport>),
    /// A book did not reconcile. The rest of the batch still ran.
    Failed { book: BookId, reason: String },
    /// A batch finished, so a shell can stop showing a spinner.
    Finished { books: usize },
}

/// A sync engine on its own thread.
///
/// Dropping it closes the channel, which ends the worker after the command
/// in flight — the same join-on-drop the loader does, so a shell that drops
/// this knows nothing is still touching the database behind it.
pub struct SyncWorker {
    /// `None` only while [`SyncWorker::drop`] is closing the channel.
    tx: Option<mpsc::Sender<SyncCommand>>,
    rx: mpsc::Receiver<SyncEvent>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl SyncWorker {
    /// Spawn the worker around an engine. `waker` is called *from the
    /// worker thread* after each event is queued, exactly as the loader's
    /// is: it should do nothing but nudge the UI thread to drain.
    pub fn spawn(mut engine: SyncEngine, waker: Arc<dyn Fn() + Send + Sync>) -> SyncWorker {
        let (tx, command_rx) = mpsc::channel::<SyncCommand>();
        let (event_tx, rx) = mpsc::channel::<SyncEvent>();
        let worker = std::thread::Builder::new()
            .name("chapbook-sync".into())
            .spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    let Ok(books) = run(&mut engine, command, &event_tx, &waker) else {
                        return; // the receiver went away
                    };
                    if event_tx.send(SyncEvent::Finished { books }).is_err() {
                        return;
                    }
                    waker();
                }
            })
            .expect("spawn sync worker");
        SyncWorker {
            tx: Some(tx),
            rx,
            worker: Some(worker),
        }
    }

    /// Ask for some work. Returns `false` once the worker has gone.
    pub fn request(&self, command: SyncCommand) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(command).is_ok())
    }

    /// Take everything that has happened since the last drain.
    pub fn drain(&self) -> Vec<SyncEvent> {
        self.rx.try_iter().collect()
    }

    /// Block until the next event, for a caller with nothing else to do —
    /// a CLI, or a test. `None` once the worker has finished.
    pub fn next_event(&self) -> Option<SyncEvent> {
        self.rx.recv().ok()
    }
}

impl Drop for SyncWorker {
    fn drop(&mut self) {
        // Close the channel first: the worker is blocked in `recv` and
        // that is what ends it. Then join, so that a dropped worker means
        // no thread is still writing to the library.
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// One command's worth of work, reporting each book as it lands rather
/// than at the end — a shelf sync is slow and a shell should be able to
/// show progress.
fn run(
    engine: &mut SyncEngine,
    command: SyncCommand,
    events: &mpsc::Sender<SyncEvent>,
    waker: &Arc<dyn Fn() + Send + Sync>,
) -> Result<usize, ()> {
    let books = match command {
        SyncCommand::Book(book) => vec![book],
        SyncCommand::All => engine
            .library()
            .books_with_sync_targets()
            .unwrap_or_default(),
    };
    let mut done = 0;
    for book in books {
        let event = match engine.sync_book(book) {
            Ok(report) => SyncEvent::Book(Box::new(report)),
            Err(e) => SyncEvent::Failed {
                book,
                reason: e.to_string(),
            },
        };
        events.send(event).map_err(|_| ())?;
        // Per book, not per batch: a shelf sync is slow, and a shell that
        // only heard at the end could not show progress through it.
        waker();
        done += 1;
    }
    Ok(done)
}
