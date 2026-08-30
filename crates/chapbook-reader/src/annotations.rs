//! Stored annotations — highlights, notes, bookmarks: capture from the
//! live selection, lazy resolution against unit text, and the layered
//! locators that anchor them. Compiled only with the `library` feature:
//! without a place to store them there is nothing for these to describe.

use chapbook_core::{resolve_in_text, BookKind, LayeredLocator, Locator, Point};
use chapbook_library::AnnotationKind;
use chapbook_paint::FrameIntent;

use crate::text_surface::unit_locator_text;
use crate::Session;

/// A stored highlight resolved into the open book's locator space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// Library annotation id — the handle for [`Session::remove_highlight`].
    pub id: i64,
    pub spine: usize,
    /// Locator offsets within the unit, `[start, end)`.
    pub start: u32,
    pub end: u32,
    /// The text as captured, for a highlight list.
    pub text: Option<String>,
    /// Stored color as written (`#rrggbb`); `None` follows the theme.
    pub color: Option<String>,
}

/// One stored annotation as recorded — enough to list every mark in a book
/// without resolving any of them against unit text.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationSummary {
    pub id: i64,
    pub kind: AnnotationKind,
    /// Unit the annotation resolves against in this book.
    pub spine_index: usize,
    /// Whole-book progression of its start, for ordering and for showing
    /// where in the book it sits.
    pub progression: f64,
    /// The quoted text for a highlight, the body for a note.
    pub text: Option<String>,
    pub color: Option<String>,
}

/// An annotation as the library stores it, plus where it lands in this
/// book's spine. Resolution into offsets waits for the unit's text.
pub(crate) struct StoredAnnotation {
    pub(crate) id: i64,
    pub(crate) kind: AnnotationKind,
    /// Spine item the endpoints resolve against: by href where the book
    /// still has that item, else the stored index.
    pub(crate) target: usize,
    /// The stored href matched a spine item — only then, and only in the
    /// same edition, are the exact offsets trustworthy.
    pub(crate) href_matched: bool,
    pub(crate) start: LayeredLocator,
    /// Ranged kinds (highlights, notes) have an end; a bookmark is a point.
    pub(crate) end: Option<LayeredLocator>,
    pub(crate) text: Option<String>,
    pub(crate) color: Option<String>,
}

/// Book-wide char counts around the current unit — what
/// `LayeredLocator::capture` needs beyond the offset itself.
pub(crate) struct UnitCharContext {
    /// The current unit's locator text.
    pub(crate) text: String,
    /// Chars in the spine items before it.
    pub(crate) prior: u64,
    /// Chars across the whole book.
    pub(crate) total: u64,
}

impl Session {
    // ---- Highlights ----

    /// Persist the current selection as a highlight and return its library
    /// id. `None` without a text selection, without a library (OPDS
    /// streams have no local record), or on a comic — no text layer, so
    /// nothing to anchor to.
    pub fn add_highlight(&mut self) -> Option<i64> {
        self.add_ranged(AnnotationKind::Highlight, None)
    }

    /// Attach a note to the current selection. The quoted text is kept as
    /// the annotation's text, the note body as its own record — a note is a
    /// highlight that says something.
    pub fn add_note(&mut self, body: &str) -> Option<i64> {
        self.add_ranged(AnnotationKind::Note, Some(body))
    }

    /// Bookmark the current page. A point, not a range, so it needs no
    /// selection — but it does need a text unit to anchor in.
    pub fn add_bookmark(&mut self) -> Option<i64> {
        let offset = self.current_offset();
        let (start, _) = self.capture_endpoints(offset, offset)?;
        let book_id = self.book_id?;
        let id = self
            .library
            .as_mut()?
            .add_annotation(book_id, AnnotationKind::Bookmark, &start, None, None, None)
            .map_err(|e| log::error!("failed to save bookmark: {e}"))
            .ok()?;
        let spine = self.spine;
        self.stored.push(StoredAnnotation {
            id,
            kind: AnnotationKind::Bookmark,
            target: spine,
            href_matched: true,
            start,
            end: None,
            text: None,
            color: None,
        });
        self.mark(FrameIntent::Annotation);
        Some(id)
    }

    fn add_ranged(&mut self, kind: AnnotationKind, body: Option<&str>) -> Option<i64> {
        let (start, end) = self.selected_range()?;
        let quote = self.selected_text()?;
        let (start_loc, end_loc) = self.capture_endpoints(start, end)?;
        let book_id = self.book_id?;
        // A note keeps its body; a highlight keeps the words it marks.
        let text = body.unwrap_or(quote.as_str());
        let id = self
            .library
            .as_mut()?
            .add_annotation(book_id, kind, &start_loc, Some(&end_loc), Some(text), None)
            .map_err(|e| log::error!("failed to save annotation: {e}"))
            .ok()?;
        let spine = self.spine;
        let text = Some(text.to_string());
        self.stored.push(StoredAnnotation {
            id,
            kind,
            target: spine,
            href_matched: true,
            start: start_loc,
            end: Some(end_loc),
            text: text.clone(),
            color: None,
        });
        self.mark_range(FrameIntent::Annotation, start, end);
        // Show it immediately: the cache is authoritative once populated.
        self.unit_mut(spine)
            .resolved_highlights
            .get_or_insert_with(Vec::new)
            .push(Highlight {
                id,
                spine,
                start,
                end,
                text,
                color: None,
            });
        Some(id)
    }

    /// Every mark in the book, as stored — no unit text is read, so this
    /// is cheap enough for a list. Ordered by position in the book.
    pub fn annotations(&self) -> Vec<AnnotationSummary> {
        let mut all: Vec<AnnotationSummary> = self
            .stored
            .iter()
            .map(|a| AnnotationSummary {
                id: a.id,
                kind: a.kind,
                spine_index: a.target,
                progression: a.start.book_progression,
                text: a.text.clone(),
                color: a.color.clone(),
            })
            .collect();
        all.sort_by(|a, b| {
            a.progression
                .total_cmp(&b.progression)
                .then(a.id.cmp(&b.id))
        });
        all
    }

    /// Marks that paint in `spine`, resolved into its locator space and
    /// cached. Empty while that text is unavailable — an image book's unit
    /// resolves only once its page has loaded. Bookmarks are points and
    /// paint nothing, so they aren't here.
    pub fn highlights(&mut self, spine: usize) -> &[Highlight] {
        let attempted = self
            .unit(spine)
            .is_some_and(|unit| unit.resolved_highlights.is_some());
        if !attempted {
            // Extracting a unit's text is not free; skip it entirely when
            // nothing is stored against this unit.
            let resolved = if self.stored.iter().any(|a| a.target == spine) {
                let Some(text) = self.unit_text(spine) else {
                    return &[];
                };
                self.resolve_highlights(spine, &text)
            } else {
                Vec::new()
            };
            self.unit_mut(spine).resolved_highlights = Some(resolved);
        }
        self.unit(spine)
            .and_then(|unit| unit.resolved_highlights.as_deref())
            .unwrap_or(&[])
    }

    /// The stored highlight under a point in panel coordinates — what a
    /// tap needs to select, recolor, or delete one by touching it. Only
    /// inside the marked text, like a link.
    pub fn highlight_at(&mut self, x: f32, y: f32) -> Option<i64> {
        let (px, py) = self.metrics.map_or((x, y), |m| m.panel_to_page(x, y));
        let (spine, page) = (self.spine, self.page);
        let offset = self
            .layout_unit(spine)?
            .pages
            .get(page)?
            .offset_at_exact(Point::new(px, py))?;
        self.highlights(spine)
            .iter()
            .find(|h| offset >= h.start && offset < h.end)
            .map(|h| h.id)
    }

    /// Recolor a highlight. `None` hands it back to the theme color.
    /// Colors are `#rgb`, `#rrggbb`, or `#rrggbbaa`.
    pub fn set_highlight_color(&mut self, id: i64, color: Option<&str>) {
        if let Some(library) = self.library_mut() {
            if let Err(e) = library.set_annotation_color(id, color) {
                log::error!("failed to recolor annotation: {e}");
                return;
            }
        }
        let color = color.map(str::to_string);
        for stored in self.stored.iter_mut().filter(|a| a.id == id) {
            stored.color = color.clone();
        }
        for resolved in self
            .units
            .values_mut()
            .filter_map(|unit| unit.resolved_highlights.as_mut())
        {
            for highlight in resolved.iter_mut().filter(|h| h.id == id) {
                highlight.color = color.clone();
            }
        }
        match self.highlight_range(id) {
            Some((start, end)) => self.mark_range(FrameIntent::Annotation, start, end),
            None => self.mark(FrameIntent::Annotation),
        }
    }

    /// Jump to a stored annotation. `false` if its unit can't be read.
    pub fn goto_annotation(&mut self, id: i64) -> bool {
        let Some((target, start)) = self
            .stored
            .iter()
            .find(|a| a.id == id)
            .map(|a| (a.target, a.start.clone()))
        else {
            return false;
        };
        let Some(text) = self.unit_text(target) else {
            return false;
        };
        let trusted = self.same_edition && self.stored.iter().any(|a| a.id == id && a.href_matched);
        let offset = resolve_in_text(&text, &start, trusted).offset();
        self.goto(Locator::new(target, offset))
    }

    /// Delete an annotation (a soft delete in the library, kept for sync).
    pub fn remove_annotation(&mut self, id: i64) {
        if let Some(library) = self.library_mut() {
            if let Err(e) = library.delete_annotation(id) {
                log::error!("failed to delete annotation: {e}");
                return;
            }
        }
        // Its extent has to be read before it is dropped from the cache.
        let range = self.highlight_range(id);
        self.stored.retain(|a| a.id != id);
        for resolved in self
            .units
            .values_mut()
            .filter_map(|unit| unit.resolved_highlights.as_mut())
        {
            resolved.retain(|h| h.id != id);
        }
        match range {
            Some((start, end)) => self.mark_range(FrameIntent::Annotation, start, end),
            None => self.mark(FrameIntent::Annotation),
        }
    }

    fn resolve_highlights(&self, spine: usize, text: &str) -> Vec<Highlight> {
        self.stored
            .iter()
            .filter(|a| a.target == spine && a.kind != AnnotationKind::Bookmark)
            .filter_map(|a| {
                let end_loc = a.end.as_ref()?;
                let trusted = self.same_edition && a.href_matched;
                let start = resolve_in_text(text, &a.start, trusted).offset();
                let end = resolve_in_text(text, end_loc, trusted).offset();
                // A range that collapsed under re-anchoring has nothing
                // left to paint.
                (end > start).then(|| Highlight {
                    id: a.id,
                    spine,
                    start,
                    end,
                    text: a.text.clone(),
                    color: a.color.clone(),
                })
            })
            .collect()
    }

    /// Full layered locators for a selection's endpoints.
    fn capture_endpoints(&self, start: u32, end: u32) -> Option<(LayeredLocator, LayeredLocator)> {
        let href = self
            .book
            .publication()
            .spine_item(self.spine)
            .ok()?
            .href
            .clone();
        match self.book.publication().kind() {
            BookKind::Epub => {
                let ctx = self.unit_char_context();
                let capture = |offset| {
                    LayeredLocator::capture(
                        &href, self.spine, &ctx.text, offset, ctx.prior, ctx.total,
                    )
                };
                Some((capture(start), capture(end)))
            }
            // A PDF page's extracted text is a real locator space, so the
            // within-unit layers are exact — but the whole-book
            // progression stays page-based, as it is for every image book.
            BookKind::Pdf => {
                let text = self.unit_text(self.spine)?;
                let units = self.book.publication().spine().len() as u64;
                let capture = |offset| {
                    let mut loc = LayeredLocator::capture(&href, self.spine, &text, offset, 0, 0);
                    loc.book_progression =
                        chapbook_core::book_progression(self.spine as u64, 0, units);
                    loc
                };
                Some((capture(start), capture(end)))
            }
            BookKind::Comic => None,
        }
    }

    /// Char counts around the current unit. The per-unit counts are a
    /// property of the file — computed once per session (the one remaining
    /// whole-book pass) and reused by every subsequent capture, so saving
    /// a position on suspend stops re-parsing the entire spine.
    pub(crate) fn unit_char_context(&self) -> UnitCharContext {
        let counts = self.char_counts.get_or_init(|| {
            (0..self.book.publication().spine().len())
                .map(|i| {
                    unit_locator_text(self.book.publication(), i)
                        .map(|text| text.chars().count() as u64)
                        .unwrap_or(0)
                })
                .collect()
        });
        UnitCharContext {
            text: self.cached_unit_text(self.spine).unwrap_or_default(),
            prior: counts.iter().take(self.spine).sum(),
            total: counts.iter().sum(),
        }
    }

    /// The extent of a resolved highlight on the current page.
    fn highlight_range(&self, id: i64) -> Option<(u32, u32)> {
        self.unit(self.spine)?
            .resolved_highlights
            .as_ref()?
            .iter()
            .find(|h| h.id == id)
            .map(|h| (h.start, h.end))
    }
}
