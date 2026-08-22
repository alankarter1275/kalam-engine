//! The pagination-first layout engine — chapbook's differentiator.
//!
//! Pipeline: styled DOM → box tree (CSS 2.1 §9.2 anonymous boxes,
//! `::before`/`::after`) → block flow with inline formatting contexts laid out
//! as `cosmic_text::Buffer`s → a streaming page cursor applying CSS
//! fragmentation rules (forced `break-before/after: page` and the legacy
//! `page-break-*` aliases, `break-inside: avoid`, widows/orphans, margins
//! discarded at page boundaries, oversized monolithic boxes sliced
//! graphically) → `ChapterLayout { pages, anchors, char_map }`.
//!
//! One spine item is one layout run; results are cached keyed by page
//! metrics, settings, and stylesheet hashes.
//!
//! The output `Page`/`Fragment` types live in `chapbook-paint` (this crate
//! *produces* into the format-neutral page model, it does not define it), so
//! image-per-page formats can produce pages without the text pipeline.
//!
//! Implemented in milestone M3.
