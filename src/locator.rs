use serde::{Deserialize, Serialize};

/// Multi-layered position tracker adapted from quote-anchored locator patterns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayeredLocator {
    /// Chapter relative HREF within the EPUB container
    pub chapter_href: String,
    /// Exact quote text anchor at reading position
    pub text_quote: String,
    /// DOM node path fallback anchor
    pub dom_path: Option<String>,
    /// Relative percentage progress (0.0 to 1.0)
    pub percentage: f32,
}

impl LayeredLocator {
    pub fn new(chapter_href: impl Into<String>, text_quote: impl Into<String>, percentage: f32) -> Self {
        Self {
            chapter_href: chapter_href.into(),
            text_quote: text_quote.into(),
            dom_path: None,
            percentage,
        }
    }
}
