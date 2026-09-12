//! Arena-based DOM for EPUB XHTML content documents, and the host-side
//! implementations of stylo's DOM traits (`TNode`, `TDocument`, `TElement`,
//! `selectors::Element`).
//!
//! Shaped by stylo's trait requirements rather than a design of its own: the
//! pinned stylo set upgrades all-at-once as a deliberate task that rewrites
//! this module.
//!
//! Documents are static after parse: no incremental restyle, no snapshots, no
//! shadow DOM, no animations, no scripting. That deletes most of stylo's
//! invalidation surface and keeps the binding small. The structure of the
//! trait impls follows blitz-dom's proven `stylo.rs` binding.
//!
//! Parsing is XML first (xml5ever — EPUB content documents are XML, and
//! the HTML algorithm mis-nests their `<a id="x"/>` shorthand), falling
//! back to lenient html5ever for files that are not well-formed. Both run
//! over the same tree builder; see `parse`.

mod cfi;
mod foreign;
mod math_fallback;
mod offsets;
mod parse;
mod stylo_data;
mod stylo_impls;
mod text;
mod tree;

pub use cfi::{cfi_for_offset, offset_for_cfi};
pub(crate) use foreign::is_svg_root;
pub use foreign::MathSource;
pub use offsets::{links, locator_offset_of, locator_offsets, locator_text, node_tag, Link};
pub use parse::parse_xhtml;
pub use stylo_impls::DomNode;
pub use text::extract_text;
pub use tree::{Document, ElementData, Node, NodeData, NodeId, StylesheetSource};
