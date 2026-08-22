//! CPU rasterization of chapbook display lists into `tiny_skia::Pixmap`s.
//!
//! Glyphs are rasterized through cosmic-text's `SwashCache` (alpha masks
//! blended at subpixel-quantized positions). All layout happens in CSS px;
//! hidpi scaling is applied here and only here.
//!
//! Implemented in milestone M4.
