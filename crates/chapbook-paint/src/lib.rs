//! The format-neutral page model and paint-neutral display list — the
//! contract every rasterization backend builds against (tiny-skia now,
//! e-ink or GPU later), and the seam where non-text formats join.
//!
//! `Page`/`Fragment` are produced by format-specific layout (chapbook-layout
//! for XHTML; image-per-page formats directly). The `DisplayList` flattening
//! and draw ops land in M4.

mod page;

pub use page::{Fragment, FragmentKind, Glyph, GlyphRun, LineFragment, Page};
