//! EPUB container parsing module using zip and quick-xml.

use anyhow::Result;

pub struct EpubContainer {
    pub title: String,
    pub chapters: Vec<String>,
}

impl EpubContainer {
    pub fn open(_path: &std::path::Path) -> Result<Self> {
        Ok(Self {
            title: "Demo Book".to_string(),
            chapters: Vec::new(),
        })
    }
}
