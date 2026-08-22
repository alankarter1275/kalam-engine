//! Paint-neutral display list: the contract every rasterization backend
//! builds against (tiny-skia now, e-ink or GPU later).
//!
//! Ops are deliberately dumb — filled rects, borders, positioned glyph runs
//! (referencing `fontdb::ID` so no re-shaping happens at paint time), images,
//! decoration lines, clips. `build_display_list` flattens a laid-out `Page`
//! into one.
//!
//! Implemented in milestone M4.
