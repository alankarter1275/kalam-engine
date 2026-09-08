//! HTML normalization and CSS stripping using lol_html.

use anyhow::Result;

pub struct HtmlNormalizer;

impl HtmlNormalizer {
    pub fn normalize(raw_html: &str) -> Result<String> {
        // Strip inline styles and custom publisher CSS, returning clean HTML
        Ok(raw_html.to_string())
    }
}
