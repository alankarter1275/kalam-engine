//! Arena-based DOM for EPUB XHTML content documents, and (from M2) the
//! host-side implementations of stylo's DOM traits (`TNode`, `TDocument`,
//! `TElement`, `selectors::Element`).
//!
//! Documents are static after parse: no incremental restyle, no snapshots, no
//! shadow DOM, no animations, no scripting. That deletes most of stylo's
//! invalidation surface and keeps the binding small. The structure of the
//! trait impls follows blitz-dom's proven `stylo.rs` binding.
//!
//! Parsing is lenient html5ever by default — real-world EPUBs contain
//! HTML-isms that strict XML parsing rejects. A `strict-xml` feature (M2+)
//! runs xml5ever over the same tree builder.

mod offsets;
mod parse;
mod text;
mod tree;

pub use offsets::{locator_offset_of, locator_text};
pub use parse::parse_xhtml;
pub use text::extract_text;
pub use tree::{Document, ElementData, Node, NodeData, NodeId, StylesheetSource};
