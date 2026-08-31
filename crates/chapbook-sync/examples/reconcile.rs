//! Reconcile a book against live services, end to end: discover them from
//! a catalog entry, import the book, make some local changes, and sync.
//!
//! ```sh
//! cd ~/wksp/mocklib && go run ./cmd/mocklib -addr :8096 -auth reader:secret
//! CATALOG=http://localhost:8096 CATALOG_AUTH=reader:secret \
//!   cargo run -p chapbook-sync --example reconcile
//! ```

use std::sync::Arc;

use chapbook_core::{LayeredLocator, Quote, LOCATOR_VERSION};
use chapbook_library::{AnnotationKind, Library};
use chapbook_opds::progression::Device;
use chapbook_opds::{OpdsClient, UreqHttp};
use chapbook_sync::{targets_of, SyncCommand, SyncEngine, SyncEvent, SyncWorker};

fn main() {
    let base = std::env::var("CATALOG").unwrap_or_else(|_| "http://localhost:8096".into());
    let credential = std::env::var("CATALOG_AUTH").ok();

    // One transport, shared by the catalog browse and by both sync halves.
    let http: Arc<dyn chapbook_opds::HttpClient> = Arc::new(UreqHttp::new());
    let mut catalog = OpdsClient::new(http.clone());
    let authorization = credential.as_deref().map(|c| {
        let (user, password) = c.split_once(':').expect("CATALOG_AUTH is user:password");
        chapbook_opds::basic_authorization(user, password)
    });
    if let Some(authorization) = &authorization {
        catalog.set_authorization(authorization.clone());
    }

    // 1. Find a book and see what it says it syncs with.
    let feed = catalog
        .fetch(&format!("{base}/opds/feed/all"))
        .expect("catalog feed");
    let entry = feed.entries.first().expect("a publication");
    let (progression_url, container_url) = targets_of(entry);
    println!("{:?}", entry.title);
    println!("  progression: {progression_url:?}");
    println!("  annotations: {container_url:?}");
    if progression_url.is_none() && container_url.is_none() {
        println!("\nnothing to sync with — is the server running with -auth?");
        return;
    }

    // 2. Download it and open it the way a shell does. The session is
    // what imports into the library, so it is the only thing that knows
    // which row the book became.
    let dir = std::env::temp_dir().join(format!("chapbook-reconcile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The session opens the default library; point that at our scratch
    // directory so the example leaves nothing behind.
    std::env::set_var("CHAPBOOK_LIBRARY_DIR", &dir);

    let acquisition = entry
        .links
        .iter()
        .find(|l| {
            l.rel
                .iter()
                .any(|r| r.starts_with(chapbook_opds::REL_ACQ_PREFIX))
        })
        .expect("an acquisition link");
    let file = dir.join("book.epub");
    catalog
        .download(&acquisition.href, &file)
        .expect("download");

    let session =
        chapbook_reader::Session::open(file.to_str().unwrap(), chapbook_core::FontSource::host())
            .expect("open the downloaded book");
    let id = session.book_id().expect("the session imported it");
    drop(session);

    // 3. This is the wiring: the book learns where it syncs.
    let mut library = Library::open(&dir).unwrap();
    library
        .set_sync_targets(id, progression_url.as_deref(), container_url.as_deref())
        .unwrap();

    // 4. Read a bit and mark something.
    library.set_position(id, &locator(1200, 0.42)).unwrap();
    library
        .add_annotation(
            id,
            AnnotationKind::Highlight,
            &locator(1200, 0.42),
            Some(&locator(1218, 0.43)),
            Some("the chase begins here"),
            Some("#ffcc00"),
        )
        .unwrap();
    println!(
        "\nbefore: position owes a write = {}, marks owing = {}",
        library.position_needs_push(id).unwrap(),
        library.annotations_needing_push(id).unwrap().len()
    );

    // 5. Sync, on a worker thread.
    let mut engine = SyncEngine::new(
        library,
        http,
        Device {
            id: "urn:uuid:reconcile-example".into(),
            name: "chapbook".into(),
        },
    );
    if let Some(authorization) = authorization {
        engine.set_authorization(authorization);
    }
    let worker = SyncWorker::spawn(engine, Arc::new(|| {}));
    worker.request(SyncCommand::Book(id));

    loop {
        match worker.next_event() {
            Some(SyncEvent::Book(report)) => {
                println!("\nposition: {:?}", report.position);
                println!("marks: {:?}", report.annotations);
            }
            Some(SyncEvent::Failed { reason, .. }) => println!("\nfailed: {reason}"),
            Some(SyncEvent::Finished { books }) => {
                println!("finished {books} book(s)");
                break;
            }
            None => break,
        }
    }
    drop(worker);

    // 6. Nothing should still owe a write.
    let library = Library::open(&dir).unwrap();
    println!(
        "\nafter: position owes a write = {}, marks owing = {}",
        library.position_needs_push(id).unwrap(),
        library.annotations_needing_push(id).unwrap().len()
    );
    println!(
        "stored remote: {:?}",
        library.sync_targets(id).unwrap().remote_modified
    );

    std::fs::remove_dir_all(&dir).ok();
}

fn locator(offset: u32, progression: f64) -> LayeredLocator {
    LayeredLocator {
        spine_href: "OEBPS/chapter4.xhtml".into(),
        spine_index: 3,
        char_offset: offset,
        locator_version: LOCATOR_VERSION,
        quote: Quote {
            prefix: "the harbour was ".into(),
            exact: String::new(),
            suffix: "quiet that morning".into(),
        },
        spine_fraction: progression,
        book_progression: progression,
    }
}
