//! # kalam-engine
//!
//! Pure-Rust, high-performance text rendering & pagination engine for Kalam (`calibre-alt`).

pub mod container;
pub mod locator;
pub mod normalizer;
pub mod layout;
pub mod painter;

pub use locator::LayeredLocator;

/// Main KalamEngine configuration parameters.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub font_size: f32,
    pub line_height: f32,
    pub font_family: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            font_size: 16.0,
            line_height: 1.4,
            font_family: "Serif".to_string(),
        }
    }
}
