//! The text surface: search, selection, and the page's text with
//! geometry — text runs, the speakable page, words under a point. See
//! the module doc in `lib.rs`.
//!
//! Built over `LineFragment` text and per-glyph locators, deliberately
//! *not* the display list. The display list carries no text: a
//! `GlyphRun` holds font glyph indices, and recovering characters from
//! those means reversing the font's cmap, which is lossy in exactly the
//! cases that matter — an `fi` ligature is one glyph for two characters,
//! contextual Arabic forms collapse several glyphs to one letter, and
//! small-caps or oldstyle variants alias. A screen reader fed that reads
//! the *wrong* text, which is worse than reading none. The runs are also
//! a much smaller thing to hold still across the C ABI than the paint
//! vocabulary: no font ids, no colours, no glyph arrays.

use chapbook_core::{BookKind, Locator, Point, Publication, Rect};
use chapbook_layout::dom;
use chapbook_paint::FrameIntent;

use crate::{SearchHit, Session, SpeakablePage, TextRun, WordSpan};

impl Session {
    // ---- Search ----

    /// Search one unit. The building block: a shell wanting the whole book
    /// without blocking drives this unit by unit on a worker.
    ///
    /// Matching is case-insensitive by simple per-character lowercasing —
    /// no full case folding, no diacritic folding — which keeps every hit
    /// on an exact char offset in the unit's locator text.
    pub fn search_unit(&mut self, spine: usize, query: &str) -> Vec<SearchHit> {
        let needle: Vec<char> = query.chars().map(fold_char).collect();
        if needle.is_empty() {
            return Vec::new();
        }
        let Some(text) = self.unit_text(spine) else {
            return Vec::new();
        };
        let chars: Vec<char> = text.chars().collect();
        let folded: Vec<char> = chars.iter().copied().map(fold_char).collect();
        let mut hits = Vec::new();
        let mut at = 0;
        while at + needle.len() <= folded.len() {
            if folded[at..at + needle.len()] == needle[..] {
                let (start, end) = (at as u32, (at + needle.len()) as u32);
                let (context, match_range) = context_around(&chars, start, end);
                hits.push(SearchHit {
                    locator: Locator::new(spine, start),
                    end,
                    context,
                    match_range,
                });
                at += needle.len();
            } else {
                at += 1;
            }
        }
        hits
    }

    /// Search the whole book, stopping at `limit` hits.
    ///
    /// **Blocking:** this reads and extracts every unit's text, which for a
    /// long book is seconds — the same contract as
    /// [`chapbook_core::Publication::unit_bytes`], and the same rule: not
    /// on a UI thread. Comics contribute nothing, having no text layer, and
    /// a PDF page contributes only once the loader has decoded it — its
    /// text layer comes out of the same pass as its pixels.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<SearchHit> {
        let mut hits = Vec::new();
        for spine in 0..self.book.publication().spine().len() {
            if hits.len() >= limit {
                break;
            }
            let mut unit = self.search_unit(spine, query);
            unit.truncate(limit - hits.len());
            hits.append(&mut unit);
        }
        hits
    }

    // ---- Selection ----

    /// Begin a selection at a point in panel coordinates (CSS px, and
    /// identical to page space unless the metrics carry a rotation).
    /// Returns whether the point hit text.
    pub fn selection_begin(&mut self, x: f32, y: f32) -> bool {
        self.selection = None;
        self.mark(FrameIntent::Selection);
        let Some(offset) = self.offset_at(x, y) else {
            return false;
        };
        self.selection = Some((offset, offset));
        true
    }

    /// Extend the selection to a point (drag). No-op without an anchor.
    pub fn selection_drag(&mut self, x: f32, y: f32) {
        let Some((anchor, _)) = self.selection else {
            return;
        };
        if let Some(offset) = self.offset_at(x, y) {
            self.selection = Some((anchor, offset));
            self.mark(FrameIntent::Selection);
        }
    }

    /// Select a locator range directly — what a shell does to show where a
    /// search hit or an annotation sits on the page.
    pub fn select_range(&mut self, start: u32, end: u32) {
        self.selection = Some((start, end));
        self.mark(FrameIntent::Selection);
    }

    pub fn selection_clear(&mut self) {
        self.selection = None;
        self.mark(FrameIntent::Selection);
    }

    /// The selected locator range `[start, end)`, when non-empty.
    pub fn selected_range(&self) -> Option<(u32, u32)> {
        let (a, b) = self.selection?;
        let (start, end) = (a.min(b), a.max(b));
        (end > start).then_some((start, end))
    }

    /// The selection's text, ready to paste.
    ///
    /// EPUB units slice the unit's locator text — the space the offsets
    /// are defined in. PDF units read their hidden text layer instead,
    /// one entry per extracted line. Comics have no text layer.
    ///
    /// Locator text is the raw offset space, so it still carries the
    /// source document's line breaks and indentation; runs of ASCII
    /// whitespace collapse to a single space here (non-breaking spaces are
    /// content and survive).
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selected_range()?;
        let raw: String = match self.book.publication().kind() {
            BookKind::Epub => {
                let unit = self.cached_unit_text(self.spine)?;
                unit.chars()
                    .skip(start as usize)
                    .take((end - start) as usize)
                    .collect()
            }
            BookKind::Pdf => self.hidden_text_in_range(start, end)?,
            BookKind::Comic => return None,
        };
        let text = raw.split_ascii_whitespace().collect::<Vec<_>>().join(" ");
        (!text.is_empty()).then_some(text)
    }

    /// A PDF unit's hidden text over a locator range. Line breaks aren't
    /// part of that locator space — the extracted lines are contiguous in
    /// it — so they are re-inserted between the sliced lines here.
    fn hidden_text_in_range(&self, start: u32, end: u32) -> Option<String> {
        let layout = self.layout(self.spine)?;
        let mut lines = Vec::new();
        for page in &layout.pages {
            for fragment in &page.fragments {
                let chapbook_paint::FragmentKind::HiddenText(line) = &fragment.kind else {
                    continue;
                };
                let line_end = line.locator_start + line.text.chars().count() as u32;
                if line_end <= start || line.locator_start >= end {
                    continue;
                }
                let from = start.saturating_sub(line.locator_start) as usize;
                let to = (end.min(line_end) - line.locator_start) as usize;
                lines.push(line.text.chars().take(to).skip(from).collect::<String>());
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn offset_at(&mut self, x: f32, y: f32) -> Option<u32> {
        // Input is in panel space; hit-testing happens in page space.
        let (x, y) = self.content_point(x, y);
        let (spine, page) = (self.spine, self.page);
        let layout = self.layout_unit(spine)?;
        layout.pages.get(page)?.offset_at(Point::new(x, y))
    }

    // ---- The text surface ----

    /// The current page's text runs in reading order — what an
    /// accessibility tree, TTS, or a selection loupe consumes.
    ///
    /// `None` until the page is laid out; `Some` but empty for a laid-out
    /// page with nothing to speak (a comic, an image-only page). One run
    /// per visual line, PDF hidden-text lines included; runs with no text
    /// (a formula whose publisher shipped no alttext) are skipped, since
    /// there is nothing to read aloud for them.
    ///
    /// Reads only what is already laid out — never the loader thread — so
    /// it is safe on the UI thread.
    pub fn page_text_runs(&self) -> Option<Vec<TextRun>> {
        let page = self.layout(self.spine)?.pages.get(self.page)?;
        let mut runs = Vec::new();
        for fragment in &page.fragments {
            use chapbook_paint::FragmentKind;
            let (line, contiguous) = match &fragment.kind {
                FragmentKind::Line(line) => (line, false),
                // PDF extracted lines are contiguous in their locator
                // space, so text length is the span's exact width.
                FragmentKind::HiddenText(line) => (line, true),
                _ => continue,
            };
            if line.text.is_empty() {
                continue;
            }
            let locator_end = if contiguous {
                line.locator_start + line.text.chars().count() as u32
            } else {
                // Shaped text diverges from locator text (collapse, soft
                // hyphens, generated marks), so the end comes from the
                // glyphs. Generated glyphs repeat a neighbor's offset,
                // which keeps the max inside the line's real range; a
                // formula's atomic locator degenerates to one char, which
                // is what "atomic" means.
                line.runs
                    .iter()
                    .flat_map(|run| run.glyphs.iter())
                    .map(|glyph| glyph.locator + 1)
                    .max()
                    .map_or(line.locator_start, |end| end.max(line.locator_start))
            };
            runs.push(TextRun {
                text: line.text.clone(),
                rect: fragment.rect,
                locator_start: line.locator_start,
                locator_end,
            });
        }
        Some(runs)
    }

    /// Page-space rects covering a locator range on the current page — one
    /// per line the range touches. Empty when the page is not laid out or
    /// the range lies elsewhere. The geometry TTS word highlighting and an
    /// accessibility tree ask for; map with
    /// [`chapbook_core::PageMetrics::page_to_panel`] under rotation.
    pub fn range_rects(&self, start: u32, end: u32) -> Vec<Rect> {
        self.layout(self.spine)
            .and_then(|layout| layout.pages.get(self.page))
            .map(|page| page.rects_for_range(start, end))
            .unwrap_or_default()
    }

    /// The current page as a TTS engine wants it: one collapsed string
    /// plus the word table mapping speech progress back to locator space.
    ///
    /// Words are segmented in locator space — the space every offset here
    /// already lives in — so a span's locator range feeds
    /// [`Session::range_rects`] and [`Session::select_range`] directly.
    /// `None` until the page is laid out; empty for a page with nothing to
    /// speak.
    pub fn speakable_page(&self) -> Option<SpeakablePage> {
        self.speakable_unit_page(self.spine, self.page)
    }

    /// [`Session::speakable_page`] for any laid-out page, not only the
    /// current one. kalam: what a scrolling shell's tap-to-look-up needs,
    /// since the page under the finger need not be the session's.
    pub(crate) fn speakable_unit_page(
        &self,
        spine: usize,
        page_idx: usize,
    ) -> Option<SpeakablePage> {
        let layout = self.layout(spine)?;
        let current = layout.pages.get(page_idx)?;
        let mut page = SpeakablePage::default();
        let (mut out_len, mut pending_space) = (0u32, false);
        match self.book.publication().kind() {
            BookKind::Epub => {
                let unit = self.cached_unit_text(spine)?;
                let start = *layout.char_map.get(page_idx)? as usize;
                let end = layout
                    .char_map
                    .get(page_idx + 1)
                    .map(|&e| e as usize)
                    .unwrap_or_else(|| unit.chars().count());
                let slice: String = unit
                    .chars()
                    .skip(start)
                    .take(end.saturating_sub(start))
                    .collect();
                append_speakable(
                    &slice,
                    start as u32,
                    &mut page,
                    &mut out_len,
                    &mut pending_space,
                );
            }
            BookKind::Pdf => {
                // Extracted lines are the PDF's whole text surface, and
                // each line knows its own locator start.
                for fragment in &current.fragments {
                    let chapbook_paint::FragmentKind::HiddenText(line) = &fragment.kind else {
                        continue;
                    };
                    if out_len > 0 {
                        pending_space = true;
                    }
                    append_speakable(
                        &line.text,
                        line.locator_start,
                        &mut page,
                        &mut out_len,
                        &mut pending_space,
                    );
                }
            }
            BookKind::Comic => {}
        }
        Some(page)
    }

    /// The word under a point in panel coordinates, as a locator range —
    /// dictionary lookup's question. `None` off text, and on whitespace or
    /// bare punctuation: a tap on a comma looks nothing up.
    pub fn word_at(&mut self, x: f32, y: f32) -> Option<(u32, u32)> {
        let offset = self.offset_at(x, y)?;
        let page = self.speakable_page()?;
        page.words
            .iter()
            .find(|word| word.locator_start <= offset && offset < word.locator_end)
            .map(|word| (word.locator_start, word.locator_end))
    }

    /// Select the word under a point — the word-boundary tap a dictionary
    /// popup starts from. Returns whether a word was there.
    pub fn select_word_at(&mut self, x: f32, y: f32) -> bool {
        let Some((start, end)) = self.word_at(x, y) else {
            return false;
        };
        self.select_range(start, end);
        true
    }

    /// A unit's locator text — the space its selection offsets live in.
    /// EPUB chapters extract it on demand; a PDF page's is its text layer,
    /// contiguous across the extracted lines, and only exists once the
    /// page has loaded.
    pub(crate) fn unit_text(&self, spine: usize) -> Option<String> {
        match self.book.publication().kind() {
            BookKind::Epub => self.cached_unit_text(spine),
            BookKind::Pdf => {
                let unit = self.unit(spine)?.loaded.as_ref()?;
                Some(unit.text.iter().map(|line| line.text.as_str()).collect())
            }
            BookKind::Comic => None,
        }
    }

    /// A unit's locator text through the one-entry cache. Selection,
    /// capture, search, and highlight resolution ask for the same unit in
    /// bursts, and every miss is an inflate + parse + walk.
    pub(crate) fn cached_unit_text(&self, spine: usize) -> Option<String> {
        if let Some((cached_spine, text)) = self.unit_text_cache.borrow().as_ref() {
            if *cached_spine == spine {
                return Some(text.clone());
            }
        }
        let text = unit_locator_text(self.book.publication(), spine)?;
        self.unit_text_cache.replace(Some((spine, text.clone())));
        Some(text)
    }
}

/// Locator text of a text unit; `None` for image units (comics), which
/// pushes the restore chain to its progression tiers.
pub(crate) fn unit_locator_text(book: &dyn Publication, spine: usize) -> Option<String> {
    if book.kind() != BookKind::Epub {
        return None;
    }
    let href = book.spine_item(spine).ok()?.href.clone();
    let bytes = book.unit_bytes(spine).ok()?;
    let doc = dom::parse_xhtml(&bytes, &href).ok()?;
    Some(dom::locator_text(&doc))
}

/// Append one stretch of locator text to a speakable page under
/// construction. The string and the word table come out of the same pass,
/// so the whitespace collapse can never disagree with the spans: runs of
/// ASCII whitespace become one space (a non-breaking space is content and
/// survives, as in [`Session::selected_text`]); soft hyphens drop from
/// the spoken text while the span keeps their locator positions; a
/// segment carrying an alphanumeric becomes a [`WordSpan`], and bare
/// punctuation is spoken but is nobody's word.
///
/// `out_len` is the running char count of `page.text` and `pending_space`
/// the collapse state, both owned by the caller so multiple stretches (a
/// PDF's extracted lines) build one page.
fn append_speakable(
    slice: &str,
    locator_base: u32,
    page: &mut SpeakablePage,
    out_len: &mut u32,
    pending_space: &mut bool,
) {
    use unicode_segmentation::UnicodeSegmentation;
    let mut offset = locator_base;
    for segment in slice.split_word_bounds() {
        let segment_chars = segment.chars().count() as u32;
        if segment.chars().all(|c| c.is_ascii_whitespace()) {
            // Collapse runs, and never lead with one.
            *pending_space = *out_len > 0;
            offset += segment_chars;
            continue;
        }
        if *pending_space {
            page.text.push(' ');
            *out_len += 1;
            *pending_space = false;
        }
        let text_start = *out_len;
        for c in segment.chars() {
            if c != '\u{AD}' {
                page.text.push(c);
                *out_len += 1;
            }
        }
        if *out_len > text_start && segment.chars().any(char::is_alphanumeric) {
            page.words.push(WordSpan {
                text_start,
                text_end: *out_len,
                locator_start: offset,
                locator_end: offset + segment_chars,
            });
        }
        offset += segment_chars;
    }
}

/// Lowercase a char one-to-one, so folded offsets still index the original
/// text. Multi-char expansions (ß → ss) keep their first char, which costs
/// a rare miss and buys exact offsets.
fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Text around a match with its whitespace collapsed, plus where the match
/// sits inside it. Locator text is raw source text — newlines and XHTML
/// indentation and all — so a results list needs it tidied.
fn context_around(chars: &[char], start: u32, end: u32) -> (String, (u32, u32)) {
    const CONTEXT_CHARS: usize = 40;
    let from = (start as usize).saturating_sub(CONTEXT_CHARS);
    let to = (end as usize + CONTEXT_CHARS).min(chars.len());

    let mut context = String::new();
    let (mut match_start, mut match_end) = (0u32, 0u32);
    let mut pending_space = false;
    for (i, c) in chars[from..to].iter().enumerate() {
        let at = from + i;
        let here = |context: &String| context.chars().count() as u32;
        if c.is_ascii_whitespace() {
            // A match ending at whitespace ends before the space that
            // collapsing may or may not emit.
            if at == end as usize && match_end == 0 {
                match_end = here(&context);
            }
            // Collapse runs, and never lead with one.
            pending_space = !context.is_empty();
            continue;
        }
        if pending_space {
            context.push(' ');
            pending_space = false;
        }
        // Both boundaries are recorded after any collapsed space is
        // emitted, so they index the string that comes back.
        if at == start as usize {
            match_start = here(&context);
        }
        if at == end as usize && match_end == 0 {
            match_end = here(&context);
        }
        context.push(*c);
    }
    if match_end == 0 {
        match_end = context.chars().count() as u32;
    }
    (context, (match_start, match_end))
}
