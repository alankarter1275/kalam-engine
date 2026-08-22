//! Cascade driver: owns stylo's `Stylist` and media `Device`, ships the
//! embedded UA stylesheet (the EPUB 3 CSS profile boundary), registers author
//! sheets from a chapter and user-origin override sheets, and runs the
//! restyle traversal to land `ComputedValues` on each element.
//!
//! Also exposes `@font-face` rules for registration with cosmic-text's font
//! database, and a `FontMetricsProvider` backed by cosmic-text for ex/ch unit
//! resolution.
//!
//! Implemented in milestone M2.
