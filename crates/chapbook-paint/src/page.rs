//! The format-neutral page model: a laid-out page of positioned fragments.
//!
//! Producers are format-specific (chapbook-layout paginates styled XHTML; a
//! future comic producer emits one image fragment per page); consumers (the
//! display-list builder, renderers, the viewer) never know the difference.
//! All coordinates are CSS px in page space, origin at the page's top-left.

use chapbook_core::{Rect, Rgba, Size};

/// One laid-out page.
#[derive(Debug, Clone)]
pub struct Page {
    /// Full page size including reader margins.
    pub size: Size,
    /// The content box the fragments were laid into.
    pub content: Rect,
    pub fragments: Vec<Fragment>,
}

/// A positioned piece of page content.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// Bounding rect in page space.
    pub rect: Rect,
    pub kind: FragmentKind,
    /// Opaque producer-defined tag (e.g. a DOM node key); `0` = untagged.
    /// Deliberately not a document-model type — see ARCHITECTURE.md.
    pub tag: u64,
}

#[derive(Debug, Clone)]
pub enum FragmentKind {
    /// One shaped visual line of text.
    Line(LineFragment),
    /// A horizontal rule.
    Rule { color: Rgba },
    /// A raster image, keyed into the producer's resource store (M5).
    Image { resource: u64 },
}

/// A shaped line: glyphs are final — no re-shaping happens downstream.
#[derive(Debug, Clone)]
pub struct LineFragment {
    /// Baseline offset from the top of the fragment rect.
    pub baseline: f32,
    pub runs: Vec<GlyphRun>,
    /// The line's source text (diagnostics, selection, golden dumps).
    pub text: String,
    /// Locator-text char offset of the line start (see `chapbook-core`
    /// locator docs); drives `char_map` and position restore.
    pub locator_start: u32,
}

/// A run of glyphs sharing one font face, size, and color.
#[derive(Debug, Clone)]
pub struct GlyphRun {
    /// Face in the producer's `fontdb` (cosmic-text's database).
    pub font: cosmic_text::fontdb::ID,
    pub font_size: f32,
    pub color: Rgba,
    pub glyphs: Vec<Glyph>,
}

/// One positioned glyph, relative to the line fragment's origin.
#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    pub id: u16,
    pub x: f32,
    /// Offset from the line's baseline (positive = below).
    pub y: f32,
    pub advance: f32,
}
