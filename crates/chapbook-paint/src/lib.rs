//! The format-neutral page model and paint-neutral display list — the
//! contract every rasterization backend builds against (tiny-skia now,
//! e-ink or GPU later), and the seam where non-text formats join.
//!
//! This crate owns `Page`/`Fragment` (a laid-out page of positioned content)
//! and `DisplayList`/`DisplayOp` (dumb draw ops: filled rects, borders,
//! positioned glyph runs referencing `fontdb::ID` so no re-shaping happens at
//! paint time, images, decoration lines, clips).
//!
//! Producers, not consumers, are format-specific: `chapbook-layout` paginates
//! styled XHTML into `Page`s; a future `chapbook-cbz` produces one
//! image-fragment `Page` per archive page directly — no DOM, no stylo, no
//! text layout. Fragments carry an opaque `u64` source tag (meaningful only
//! to their producer, e.g. a DOM node mapping held in `ChapterLayout`) so
//! this crate never depends on any document model.
//!
//! Implemented in milestone M4.
