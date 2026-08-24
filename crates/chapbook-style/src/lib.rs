//! Cascade driver: owns stylo's `Stylist` and media `Device`, ships the
//! embedded UA stylesheet (the EPUB 3 CSS profile boundary), registers author
//! sheets from a chapter and user-origin override sheets, and runs the
//! restyle traversal to land `ComputedValues` on each element.
//!
//! Known gaps: `@import` rules are parsed but not fetched (no stylesheet
//! loader — chapbook-epub can inline-resolve them); font metrics for
//! `ex`/`ch` units use stylo's fallback approximations rather than real
//! font tables (see `crate::fonts`). `@font-face` families are surfaced
//! here and registered with the text system by chapbook-layout.

mod dump;
mod engine;
mod fonts;

pub use dump::dump_computed_styles;
pub use engine::{computed, StyleEngine, UA_CSS};
pub use fonts::BookFontMetricsProvider;
