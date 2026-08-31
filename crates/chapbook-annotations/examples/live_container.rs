//! Take one highlight out to a live Web Annotation container and back:
//! create, read, update under its entity tag, watch a stale tag be
//! refused, list, delete, and confirm the tombstone.
//!
//! The assertion in the middle is the point — what the server hands back
//! has to map to the same `Mark` that went out, offsets and all.
//!
//! Point it at any Web Annotation Protocol container. `CONTAINER` is its
//! IRI, `SOURCE` the publication the marks anchor into, and
//! `CONTAINER_AUTH` an optional `user:password`.
//!
//! It creates and deletes one annotation, so give it a container you do
//! not mind writing to.
//!
//! ```sh
//! CONTAINER=https://library.example.com/annotations/ \
//!   cargo run -p chapbook-annotations --example live_container
//! ```

use chapbook_annotations::{
    from_annotation, to_annotation, AnnotationContainer, ContainerError, Mark,
};
use chapbook_core::{LayeredLocator, Quote, LOCATOR_VERSION};
use chapbook_library::AnnotationKind;

fn locator(offset: u32, prefix: &str, exact: &str, suffix: &str) -> LayeredLocator {
    LayeredLocator {
        spine_href: "OEBPS/chapter4.xhtml".into(),
        spine_index: 3,
        char_offset: offset,
        locator_version: LOCATOR_VERSION,
        quote: Quote {
            prefix: prefix.into(),
            exact: exact.into(),
            suffix: suffix.into(),
        },
        spine_fraction: 0.25,
        book_progression: 0.42,
    }
}

fn main() {
    let container_url =
        std::env::var("CONTAINER").unwrap_or_else(|_| "http://localhost:8080/annotations/".into());
    // What the annotations anchor into. Any IRI naming the publication
    // will do; a catalog would give this as the acquisition link.
    let source = std::env::var("SOURCE").unwrap_or_else(|_| "urn:isbn:9780000000000".into());
    let mut container = AnnotationContainer::with_ureq();
    if let Ok(credential) = std::env::var("CONTAINER_AUTH") {
        let (user, password) = credential
            .split_once(':')
            .expect("CONTAINER_AUTH is user:password");
        container.set_basic_auth(user, password);
    }

    let mark = Mark {
        kind: AnnotationKind::Highlight,
        start: locator(1200, "the harbour was ", "quiet that morning", " and the"),
        end: Some(locator(
            1218,
            "the harbour was ",
            "quiet that morning",
            " and the",
        )),
        text: Some("the chase begins here".into()),
        color: Some("#ffcc00".into()),
        created: Some("2026-08-30T12:00:00Z".into()),
        modified: Some("2026-08-30T12:00:00Z".into()),
    };

    let document = to_annotation(&mark, &source);
    println!(
        "POST body:\n{}\n",
        serde_json::to_string_pretty(&document).unwrap()
    );

    let stored = match container.create(&container_url, &document) {
        Ok(stored) => stored,
        Err(err) => {
            println!("create failed: {err}");
            return;
        }
    };
    println!("created {} etag={:?}", stored.iri, stored.etag);

    // Round trip the mapping through what the server actually stored.
    let back = from_annotation(&stored.annotation);
    println!("\nmapped back: kind={:?} color={:?}", back.kind, back.color);
    println!(
        "  start offset={} v{}",
        back.start.char_offset, back.start.locator_version
    );
    println!("  quote={:?}", back.start.quote);
    println!(
        "  end offset={:?}",
        back.end.as_ref().map(|e| e.char_offset)
    );
    println!("  progression={}", back.start.book_progression);
    println!(
        "  spine={} #{}",
        back.start.spine_href, back.start.spine_index
    );
    assert_eq!(back.start, mark.start, "start locator did not survive");
    assert_eq!(back.end, mark.end, "end locator did not survive");
    assert_eq!(back.color, mark.color);
    assert_eq!(back.text, mark.text);
    assert_eq!(back.kind, mark.kind);
    println!("  LOSSLESS: ok");

    // Read it back, with its entity tag.
    let fetched = container.get(&stored.iri).expect("get");
    println!("\nGET etag={:?}", fetched.etag);

    // Update, guarded.
    let mut edited = fetched.annotation.clone();
    edited.body_value = Some("edited body".into());
    match container.update(&stored.iri, &edited, fetched.etag.as_deref()) {
        Ok(updated) => println!("UPDATE ok, new etag={:?}", updated.etag),
        Err(err) => println!("UPDATE failed: {err}"),
    }

    // Update again with the *stale* tag: must be refused.
    match container.update(&stored.iri, &edited, fetched.etag.as_deref()) {
        Ok(_) => println!("STALE UPDATE: accepted (no concurrency guard!)"),
        Err(ContainerError::Conflict { current }) => println!(
            "STALE UPDATE -> 412 conflict, server copy present: {}",
            current.is_some()
        ),
        Err(err) => println!("STALE UPDATE -> {err}"),
    }

    // List the container.
    match container.all(&container_url, Some(20)) {
        Ok(items) => println!("\nCONTAINER holds {} annotations", items.len()),
        Err(err) => println!("\nLIST failed: {err}"),
    }

    // Delete, then confirm the tombstone.
    match container.delete(&stored.iri, None) {
        Ok(()) => println!("DELETE ok"),
        Err(err) => println!("DELETE failed: {err}"),
    }
    match container.get(&stored.iri) {
        Err(ContainerError::Gone) => println!("GET after delete -> gone, as it should be"),
        Ok(_) => println!("GET after delete -> still there"),
        Err(err) => println!("GET after delete -> {err}"),
    }
}
