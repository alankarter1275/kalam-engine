//! Cascade driver: owns stylo's `Stylist` and media `Device`, ships the
//! embedded UA stylesheet (the EPUB 3 CSS profile boundary), registers author
//! sheets from a chapter and user-origin override sheets, and runs the
//! restyle traversal to land `ComputedValues` on each element.
//!
//! Known gaps at M2, tracked for later milestones: `@import` rules are parsed
//! but not fetched (no stylesheet loader — chapbook-epub can inline-resolve
//! them); `@font-face` families are surfaced but not yet registered with the
//! text system (M5); font metrics for `ex`/`ch` units use stylo's fallback
//! approximations until the cosmic-text-backed provider lands with layout
//! (M3).

mod dump;
mod engine;
mod fonts;

pub use dump::dump_computed_styles;
pub use engine::{computed, StyleEngine, UA_CSS};
pub use fonts::BookFontMetricsProvider;
