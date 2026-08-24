//! PDF documents as [`Publication`]s, rasterized via hayro.
//!
//! PDF pages are pre-laid-out, so a PDF reads through the image-per-page
//! path CBZ opened: each page is one spine item and `unit_bytes` returns
//! the page rasterized to PNG at [`RENDER_SCALE`]× its natural size (a
//! fixed-scale raster keeps the `Publication` contract simple; zooming
//! past 2× goes soft — a resolution-aware contract is future work).
//! Rendering is hayro's: pure Rust, CPU-only, with the standard-14 fonts
//! embedded. Encrypted PDFs are rejected at open (a hayro limitation).
//!
//! Rasterizing a cold page takes real CPU time on complex PDFs — the same
//! `unit_bytes` blocking contract as a remote comic page, for a different
//! reason.

use std::path::Path;
use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};

use chapbook_core::{
    BookKind, BookMetadata, ChapbookError, Publication, Resource, Result, SpineItem, TocEntry,
};

/// Raster scale relative to the page's natural (PDF point) dimensions.
pub const RENDER_SCALE: f32 = 2.0;

/// An opened PDF document.
pub struct PdfBook {
    pdf: Pdf,
    interpreter: InterpreterSettings,
    metadata: BookMetadata,
    spine: Vec<SpineItem>,
    toc: Vec<TocEntry>,
}

impl PdfBook {
    pub fn open(path: &Path) -> Result<PdfBook> {
        let open_err = |reason: String| ChapbookError::BookOpen {
            path: path.to_path_buf(),
            reason,
        };
        let data = std::fs::read(path).map_err(|e| open_err(e.to_string()))?;
        let pdf = Pdf::new(Arc::new(data)).map_err(|e| open_err(format!("{e:?}")))?;
        let page_count = pdf.pages().len();
        if page_count == 0 {
            return Err(open_err("PDF has no pages".into()));
        }

        let text = |bytes: &Option<Vec<u8>>| -> Option<String> {
            bytes
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .filter(|s| !s.trim().is_empty())
        };
        let md = pdf.metadata();
        let metadata = BookMetadata {
            title: text(&md.title).or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
            }),
            authors: text(&md.author).into_iter().collect(),
            description: text(&md.subject),
            format_version: "pdf".into(),
            ..BookMetadata::default()
        };
        let spine = (0..page_count)
            .map(|i| SpineItem {
                id: format!("page-{}", i + 1),
                href: format!("page-{}", i + 1),
                media_type: "image/png".into(),
                linear: true,
            })
            .collect();

        Ok(PdfBook {
            pdf,
            interpreter: InterpreterSettings::default(),
            metadata,
            spine,
            toc: Vec::new(),
        })
    }
}

impl Publication for PdfBook {
    fn kind(&self) -> BookKind {
        BookKind::Pdf
    }

    fn metadata(&self) -> &BookMetadata {
        &self.metadata
    }

    fn spine(&self) -> &[SpineItem] {
        &self.spine
    }

    fn toc(&self) -> &[TocEntry] {
        &self.toc
    }

    /// Rasterize one page to PNG on a white ground.
    fn unit_bytes(&self, spine_index: usize) -> Result<Vec<u8>> {
        self.spine_item(spine_index)?;
        let pages = self.pdf.pages();
        let page = pages
            .get(spine_index)
            .ok_or(ChapbookError::SpineOutOfRange(spine_index))?;
        let cache = RenderCache::new();
        let settings = RenderSettings {
            x_scale: RENDER_SCALE,
            y_scale: RENDER_SCALE,
            bg_color: WHITE,
            ..RenderSettings::default()
        };
        let pixmap = hayro::render(page, &cache, &self.interpreter, &settings);
        pixmap
            .into_png()
            .map_err(|e| ChapbookError::BookMalformed(format!("PNG encode: {e}")))
    }

    fn cover(&self) -> Result<Option<Resource>> {
        Ok(Some(Resource {
            media_type: "image/png".into(),
            data: self.unit_bytes(0)?,
        }))
    }
}
