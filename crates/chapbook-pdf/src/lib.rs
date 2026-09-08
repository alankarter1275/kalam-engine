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
//!
//! The document outline becomes the table of contents; see [`outline`].

use std::path::Path;
use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};

mod outline;
mod strings;
mod text;
pub use text::{TextGlyph, TextLine};

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
        let data = std::fs::read(path).map_err(|e| ChapbookError::BookOpen {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        Self::from_bytes_at(data, path)
    }

    /// Open from bytes already in memory.
    ///
    /// The natural constructor for this format, and the one `open` is
    /// written in terms of: hayro takes an `Arc<Vec<u8>>`, so a PDF was
    /// always read whole. Nothing is given up by not having a path except
    /// the title fallback, which is the file stem.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<PdfBook> {
        Self::from_bytes_at(bytes, Path::new(""))
    }

    /// Open from any seekable handle, by reading it. Present for symmetry
    /// with the zip formats; unlike them it buys nothing, because hayro
    /// wants the whole file anyway.
    pub fn read(mut reader: impl chapbook_core::ReadSeek + 'static) -> Result<PdfBook> {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut bytes).map_err(|e| {
            ChapbookError::BookOpen {
                path: std::path::PathBuf::new(),
                reason: e.to_string(),
            }
        })?;
        Self::from_bytes(bytes)
    }

    fn from_bytes_at(data: Vec<u8>, path: &Path) -> Result<PdfBook> {
        let open_err = |reason: String| ChapbookError::BookOpen {
            path: path.to_path_buf(),
            reason,
        };
        let pdf = Pdf::new(Arc::new(data)).map_err(|e| open_err(format!("{e:?}")))?;
        let page_count = pdf.pages().len();
        if page_count == 0 {
            return Err(open_err("PDF has no pages".into()));
        }

        let text = |bytes: &Option<Vec<u8>>| -> Option<String> {
            bytes
                .as_ref()
                .map(|b| strings::text_string(b))
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

        let toc = outline::read(&pdf);
        Ok(PdfBook {
            pdf,
            interpreter: InterpreterSettings::default(),
            metadata,
            spine,
            toc,
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

/// A page rasterized to straight (un-premultiplied) RGBA.
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Natural page size in PDF points (the space [`TextLine`]s live in).
    pub natural: (f32, f32),
}

impl PdfBook {
    /// Natural (point) dimensions of a page after rotation/cropping.
    pub fn page_natural_size(&self, spine_index: usize) -> Option<(f32, f32)> {
        self.pdf
            .pages()
            .get(spine_index)
            .map(|p| p.render_dimensions())
    }

    /// Rasterize a page at [`RENDER_SCALE`] straight to RGBA — the viewer
    /// path (skips the PNG round-trip `unit_bytes` does for the trait).
    pub fn render_page(&self, spine_index: usize) -> Result<RenderedPage> {
        let pages = self.pdf.pages();
        let page = pages
            .get(spine_index)
            .ok_or(ChapbookError::SpineOutOfRange(spine_index))?;
        let natural = page.render_dimensions();
        let cache = RenderCache::new();
        let settings = RenderSettings {
            x_scale: RENDER_SCALE,
            y_scale: RENDER_SCALE,
            bg_color: WHITE,
            ..RenderSettings::default()
        };
        let pixmap = hayro::render(page, &cache, &self.interpreter, &settings);
        let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
        // Premultiplied → straight RGBA (the page ground is opaque white,
        // so alpha is 255 everywhere and the copy is direct).
        let rgba = pixmap.data_as_u8_slice().to_vec();
        Ok(RenderedPage {
            width,
            height,
            rgba,
            natural,
        })
    }

    /// Extract the page's text layer: lines of positioned glyphs in
    /// natural (point) coordinates, reading-ordered top to bottom.
    pub fn text_page(&self, spine_index: usize) -> Result<Vec<TextLine>> {
        let pages = self.pdf.pages();
        let page = pages
            .get(spine_index)
            .ok_or(ChapbookError::SpineOutOfRange(spine_index))?;
        Ok(text::extract(page, &self.interpreter))
    }
}
