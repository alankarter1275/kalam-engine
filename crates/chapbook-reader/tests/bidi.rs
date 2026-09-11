//! Bidirectional text, end to end: shaping, visual order, alignment,
//! hit-testing and selection geometry over Hebrew and Arabic.
//!
//! This suite exists because none of it was ever checked. cosmic-text does
//! the Unicode Bidirectional Algorithm, so the reordering was assumed to
//! work — and it does. Everything chapbook wrapped around it did not.
//! The one fixture that sounded like coverage was `rtl.epub` — English
//! prose with an RTL page-progression direction and no Hebrew or Arabic in
//! it at all. It tests which way pages turn, and is called
//! `page-direction.epub` now so it stops claiming otherwise.
//!
//! Three defects were found the first time a Hebrew paragraph was actually
//! laid out, each with a regression test below:
//!
//! - **Every RTL paragraph was flush against the wrong margin.** Nothing
//!   read the `direction` property, and `text-align: start` — the initial
//!   value — mapped unconditionally to `Align::Left`.
//! - **Hit-testing on an RTL line always answered with the end of the
//!   line.** Glyphs are stored in logical order and positioned visually;
//!   the caret search read "later in the list" as "further right", which
//!   inside an RTL run is backwards. Selecting Hebrew was impossible and a
//!   tap reported the wrong word.
//! - **A selection could paint over text it had not selected.** Highlight
//!   geometry took the leftmost and rightmost selected glyph on each line,
//!   which is one rect — but a logically contiguous range in bidi text can
//!   be two visually separate pieces with unselected letters between them.
//!
//! The fixture's own comments say what each paragraph is for.

use std::path::PathBuf;

use chapbook_core::{
    EdgeSizes, Faces, FallbackFamilies, Fallbacks, FontSource, PageMetrics, Rotation, ScriptTag,
    Size, Theme,
};
use chapbook_paint::DisplayOp;
use chapbook_reader::{Session, SessionConfig};

// ---- harness ----

const PAGE: Size = Size { w: 600.0, h: 800.0 };
const MARGIN: f32 = 40.0;

fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// Crimson Text for Latin, plus the vendored Hebrew and Arabic faces
/// reached through per-script fallback.
///
/// Named per script rather than dropped into `fixtures/fonts`: that
/// directory is scanned recursively and pinned at four faces, and a
/// Hebrew face landing in it would change what every other fixture falls
/// back to. `Fallbacks::None` is why this has to be explicit at all — the
/// embedded source deliberately has no fallback, so without this every
/// character here would render as tofu and the whole suite would pass
/// while proving nothing.
fn bidi_fonts() -> FontSource {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut source = FontSource::embedded(base.join("fonts"), "Crimson Text");
    source.faces.push(Faces::Dir(base.join("fonts-bidi")));
    source.fallback = Fallbacks::Explicit(FallbackFamilies {
        common: Vec::new(),
        per_script: vec![
            (
                ScriptTag::new("Hebr").unwrap(),
                vec!["Noto Sans Hebrew".into()],
            ),
            (
                ScriptTag::new("Arab").unwrap(),
                vec!["Noto Naskh Arabic".into()],
            ),
        ],
        forbidden: Vec::new(),
    });
    source
}

fn open(_name: &str) -> Session {
    let config = SessionConfig::new(bidi_fonts());
    let mut session =
        Session::open_with(fixture("epub/bidi.epub"), config).expect("open the bidi fixture");
    session.set_metrics(PageMetrics {
        size: PAGE,
        margins: EdgeSizes::uniform(MARGIN),
        dpi_scale: 1.0,
        rotation: Rotation::None,
    });
    for _ in 0..200 {
        session.render().expect("render");
        if !session.has_pending_loads() {
            session.poll_loaded();
            session.render().expect("render");
            return session;
        }
        session.poll_loaded();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the unit never finished loading");
}

/// One laid-out paragraph, found by how its text begins so the tests do
/// not carry locator numbers that any edit to the fixture would move.
struct Para {
    text: String,
    start: u32,
    end: u32,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

impl Para {
    /// The locator offset of the `n`th character of this paragraph.
    fn at(&self, n: usize) -> u32 {
        self.start + n as u32
    }

    /// Where a character sits in the paragraph, by its char index.
    fn index_of(&self, needle: &str) -> usize {
        let byte = self.text.find(needle).expect("substring is in the text");
        self.text[..byte].chars().count()
    }

    fn mid_y(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

fn para(session: &Session, starts_with: &str) -> Para {
    let run = session
        .page_text_runs()
        .expect("laid out")
        .into_iter()
        .find(|r| r.text.starts_with(starts_with))
        .unwrap_or_else(|| panic!("no paragraph beginning {starts_with:?} on the page"));
    Para {
        text: run.text,
        start: run.locator_start,
        end: run.locator_end,
        left: run.rect.origin.x,
        right: run.rect.origin.x + run.rect.size.w,
        top: run.rect.origin.y,
        bottom: run.rect.origin.y + run.rect.size.h,
    }
}

/// Every glyph whose locator falls in `[start, end)`, in logical order,
/// with `x` in page space.
fn glyphs(session: &mut Session, start: u32, end: u32) -> Vec<(u32, f32, f32, u16)> {
    let frame = session.frame().expect("a frame");
    let mut out = Vec::new();
    for op in &frame.list.ops {
        let DisplayOp::GlyphRun { origin, glyphs, .. } = op else {
            continue;
        };
        for g in glyphs {
            if g.locator >= start && g.locator < end {
                out.push((g.locator, origin.x + g.x, g.advance, g.id));
            }
        }
    }
    out.sort_by_key(|&(locator, ..)| locator);
    out
}

/// The words the text surface reports inside one paragraph, in logical
/// order.
fn words_in(session: &mut Session, p: &Para) -> Vec<(u32, u32)> {
    session
        .speakable_page()
        .expect("a speakable page")
        .words
        .iter()
        .filter(|w| w.locator_start >= p.start && w.locator_end <= p.end)
        .map(|w| (w.locator_start, w.locator_end))
        .collect()
}

/// The horizontal centre of a whole word, in page space.
///
/// Deliberately not the centre of a single glyph. A point exactly on a
/// glyph's midpoint is the one place the caret rule is genuinely
/// ambiguous — it is the boundary between "this offset" and "the next
/// one" — and which side it lands on there comes down to the last bit of
/// a float. A word is several glyphs wide, so its centre is nowhere near
/// that boundary.
fn word_centre(session: &mut Session, start: u32, end: u32) -> f32 {
    let g = glyphs(session, start, end);
    assert!(!g.is_empty(), "no glyphs render {start}..{end}");
    let left = g.iter().map(|&(_, x, ..)| x).fold(f32::INFINITY, f32::min);
    let right = g
        .iter()
        .map(|&(_, x, w, _)| x + w)
        .fold(f32::NEG_INFINITY, f32::max);
    (left + right) / 2.0
}

/// The selection fills the shell would paint, as (left, right) in page
/// space, for the line at `y`.
fn selection_spans(session: &mut Session, y: f32) -> Vec<(f32, f32)> {
    let selection = Theme::default().selection();
    let frame = session.frame().expect("a frame");
    let mut spans: Vec<(f32, f32)> = frame
        .list
        .ops
        .iter()
        .filter_map(|op| match op {
            DisplayOp::Band { rect, color, .. } if *color == selection => Some(*rect),
            _ => None,
        })
        .filter(|r| r.origin.y <= y && y <= r.origin.y + r.size.h)
        .map(|r| (r.origin.x, r.origin.x + r.size.w))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    spans
}

// ---- the fonts are actually there ----

/// Without this the rest of the suite could pass over tofu. `Fallbacks`
/// on the fixture source is `None` by default, so a missing per-script
/// mapping is silent: the text still lays out, still has offsets, still
/// hit-tests. It is just not Hebrew.
#[test]
fn hebrew_and_arabic_reach_the_page_as_themselves() {
    let mut session = open("fonts");
    for starts_with in ["שלום עולם", "مرحبا"] {
        let p = para(&session, starts_with);
        let glyphs = glyphs(&mut session, p.start, p.end);
        assert!(!glyphs.is_empty(), "{starts_with}: nothing was laid out");
        let notdef = glyphs.iter().filter(|&&(.., id)| id == 0).count();
        assert_eq!(
            notdef,
            0,
            "{starts_with}: {notdef} of {} glyphs are .notdef — the face has no coverage",
            glyphs.len()
        );
    }
}

/// Arabic letters take initial, medial, final and isolated forms, and
/// choosing between them is contextual substitution the shaper has to
/// run. It is a 1:1 mapping, so a glyph *count* proves nothing — the
/// evidence is that one character comes out as different glyphs in
/// different places.
#[test]
fn arabic_letters_take_their_joining_forms() {
    let mut session = open("joining");
    let p = para(&session, "مرحبا");
    let glyphs = glyphs(&mut session, p.start, p.end);

    // Beh appears four times in this paragraph, in three different joining
    // contexts: medial in مرحبا, initial in بالعالم, isolated after the
    // non-joining alef of كتاب.
    let ids: Vec<u16> = p
        .text
        .chars()
        .enumerate()
        .filter(|&(_, ch)| ch == 'ب')
        .filter_map(|(i, _)| {
            let locator = p.at(i);
            glyphs
                .iter()
                .find(|&&(l, ..)| l == locator)
                .map(|&(.., id)| id)
        })
        .collect();
    assert!(
        ids.len() >= 3,
        "expected several occurrences of beh, found {}",
        ids.len()
    );
    let distinct: std::collections::BTreeSet<u16> = ids.iter().copied().collect();
    assert!(
        distinct.len() >= 3,
        "beh came out as {} distinct glyph(s) {distinct:?} across {} occurrences — \
         it is not being joined, only mapped",
        distinct.len(),
        ids.len()
    );
}

// ---- visual order ----

#[test]
fn an_rtl_line_runs_right_to_left() {
    let mut session = open("order");
    let p = para(&session, "שלום עולם");
    let glyphs = glyphs(&mut session, p.start, p.end);
    for pair in glyphs.windows(2) {
        let [(a, ax, ..), (b, bx, ..)] = pair else {
            unreachable!()
        };
        assert!(
            bx < ax,
            "locator {a} is at x={ax:.1} and the later {b} at x={bx:.1}: \
             reading forward moved right on a right-to-left line"
        );
    }
}

/// The other half of the same claim: an LTR island inside RTL text keeps
/// its own direction rather than inheriting the paragraph's.
#[test]
fn an_ltr_island_inside_rtl_text_still_reads_left_to_right() {
    let mut session = open("island");
    let p = para(&session, "שלום EPUB");
    let e = p.index_of("EPUB");
    let island = glyphs(&mut session, p.at(e), p.at(e + 4));
    assert_eq!(island.len(), 4, "EPUB should be four glyphs");
    for pair in island.windows(2) {
        let [(a, ax, ..), (b, bx, ..)] = pair else {
            unreachable!()
        };
        assert!(
            bx > ax,
            "inside EPUB, locator {a} at x={ax:.1} is not left of {b} at x={bx:.1}"
        );
    }
    // And the island sits between the two Hebrew words, not beside them.
    let hebrew_before = glyphs(&mut session, p.start, p.at(e - 1));
    let hebrew_after = glyphs(&mut session, p.at(e + 5), p.end);
    let leftmost_of_first = hebrew_before
        .iter()
        .map(|&(_, x, ..)| x)
        .fold(f32::INFINITY, f32::min);
    let rightmost_of_last = hebrew_after
        .iter()
        .map(|&(_, x, w, _)| x + w)
        .fold(f32::NEG_INFINITY, f32::max);
    let island_left = island.first().unwrap().1;
    let island_right = island.last().unwrap().1 + island.last().unwrap().2;
    assert!(
        rightmost_of_last <= island_left && island_right <= leftmost_of_first,
        "the first Hebrew word should be rightmost and the last leftmost, \
         with EPUB between them"
    );
}

// ---- alignment ----

/// The regression test for the first defect. `start` is the initial value
/// of `text-align`, and it is logical: in an RTL paragraph it means the
/// right edge. Every Hebrew and Arabic paragraph used to sit against the
/// left margin with a ragged right edge.
#[test]
fn an_rtl_paragraph_is_flush_right() {
    let session = open("align");
    let content_right = PAGE.w - MARGIN;
    for starts_with in ["שלום עולם", "مرحبا", "שלום EPUB"] {
        let p = para(&session, starts_with);
        assert!(
            (p.right - content_right).abs() < 1.0,
            "{starts_with}: right edge at {:.1}, content box ends at {content_right:.1}",
            p.right
        );
        assert!(
            p.left > MARGIN + 1.0,
            "{starts_with}: left edge at {:.1} is against the left margin — \
             the line was not right-aligned, it merely filled the measure",
            p.left
        );
    }
    // The control: an LTR paragraph in the same document is unaffected.
    let latin = para(&session, "Hello world");
    assert!(
        (latin.left - MARGIN).abs() < 1.0,
        "the Latin control moved to {:.1}",
        latin.left
    );
}

/// What the alignment fix does *not* fix, pinned so it is known rather
/// than discovered.
///
/// cosmic-text 0.19 resolves the bidi base level per line from the first
/// strong character and offers no way to state it, so a paragraph that
/// declares `dir="rtl"` but opens with a Latin word is ordered as if it
/// were LTR — `EPUB` lands at the *left* end, where a browser honouring
/// the declaration would put it at the right. The declaration does reach
/// alignment, so the line is flush right either way.
///
/// Fixing this means an upstream change or a different shaper. Until
/// then, this test failing is good news.
#[test]
fn bidi_base_level_comes_from_the_text_not_the_declaration() {
    let mut session = open("declared");
    let p = para(&session, "EPUB");
    assert!(
        (p.right - (PAGE.w - MARGIN)).abs() < 1.0,
        "the declared direction should still win the alignment"
    );
    let epub = glyphs(&mut session, p.start, p.at(4));
    let epub_left = epub.first().expect("EPUB is on the page").1;
    let rest = glyphs(&mut session, p.at(5), p.end);
    let hebrew_left = rest
        .iter()
        .map(|&(_, x, ..)| x)
        .fold(f32::INFINITY, f32::min);
    assert!(
        epub_left < hebrew_left,
        "EPUB is no longer left of the Hebrew — the base level now follows \
         the declaration, so this limitation is fixed and the test should go"
    );
}

// ---- hit-testing ----

/// The regression test for the second defect. On an RTL line the caret
/// search used to walk the glyph list treating later-in-the-list as
/// further-right, so every point matched every glyph and the answer was
/// always the last offset on the line. A tap anywhere in a Hebrew
/// paragraph reported the final word.
#[test]
fn hit_testing_an_rtl_line_starts_from_the_right_margin() {
    let mut session = open("hit");
    let p = para(&session, "שלום עולם");
    let y = p.mid_y();
    let words = words_in(&mut session, &p);
    let (first, last) = (words[0], words[words.len() - 1]);

    let first_x = word_centre(&mut session, first.0, first.1);
    let last_x = word_centre(&mut session, last.0, last.1);
    assert!(
        first_x > last_x,
        "the paragraph's first word sits at x={first_x:.1} and its last at \
         x={last_x:.1}: reading order does not begin on the right"
    );

    assert_eq!(
        session.word_at(first_x, y),
        Some(first),
        "a tap on the rightmost word did not report the paragraph's first"
    );
    assert_eq!(
        session.word_at(last_x, y),
        Some(last),
        "a tap on the leftmost word did not report the paragraph's last"
    );

    // And the offsets fall as the point moves right, which is what makes a
    // drag select a growing range rather than jumping.
    let mut previous = u32::MAX;
    for step in 0..24 {
        let x = p.left + (p.right - p.left) * (step as f32 / 23.0);
        if let Some((start, _)) = session.word_at(x, y) {
            assert!(
                start <= previous,
                "moving right from x={x:.1} the offset rose from {previous} to {start}"
            );
            previous = start;
        }
    }
}

/// Which side of a glyph the caret falls on is mirrored too, and this is
/// the only thing `Glyph::rtl` is for.
///
/// Dragging a selection through Hebrew moves the caret between characters.
/// In an LTR run the caret advances once the point is past a glyph's
/// midpoint on the *right*; in an RTL run reading continues to the
/// **left**, so it is the left half that advances. Getting this backwards
/// is not dramatic — nothing crashes, no line moves — the selection is
/// just one character off from the finger, in every drag, forever.
#[test]
fn the_caret_advances_leftward_through_an_rtl_glyph() {
    let mut session = open("caret");
    let p = para(&session, "שלום עולם");
    let y = p.mid_y();

    // A glyph inside the first word, away from either end of the line.
    let target = p.at(1);
    let g = glyphs(&mut session, target, target + 1);
    let (_, x, advance, _) = g[0];
    let centre = x + advance / 2.0;
    let step = advance / 4.0;

    // Anchor at the paragraph's first character, which in an RTL line is
    // its rightmost, then drag onto each half of the target glyph.
    assert!(
        session.selection_begin(p.right - 1.0, y),
        "the drag did not start on text"
    );

    session.selection_drag(centre + step, y);
    let right_half = session.selected_range().expect("a range");
    session.selection_drag(centre - step, y);
    let left_half = session.selected_range().expect("a range");

    assert_eq!(
        right_half.1, target,
        "the right half of an RTL glyph should leave the caret before it"
    );
    assert_eq!(
        left_half.1,
        target + 1,
        "the left half is the reading-order side: the caret should have \
         advanced past the glyph"
    );
}

/// The LTR control for the same code path, so a failure above is about
/// direction and not about hit-testing in general.
#[test]
fn hit_testing_an_ltr_line_still_starts_from_the_left() {
    let mut session = open("hit-ltr");
    let p = para(&session, "Hello world");
    let y = p.mid_y();
    let words = words_in(&mut session, &p);
    let (first, last) = (words[0], words[words.len() - 1]);

    let first_x = word_centre(&mut session, first.0, first.1);
    let last_x = word_centre(&mut session, last.0, last.1);
    assert!(first_x < last_x, "an LTR line should run left to right");
    assert_eq!(session.word_at(first_x, y), Some(first));
    assert_eq!(session.word_at(last_x, y), Some(last));
}

// ---- selection geometry ----

/// The regression test for the third defect, and the reason it matters:
/// the highlight is what tells a reader what they selected.
///
/// Selecting from the start of the line through the middle of `EPUB`
/// covers two visually separate pieces — the Hebrew word at the right, and
/// `EP` in the middle — with `UB` sitting between them, unselected. One
/// spanning rect would paint over `UB` and lie about it.
#[test]
fn a_selection_that_splits_visually_paints_a_rect_per_piece() {
    let mut session = open("split");
    let p = para(&session, "שלום EPUB");
    let e = p.index_of("EPUB");

    // Everything up to and including "EP".
    session.select_range(p.start, p.at(e + 2));
    let spans = selection_spans(&mut session, p.mid_y());
    assert_eq!(
        spans.len(),
        2,
        "expected two highlight rects for a visually split range, got {spans:?}"
    );

    // The gap between them is exactly the unselected "UB".
    let ub = glyphs(&mut session, p.at(e + 2), p.at(e + 4));
    let ub_left = ub.iter().map(|&(_, x, ..)| x).fold(f32::INFINITY, f32::min);
    let ub_right = ub
        .iter()
        .map(|&(_, x, w, _)| x + w)
        .fold(f32::NEG_INFINITY, f32::max);
    let (gap_start, gap_end) = (spans[0].1, spans[1].0);
    assert!(
        gap_start <= ub_left + 0.5 && ub_right <= gap_end + 0.5,
        "the unselected UB spans {ub_left:.1}..{ub_right:.1} but the gap between \
         the highlights is {gap_start:.1}..{gap_end:.1} — the highlight covers \
         text that is not selected"
    );
}

/// The ordinary case must stay ordinary: one rect per line, so a shell
/// that draws them naively sees no change.
#[test]
fn an_unsplit_selection_is_still_one_rect() {
    let mut session = open("unsplit");
    for starts_with in ["Hello world", "שלום עולם"] {
        let p = para(&session, starts_with);
        session.select_range(p.start, p.at(4));
        let spans = selection_spans(&mut session, p.mid_y());
        assert_eq!(
            spans.len(),
            1,
            "{starts_with}: a contiguous selection came back as {spans:?}"
        );
    }
}

/// The text surface over RTL — the material an accessibility tree and TTS
/// are built from, so its order is a reading order, not a screen order.
///
/// This assertion used to live in `tests/text_surface.rs` under the name
/// `rtl_runs_have_sane_ranges`, pointed at a fixture with no RTL text in
/// it. The comment there described exactly what is checked below —
/// per-line locators not monotonic in x, runs still in reading order —
/// about a book that could not have demonstrated any of it.
#[test]
fn the_text_surface_over_rtl_stays_in_reading_order() {
    let session = open("surface");
    let runs = session.page_text_runs().expect("laid out");
    assert!(!runs.is_empty());

    let mut last_start = 0;
    for run in &runs {
        assert!(!run.text.is_empty(), "empty runs are skipped");
        assert!(
            run.locator_end >= run.locator_start,
            "range is [start, end): {run:?}"
        );
        assert!(
            run.locator_start >= last_start,
            "runs are in reading order: {run:?}"
        );
        last_start = run.locator_start;
        assert!(
            run.rect.min_x() >= -0.5
                && run.rect.min_y() >= -0.5
                && run.rect.max_x() <= PAGE.w + 0.5
                && run.rect.max_y() <= PAGE.h + 0.5,
            "rect is on the page: {run:?}"
        );
    }

    // The Hebrew and Arabic runs are single runs covering their whole
    // paragraph even though their glyphs run the other way: a run is a
    // line of reading, not a left-to-right sweep.
    for starts_with in ["שלום עולם", "مرحبا"] {
        let p = para(&session, starts_with);
        assert_eq!(
            p.text.chars().count() as u32,
            p.end - p.start,
            "{starts_with}: the run's locator range should cover its text once"
        );
    }
}

/// Selection is a locator range, and locator space runs in logical order
/// whichever way the glyphs do. The text that comes back must be the text
/// that was asked for.
#[test]
fn selected_text_over_rtl_is_in_logical_order() {
    let mut session = open("text");
    let p = para(&session, "שלום עולם");
    session.select_range(p.start, p.at(4));
    assert_eq!(session.selected_text().as_deref(), Some("שלום"));
}
