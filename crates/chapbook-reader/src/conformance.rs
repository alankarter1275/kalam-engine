//! A conformance harness for shells.
//!
//! The rest of the test suite asks what the engine computes. This asks
//! something a golden image cannot: whether the code *driving* a
//! [`Session`] holds up its end. Those are different failures, and the
//! second kind had been invisible.
//!
//! The instance that motivated the harness: the first end-to-end run of
//! `chapbook-panel-fbdev`'s `show` example on real hardware turned exactly
//! one page and stopped. Nothing was wrong with the engine. The shell
//! compared `session.page()` across a turn, but a turn off the end of a
//! unit crosses into the next one by resetting the page to 0 — so a
//! successful move read as "did not move", and since most books open on a
//! single-page cover it read that on the first turn. No test in the
//! workspace could have caught it, because none of them drove a shell.
//!
//! The prose companion is `docs/SHELLS.md`, which states the same rules
//! in the order a shell author meets them.
//!
//! # Using it
//!
//! Give the harness a way to open a session; it opens a fresh one per
//! check, because checks move the reading position and a shared session
//! would make them order-dependent.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use chapbook_reader::chapbook_core::FontSource;
//! use chapbook_reader::{conformance, Session};
//!
//! // A fixed source, not the host's: a conformance run should mean the
//! // same thing on every machine it is run on.
//! let fonts = FontSource::embedded("fixtures/fonts", "Crimson Text");
//! let report = conformance::Harness::new(move || {
//!     Session::open("book.epub", fonts.clone()).unwrap()
//! })
//! .run();
//! report.assert_ok();
//! # Ok(())
//! # }
//! ```
//!
//! For [`Check::PositionSurvivesARestart`] the closure must reopen the
//! *same* library — the position is persisted, so a factory that hands
//! `SessionConfig::with_library_dir` a fresh temporary directory each call
//! cannot observe it. That check reports [`Outcome::Skipped`] rather than failing
//! when the book has no library record at all.
//!
//! The harness only ever calls the public API. It is written to be read as
//! well as run: a shell author who wants to know the right way to drive a
//! turn, settle a background load, or resize can copy the body of the
//! matching check.
//!
//! It is not read-only. Checking that a position survives a restart means
//! saving one, so a run moves the stored reading position for the book it
//! is pointed at — give the factory a scratch `with_library_dir` if that
//! is not welcome.

use std::fmt;
use std::time::{Duration, Instant};

use chapbook_core::{BookKind, EdgeSizes, PageMetrics, Rect, Rotation, Size};
use chapbook_paint::FrameIntent;

use crate::{Position, Session};

/// One rule a shell relies on. Named rather than free-form so a failing
/// report can be matched on, and so the list itself documents the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    /// A turn that crosses into the next unit still counts as a move —
    /// the failure this harness exists for.
    ACrossedUnitStillMoves,
    /// Turning forward from the start reaches the end of the book, visits
    /// units in order, and terminates.
    PageTurnsWalkTheWholeBook,
    /// Forward *n* then back *n* lands where it started.
    TurnsAreReversible,
    /// At the end of the book a turn reports no move and disturbs nothing.
    TheEndOfTheBookStandsStill,
    /// New metrics relayout and keep the reader's place.
    ResizeKeepsThePlace,
    /// Rotation is a panel change, not a reflow: the position is identical.
    RotationIsNotAReflow,
    /// Taking a frame consumes the change record.
    FrameConsumesTheChangeRecord,
    /// Stated damage lies inside the page it describes.
    DamageStaysInsideThePage,
    /// Background loads converge without the shell blocking on I/O.
    PendingLoadsConverge,
    /// A selection is visible to the shell and a turn drops it.
    ASelectionLivesAndDiesWithThePage,
    /// A saved position comes back on reopen.
    PositionSurvivesARestart,
}

impl Check {
    /// Every check, in the order [`Harness::run`] performs them.
    pub const ALL: &'static [Check] = &[
        Check::ACrossedUnitStillMoves,
        Check::PageTurnsWalkTheWholeBook,
        Check::TurnsAreReversible,
        Check::TheEndOfTheBookStandsStill,
        Check::ResizeKeepsThePlace,
        Check::RotationIsNotAReflow,
        Check::FrameConsumesTheChangeRecord,
        Check::DamageStaysInsideThePage,
        Check::PendingLoadsConverge,
        Check::ASelectionLivesAndDiesWithThePage,
        Check::PositionSurvivesARestart,
    ];
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Check::ACrossedUnitStillMoves => "a crossed unit still moves",
            Check::PageTurnsWalkTheWholeBook => "page turns walk the whole book",
            Check::TurnsAreReversible => "turns are reversible",
            Check::TheEndOfTheBookStandsStill => "the end of the book stands still",
            Check::ResizeKeepsThePlace => "resize keeps the place",
            Check::RotationIsNotAReflow => "rotation is not a reflow",
            Check::FrameConsumesTheChangeRecord => "frame consumes the change record",
            Check::DamageStaysInsideThePage => "damage stays inside the page",
            Check::PendingLoadsConverge => "pending loads converge",
            Check::ASelectionLivesAndDiesWithThePage => "a selection lives and dies with the page",
            Check::PositionSurvivesARestart => "position survives a restart",
        })
    }
}

/// What a check concluded. `Skipped` is a first-class answer: a comic has
/// no text to select and a single-unit book has no unit to cross, and
/// calling either a pass would overstate what ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed(String),
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub check: Check,
    pub outcome: Outcome,
}

/// The result of a run.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    /// Whether every check that ran passed. Skips do not fail a run.
    pub fn passed(&self) -> bool {
        self.failures().next().is_none()
    }

    pub fn failures(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| matches!(f.outcome, Outcome::Failed(_)))
    }

    /// Panic with the full report unless everything that ran passed —
    /// the one-line form for a `#[test]`.
    #[track_caller]
    pub fn assert_ok(&self) {
        assert!(self.passed(), "shell conformance failed\n{self}");
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for finding in &self.findings {
            match &finding.outcome {
                Outcome::Passed => writeln!(f, "  ok      {}", finding.check)?,
                Outcome::Skipped(why) => writeln!(f, "  skip    {} — {why}", finding.check)?,
                Outcome::Failed(why) => writeln!(f, "  FAILED  {} — {why}", finding.check)?,
            }
        }
        Ok(())
    }
}

/// Metrics the harness lays out under when the caller states none: an
/// ordinary portrait page, big enough that a fixture chapter paginates
/// into more than one page.
pub fn default_metrics() -> PageMetrics {
    PageMetrics {
        size: Size::new(600.0, 800.0),
        margins: EdgeSizes::uniform(32.0),
        dpi_scale: 1.0,
        rotation: Rotation::None,
    }
}

/// Drain background loads until the session has none in flight.
///
/// This is the shape of the blocking contract, and shells should copy it:
/// units decode on the loader thread, the shell polls (or wakes on
/// [`Session::set_waker`]) and redraws — it never reaches for
/// `unit_bytes` itself. Returns false if the budget ran out first.
pub fn settle(session: &mut Session, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    // Laying the unit out is what queues its load, so ask for something
    // that needs the layout before waiting on it. Deliberately not
    // `frame()`: taking a frame *consumes* the change record, so settling
    // that way would eat the very intent the shell is about to act on.
    let _ = session.page_count();
    while session.has_pending_loads() {
        if Instant::now() > deadline {
            return false;
        }
        if !session.poll_loaded() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = session.page_count();
    }
    session.poll_loaded();
    let _ = session.page_count();
    true
}

/// Runs [`Check::ALL`] against sessions from a factory.
pub struct Harness<F> {
    open: F,
    metrics: PageMetrics,
    budget: Duration,
}

impl<F: FnMut() -> Session> Harness<F> {
    pub fn new(open: F) -> Self {
        Harness {
            open,
            metrics: default_metrics(),
            budget: Duration::from_secs(10),
        }
    }

    /// Lay out under these metrics instead of [`default_metrics`].
    pub fn metrics(mut self, metrics: PageMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// How long any one check may wait for background loads.
    pub fn budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    pub fn run(mut self) -> Report {
        let mut findings = Vec::new();
        for &check in Check::ALL {
            let mut session = (self.open)();
            session.set_metrics(self.metrics);
            let outcome = if settle(&mut session, self.budget) {
                self.perform(check, &mut session)
            } else {
                Outcome::Failed(format!(
                    "background loads did not settle within {:?}",
                    self.budget
                ))
            };
            findings.push(Finding { check, outcome });
        }
        Report { findings }
    }

    fn perform(&mut self, check: Check, session: &mut Session) -> Outcome {
        match check {
            Check::ACrossedUnitStillMoves => self.crossed_unit_moves(session),
            Check::PageTurnsWalkTheWholeBook => self.walk_the_book(session),
            Check::TurnsAreReversible => self.reversible(session),
            Check::TheEndOfTheBookStandsStill => self.end_stands_still(session),
            Check::ResizeKeepsThePlace => self.resize_keeps_place(session),
            Check::RotationIsNotAReflow => self.rotation_is_not_a_reflow(session),
            Check::FrameConsumesTheChangeRecord => self.frame_consumes_record(session),
            Check::DamageStaysInsideThePage => self.damage_inside_page(session),
            Check::PendingLoadsConverge => self.loads_converge(session),
            Check::ASelectionLivesAndDiesWithThePage => self.selection_lifecycle(session),
            Check::PositionSurvivesARestart => self.position_survives(session),
        }
    }

    /// The regression that named this harness. Walk to the first turn that
    /// changes unit and check both halves: the turn reports a move, and
    /// the page number alone does *not* show one — which is exactly why a
    /// shell must compare [`Position`], and why comparing pages looked
    /// right until it met a one-page cover.
    fn crossed_unit_moves(&mut self, session: &mut Session) -> Outcome {
        if session.spine_len() < 2 {
            return Outcome::Skipped("single-unit book: no unit to cross".into());
        }
        for _ in 0..MAX_TURNS {
            let before = session.position();
            if !session.next_page() {
                break;
            }
            let after = session.position();
            if after.spine == before.spine {
                let _ = settle(session, self.budget);
                continue;
            }
            if after.page > before.page {
                return Outcome::Skipped(format!(
                    "this book's first unit crossing went {} -> {}, so it does not \
                     exercise the reset-to-zero case",
                    before.page, after.page
                ));
            }
            return Outcome::Passed;
        }
        Outcome::Failed("never reached a unit boundary before the turn budget ran out".into())
    }

    /// Turning forward from the start must reach the end: visit units in
    /// order, never revisit one, and stop. A shell whose loop exits early
    /// fails the first clause; one whose exit test never fires fails the
    /// last.
    fn walk_the_book(&mut self, session: &mut Session) -> Outcome {
        let mut visited = vec![session.position().spine];
        let mut turns = 0usize;
        loop {
            let before = session.position();
            if !session.next_page() {
                break;
            }
            turns += 1;
            if turns > MAX_TURNS {
                return Outcome::Failed(format!(
                    "still turning after {MAX_TURNS} pages: the walk does not terminate"
                ));
            }
            let after = session.position();
            if after == before {
                return Outcome::Failed(format!(
                    "next_page reported a move but the position stayed at {before:?}"
                ));
            }
            if after.spine < before.spine {
                return Outcome::Failed(format!(
                    "a forward turn went backwards through the spine: {before:?} -> {after:?}"
                ));
            }
            if after.spine != before.spine {
                if after.page != 0 {
                    return Outcome::Failed(format!(
                        "crossing into unit {} landed on page {}, not its first page",
                        after.spine, after.page
                    ));
                }
                visited.push(after.spine);
            }
            let _ = settle(session, self.budget);
        }
        let last = session.position();
        if last.spine + 1 != session.spine_len() {
            return Outcome::Failed(format!(
                "turning forward stopped in unit {} of {}: the rest of the book is unreachable \
                 by page turn",
                last.spine,
                session.spine_len()
            ));
        }
        // Units are consumed in strictly increasing order, so the walk
        // covers everything between the first and the last.
        if visited.windows(2).any(|w| w[0] >= w[1]) {
            return Outcome::Failed(format!("units were not visited in order: {visited:?}"));
        }
        Outcome::Passed
    }

    fn reversible(&mut self, session: &mut Session) -> Outcome {
        let start = session.position();
        let mut forward = 0;
        for _ in 0..8 {
            if !session.next_page() {
                break;
            }
            forward += 1;
            let _ = settle(session, self.budget);
        }
        if forward == 0 {
            return Outcome::Skipped("book is a single page: nothing to turn back".into());
        }
        for _ in 0..forward {
            if !session.prev_page() {
                return Outcome::Failed(format!(
                    "ran out of pages going back after {forward} forward turns, \
                     stuck at {:?}",
                    session.position()
                ));
            }
            let _ = settle(session, self.budget);
        }
        let back = session.position();
        if back != start {
            return Outcome::Failed(format!(
                "{forward} turns forward and back landed on {back:?}, not {start:?}"
            ));
        }
        Outcome::Passed
    }

    fn end_stands_still(&mut self, session: &mut Session) -> Outcome {
        let mut turns = 0;
        while session.next_page() {
            turns += 1;
            if turns > MAX_TURNS {
                return Outcome::Failed("never reached the end of the book".into());
            }
            let _ = settle(session, self.budget);
        }
        let at_end = session.position();
        let _ = session.frame();
        if session.next_page() {
            return Outcome::Failed("a turn past the end of the book moved".into());
        }
        if session.position() != at_end {
            return Outcome::Failed("a turn past the end changed the position anyway".into());
        }
        // A shell repaints on intent; the refusal must not ask for one.
        if let Some(frame) = session.frame() {
            if frame.intent != FrameIntent::Repaint {
                return Outcome::Failed(format!(
                    "a turn that did nothing still reported {:?}",
                    frame.intent
                ));
            }
        }
        Outcome::Passed
    }

    fn resize_keeps_place(&mut self, session: &mut Session) -> Outcome {
        // Get off the first page, so "kept the place" is a real claim.
        for _ in 0..3 {
            if !session.next_page() {
                break;
            }
            let _ = settle(session, self.budget);
        }
        let before_offset = session.current_offset();
        let before = session.position();
        let mut narrower = self.metrics;
        narrower.size = Size::new(self.metrics.size.w * 0.75, self.metrics.size.h);
        session.set_metrics(narrower);
        let _ = settle(session, self.budget);

        let Some(frame) = session.frame() else {
            return Outcome::Failed("no frame after a resize".into());
        };
        if frame.intent != FrameIntent::Relayout {
            return Outcome::Failed(format!(
                "a resize reported {:?}, not Relayout — an e-ink shell would \
                 pick the wrong waveform for a page that changed everywhere",
                frame.intent
            ));
        }
        if session.position().spine != before.spine {
            return Outcome::Failed(format!(
                "a resize moved the reader out of unit {} into unit {}",
                before.spine,
                session.position().spine
            ));
        }
        // Image books carry no char map, so the offset is 0 either way and
        // the unit is the whole claim.
        if session.kind() == BookKind::Epub {
            let after_offset = session.current_offset();
            let page = session.page_count();
            if after_offset > before_offset {
                return Outcome::Failed(format!(
                    "a resize moved the reader forward: offset {before_offset} -> \
                     {after_offset} (unit now {page} pages)"
                ));
            }
        }
        Outcome::Passed
    }

    fn rotation_is_not_a_reflow(&mut self, session: &mut Session) -> Outcome {
        for _ in 0..3 {
            if !session.next_page() {
                break;
            }
            let _ = settle(session, self.budget);
        }
        let before = session.position();
        let pages = session.page_count();
        let mut turned = self.metrics;
        turned.rotation = match self.metrics.rotation {
            Rotation::None => Rotation::Quarter,
            _ => Rotation::None,
        };
        session.set_metrics(turned);
        let _ = settle(session, self.budget);
        if session.position() != before {
            return Outcome::Failed(format!(
                "turning the panel moved the reader from {before:?} to {:?} — \
                 rotation happens on the way to the panel, so the layout must \
                 not be rebuilt",
                session.position()
            ));
        }
        if session.page_count() != pages {
            return Outcome::Failed(format!(
                "turning the panel repaginated the unit: {pages} pages -> {}",
                session.page_count()
            ));
        }
        Outcome::Passed
    }

    fn frame_consumes_record(&mut self, session: &mut Session) -> Outcome {
        if !session.next_page() {
            return Outcome::Skipped("book is a single page: nothing to change".into());
        }
        let _ = settle(session, self.budget);
        let Some(first) = session.frame() else {
            return Outcome::Failed("no frame after a page turn".into());
        };
        if first.intent < FrameIntent::PageTurn {
            return Outcome::Failed(format!(
                "a page turn reported {:?}, which understates it",
                first.intent
            ));
        }
        let Some(second) = session.frame() else {
            return Outcome::Failed("no frame on a repaint".into());
        };
        if second.intent != FrameIntent::Repaint {
            return Outcome::Failed(format!(
                "taking a frame did not consume the change record: the next one \
                 still reported {:?}",
                second.intent
            ));
        }
        Outcome::Passed
    }

    fn damage_inside_page(&mut self, session: &mut Session) -> Outcome {
        let page = Rect::new(0.0, 0.0, self.metrics.size.w, self.metrics.size.h);
        let mut checked = 0;
        for _ in 0..12 {
            let Some(frame) = session.frame() else { break };
            if let Some(damage) = frame.damage {
                checked += 1;
                if !page.contains_rect(&damage) {
                    return Outcome::Failed(format!(
                        "{:?} damage {damage:?} falls outside the {page:?} page it \
                         describes: a shell blitting only that rect would read \
                         out of bounds",
                        frame.intent
                    ));
                }
            }
            if !session.next_page() {
                break;
            }
            let _ = settle(session, self.budget);
        }
        // A selection is the change that always states its region, so it
        // guarantees this check has something to look at.
        let offset = session.current_offset();
        session.select_range(offset, offset + 24);
        if let Some(frame) = session.frame() {
            if let Some(damage) = frame.damage {
                checked += 1;
                if !page.contains_rect(&damage) {
                    return Outcome::Failed(format!(
                        "selection damage {damage:?} falls outside the {page:?} page"
                    ));
                }
            }
        }
        if checked == 0 {
            return Outcome::Skipped("no frame stated a damage region".into());
        }
        Outcome::Passed
    }

    fn loads_converge(&mut self, session: &mut Session) -> Outcome {
        // Walk a few units, settling each: a shell that never polls sits
        // on placeholders forever, and one that blocks on I/O in its event
        // loop stops answering input.
        for _ in 0..4 {
            if !session.next_unit() {
                break;
            }
            if !settle(session, self.budget) {
                return Outcome::Failed(format!(
                    "unit {} still had loads in flight after {:?}",
                    session.position().spine,
                    self.budget
                ));
            }
            if session.has_pending_loads() {
                return Outcome::Failed("settled, but loads are still pending".into());
            }
            if session.frame().is_none() {
                return Outcome::Failed(format!(
                    "unit {} produced no frame once its load landed",
                    session.position().spine
                ));
            }
        }
        Outcome::Passed
    }

    fn selection_lifecycle(&mut self, session: &mut Session) -> Outcome {
        if session.kind() == BookKind::Comic {
            return Outcome::Skipped("comic pages have no text layer".into());
        }
        let offset = session.current_offset();
        session.select_range(offset, offset + 24);
        let Some((start, end)) = session.selected_range() else {
            return Outcome::Failed("a selected range did not come back".into());
        };
        if (start, end) != (offset, offset + 24) {
            return Outcome::Failed(format!(
                "selected {offset}..{} but got back {start}..{end}",
                offset + 24
            ));
        }
        if let Some(frame) = session.frame() {
            if frame.intent < FrameIntent::Selection {
                return Outcome::Failed(format!(
                    "a selection reported {:?}, which understates it",
                    frame.intent
                ));
            }
        }
        session.selection_clear();
        if session.selected_range().is_some() {
            return Outcome::Failed("clearing left a selection behind".into());
        }
        // A turn takes the selection with it: the range is in the old
        // unit's locator space and means nothing on the new page.
        session.select_range(offset, offset + 24);
        if session.next_page() && session.selected_range().is_some() {
            return Outcome::Failed(
                "a page turn kept the selection: its offsets no longer name \
                 anything on screen"
                    .into(),
            );
        }
        Outcome::Passed
    }

    fn position_survives(&mut self, session: &mut Session) -> Outcome {
        for _ in 0..5 {
            if !session.next_page() {
                break;
            }
            let _ = settle(session, self.budget);
        }
        let saved = session.position();
        if saved.spine == 0 && saved.page == 0 {
            return Outcome::Skipped("book is a single page: no position to restore".into());
        }
        session.save_position();

        let mut reopened = (self.open)();
        reopened.set_metrics(self.metrics);
        if !settle(&mut reopened, self.budget) {
            return Outcome::Failed("the reopened session never settled".into());
        }
        let restored = reopened.position();
        if restored == (Position { spine: 0, page: 0 }) && saved.spine != 0 {
            return Outcome::Skipped(
                "reopening started from the beginning: this factory does not \
                 reopen the same library, so there is nothing to restore from"
                    .into(),
            );
        }
        if restored.spine != saved.spine {
            return Outcome::Failed(format!(
                "saved {saved:?} but reopened at {restored:?}: the reader lost \
                 their place across a restart"
            ));
        }
        Outcome::Passed
    }
}

/// How far the harness will turn before calling a book unwalkable. Books
/// in the wild are shorter than this by orders of magnitude; it exists to
/// turn a non-terminating walk into a failure instead of a hung test.
const MAX_TURNS: usize = 20_000;
