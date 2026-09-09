//! `ReaderView`: the page on screen, as a GTK widget.
//!
//! One `gtk::DrawingArea` with an engine `Session` behind it. The engine
//! rasterizes the page with tiny-skia at device pixels; the draw function
//! converts the pixels to cairo's byte order and paints them at 1/scale,
//! so GTK's logical coordinates and the engine's CSS px agree. That
//! ten-line blit, the press/drag/tap handling and the background-load
//! wakeup are lifted from `chapbook-viewer-gtk/src/linux.rs`, the
//! reference shell; what is Kalam's is everything around them — the
//! preference plumbing, the callbacks, the column width.
//!
//! The widget owns nothing Kalam owns. Positions, highlights, bookmarks and
//! the dictionary stay in Kalam's database; this widget reports what
//! happened (a page turned, a word was tapped, text was selected) through
//! plain Rust callbacks and takes instructions (go here, use this theme)
//! through plain methods. It never opens a network connection, never runs
//! a script, and never reads a file it was not handed.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::cairo;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use chapbook_core::{
    Action, EdgeSizes, Key, KeyMap, LayeredLocator, PageMetrics, Rect, Rotation, Size, TapZones,
    TocEntry,
};
use chapbook_reader::{HostHighlight, Session, SessionConfig, SessionEvent};

use crate::prefs::{HighlightColor, KalamPrefs};

/// Padding between the page and the widget's edge, in CSS px. Kalam's
/// page CSS had `padding: 56px 28px 96px 28px` on a scrolling body; a
/// paged view needs less at the bottom (no scroll runway), and the sides
/// only matter when the window is narrower than the column.
const MARGIN_TOP: f32 = 48.0;
const MARGIN_BOTTOM: f32 = 40.0;
const MARGIN_SIDE_MIN: f32 = 28.0;

/// A press that moves less than this is a tap, not a drag. A mouse's
/// threshold; a touchscreen shell would want the platform's own.
const TAP_SLOP: f64 = 4.0;

/// How much laid-out text and decoded image the engine keeps around when
/// [`ReaderOptions::cache_budget`] is `None`. The engine's own default is
/// 192 MB, sized for comics on a phone; a novel's chapter is well under a
/// megabyte laid out, so this holds a whole book's worth of chapters and
/// still leaves room for a cover, on a machine with 4 GB in total.
pub const DEFAULT_CACHE_BUDGET: usize = 32 * 1024 * 1024;

/// A draw slower than this is logged (at `info`), so a run's log says
/// which page turns were not instant and what the engine was doing.
const SLOW_FRAME: std::time::Duration = std::time::Duration::from_millis(25);

/// Where the reader is, in the terms Kalam stores and shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadingPosition {
    /// Spine index of the chapter on screen — Kalam's `chapter_index`.
    pub chapter: usize,
    /// How many chapters the book has.
    pub chapter_count: usize,
    /// Page within the chapter, from zero, and how many the chapter has at
    /// the current size and settings.
    pub page: usize,
    pub page_count: usize,
    /// How far into the chapter the top of this page is, `0.0..=1.0` —
    /// the number Kalam's `reading_progress.fraction` column holds. Text
    /// position, not page arithmetic, so it survives a font-size change.
    /// Cheap: this is what to save on every page turn. The durable form
    /// is [`ReaderView::locator`].
    pub fraction: f64,
}

/// A word the reader tapped, for the dictionary popover.
#[derive(Debug, Clone, PartialEq)]
pub struct TappedWord {
    /// The word itself, trimmed, as the page shows it.
    pub word: String,
    /// The sentence around it, for sense ranking — what Kalam's
    /// `sentenceAroundText` produced from the DOM.
    pub sentence: String,
    /// Where the word sits, in widget coordinates, so a popover can point
    /// at it.
    pub rect: Rect,
    /// Kalam's id of the highlight the word sits in, if any — the tap
    /// that means "this highlight" (recolour, delete) rather than "this
    /// word" (look it up). Kalam decides which; the widget reports both.
    pub highlight: Option<i64>,
}

/// A text selection the reader finished, for the copy/highlight/lookup
/// chip.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectedText {
    pub text: String,
    /// The selected span's bounding box in widget coordinates, for placing
    /// the chip clear of it.
    pub rect: Rect,
}

/// What to do about the host's fonts, and other knobs a shell sets once.
/// The default is the right answer for Kalam: bundled fonts only, the
/// engine's own directory, its default cache.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReaderOptions {
    /// Scan the system's fonts too, for scripts the bundled faces lack.
    /// Off by default: on a cold hard disk the scan alone is seconds.
    pub host_fonts: bool,
    /// Where the engine keeps its own small database (reading settings per
    /// book, page cache). `None` uses the platform default
    /// (`~/.local/share/chapbook`). Kalam should hand over a directory
    /// under its own data dir so everything lives in one place.
    pub engine_dir: Option<std::path::PathBuf>,
    /// How much decoded-image and layout cache the engine may keep, in
    /// bytes. `None` is [`DEFAULT_CACHE_BUDGET`] (32 MB), not the engine's
    /// own, comic-sized default.
    pub cache_budget: Option<usize>,
}

type PositionCallback = dyn Fn(&ReadingPosition);
type WordCallback = dyn Fn(&TappedWord);
type SelectionCallback = dyn Fn(Option<&SelectedText>);
type LinkCallback = dyn Fn(&str);

/// Callbacks a shell installs. All run on the GTK main thread, from inside
/// the widget's own handlers — keep them quick, or defer to an idle. A
/// callback may call the view's methods (turn a page, change a
/// preference) but must not install another callback from inside itself;
/// do that from an idle too.
#[derive(Default)]
struct Callbacks {
    position: Option<Box<PositionCallback>>,
    word: Option<Box<WordCallback>>,
    selection: Option<Box<SelectionCallback>>,
    link: Option<Box<LinkCallback>>,
}

struct Inner {
    session: RefCell<Session>,
    prefs: Cell<KalamPrefs>,
    callbacks: RefCell<Callbacks>,
    /// Last position reported, so a repaint that moved nothing says
    /// nothing.
    last_reported: RefCell<Option<(usize, usize, usize)>>,
    /// Last size drawn at (width, height, scale factor), so a change —
    /// each one is a full relayout — is logged once.
    last_size: Cell<(i32, i32, i32)>,
    keys: KeyMap,
    zones: TapZones,
}

/// The reading widget. Cheap to clone (a reference); dropped when the last
/// clone and the widget tree let go of it.
#[derive(Clone)]
pub struct ReaderView {
    area: gtk::DrawingArea,
    inner: Rc<Inner>,
}

impl ReaderView {
    /// Open a book from a path and build the widget around it.
    ///
    /// Everything expensive happens here: reading the zip's directory,
    /// parsing the package, loading the eight bundled faces (and the
    /// host's, if asked). The first chapter is laid out on the first draw,
    /// once the widget knows its size.
    pub fn open(
        path: impl AsRef<Path>,
        prefs: KalamPrefs,
        options: &ReaderOptions,
    ) -> chapbook_core::Result<ReaderView> {
        let mut config = SessionConfig::new(crate::fonts::font_source(options.host_fonts));
        if let Some(dir) = &options.engine_dir {
            config = config.with_library_dir(dir.clone());
        }
        let budget = options.cache_budget.unwrap_or(DEFAULT_CACHE_BUDGET);
        config = config.with_cache_budget(budget);
        // By handle, not by path. Opened by path, the engine's own library
        // *imports* the book — copies the file into its folder — the first
        // time it sees it. By handle it *adopts* it: a record keyed by the
        // file's hash, no copy. Kalam already keeps the file; one copy on
        // a small disk is enough.
        let file = std::fs::File::open(path.as_ref())?;
        let mut session = Session::open_with(chapbook_core::Source::reader(file), config)?;
        // Kalam's settings, before the first layout, so nothing is laid
        // out twice. `ThisBook` rather than `Global`: the engine's own
        // record of the reader's default is not Kalam's to overwrite, and
        // Kalam re-applies its preferences on every open anyway.
        session.set_settings(
            prefs.reading_settings(),
            chapbook_reader::SettingsScope::ThisBook,
        );
        // Thirds, with an inert middle: Kalam's chrome is Kalam's, so the
        // band that would toggle a menu does nothing here. The direction
        // comes from the book.
        let zones = TapZones {
            middle: None,
            ..TapZones::new(session.reading_direction())
        };
        // Page turns and chapter skips from the engine's default map;
        // font size and theme are Kalam's preferences, changed through
        // Kalam's own controls, so those keys are unbound rather than
        // left to move the text behind the settings panel's back.
        let mut keys = KeyMap::default();
        for key in [
            Key::Char('+'),
            Key::Char('='),
            Key::Char('-'),
            Key::Char('t'),
            Key::Char('m'),
        ] {
            keys.unbind(key);
        }

        let area = gtk::DrawingArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_focusable(true);
        area.set_can_focus(true);

        let view = ReaderView {
            area,
            inner: Rc::new(Inner {
                session: RefCell::new(session),
                prefs: Cell::new(prefs.clamped()),
                callbacks: RefCell::new(Callbacks::default()),
                last_reported: RefCell::new(None),
                last_size: Cell::new((0, 0, 0)),
                keys,
                zones,
            }),
        };
        view.install_draw();
        view.install_keys();
        view.install_pointer();
        view.install_loader_wakeup();
        Ok(view)
    }

    /// The GTK widget to put in a container.
    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    // ---- Callbacks ----

    /// Called after every page turn, jump, relayout or resize with where
    /// the reader is now. Kalam saves `chapter`/`fraction` (and the
    /// locator) from here.
    pub fn connect_position(&self, f: impl Fn(&ReadingPosition) + 'static) {
        self.inner.callbacks.borrow_mut().position = Some(Box::new(f));
    }

    /// Called when the reader taps a word (press and release without
    /// moving, on text). Kalam opens its dictionary popover.
    pub fn connect_word(&self, f: impl Fn(&TappedWord) + 'static) {
        self.inner.callbacks.borrow_mut().word = Some(Box::new(f));
    }

    /// Called when a drag-selection ends with text in it (`Some`), and
    /// when the selection is cleared (`None`) — show and hide the chip.
    pub fn connect_selection(&self, f: impl Fn(Option<&SelectedText>) + 'static) {
        self.inner.callbacks.borrow_mut().selection = Some(Box::new(f));
    }

    /// Called for a link the engine will not follow itself — anything
    /// with a scheme (`https://…`, `mailto:`). Internal links (footnotes,
    /// cross-references) are followed in place and never reported.
    pub fn connect_external_link(&self, f: impl Fn(&str) + 'static) {
        self.inner.callbacks.borrow_mut().link = Some(Box::new(f));
    }

    // ---- Preferences ----

    pub fn prefs(&self) -> KalamPrefs {
        self.inner.prefs.get()
    }

    /// Apply new preferences. Relayout keeps the reading position (the
    /// engine re-finds the same text offset in the new pagination), and
    /// the position callback fires with the new page numbers.
    pub fn set_prefs(&self, prefs: KalamPrefs) {
        let prefs = prefs.clamped();
        let old = self.inner.prefs.replace(prefs);
        if old == prefs {
            return;
        }
        {
            let mut s = self.inner.session.borrow_mut();
            s.set_settings(
                prefs.reading_settings(),
                chapbook_reader::SettingsScope::ThisBook,
            );
        }
        // The column width is page geometry, not a setting; the draw
        // function recomputes it from the widget size.
        self.area.queue_draw();
    }

    pub fn set_theme(&self, theme: crate::prefs::KalamTheme) {
        self.set_prefs(KalamPrefs {
            theme,
            ..self.prefs()
        });
    }

    pub fn set_font_px(&self, font_px: f32) {
        self.set_prefs(KalamPrefs {
            font_px,
            ..self.prefs()
        });
    }

    pub fn set_line_height(&self, line_height: f32) {
        self.set_prefs(KalamPrefs {
            line_height,
            ..self.prefs()
        });
    }

    pub fn set_column_px(&self, column_px: f32) {
        self.set_prefs(KalamPrefs {
            column_px,
            ..self.prefs()
        });
    }

    // ---- Navigation ----

    pub fn next_page(&self) {
        self.apply(Action::NextPage);
    }

    pub fn prev_page(&self) {
        self.apply(Action::PrevPage);
    }

    pub fn next_chapter(&self) {
        self.apply(Action::NextUnit);
    }

    pub fn prev_chapter(&self) {
        self.apply(Action::PrevUnit);
    }

    /// Jump to a chapter and a fraction of the way through it — Kalam's
    /// `JumpToLocation(chapter, fraction)` and its bookmarks, which store
    /// exactly that pair. `false` if the chapter does not exist.
    pub fn goto_chapter(&self, chapter: usize, fraction: f64) -> bool {
        let moved = {
            let mut s = self.inner.session.borrow_mut();
            if chapter >= s.spine_len() {
                false
            } else {
                let offset = s
                    .chapter_char_count(chapter)
                    .map(|total| (fraction.clamp(0.0, 1.0) * total as f64).round() as u32)
                    .unwrap_or(0);
                s.goto(chapbook_core::Locator::new(chapter, offset))
            }
        };
        if moved {
            self.area.queue_draw();
        }
        moved
    }

    /// Return to a stored [`ReaderView::locator`]. `same_edition`
    /// says whether the file is the one the locator was captured from
    /// (Kalam knows: same `books.id`, unchanged file); when unsure pass
    /// `false` and the engine re-finds the text by its quote context.
    pub fn goto_locator(&self, locator: &LayeredLocator, same_edition: bool) -> bool {
        let moved = self
            .inner
            .session
            .borrow_mut()
            .goto_layered(locator, same_edition);
        if moved {
            self.area.queue_draw();
        }
        moved
    }

    /// Jump to a table-of-contents entry.
    pub fn goto_toc(&self, entry: &TocEntry) -> bool {
        let moved = self.inner.session.borrow_mut().goto_toc(entry);
        if moved {
            self.area.queue_draw();
        }
        moved
    }

    /// The book's table of contents, as the publisher wrote it (nested).
    pub fn toc(&self) -> Vec<TocEntry> {
        self.inner.session.borrow().toc().to_vec()
    }

    pub fn title(&self) -> String {
        self.inner.session.borrow().title().to_string()
    }

    pub fn chapter_count(&self) -> usize {
        self.inner.session.borrow().spine_len()
    }

    /// Where the reader is right now. The same value the position
    /// callback delivers, for a shell that wants to ask rather than be
    /// told (on close, say).
    pub fn position(&self) -> ReadingPosition {
        let mut s = self.inner.session.borrow_mut();
        position_of(&mut s)
    }

    /// The durable record of the current place. Store it whole (JSON in
    /// Kalam's unused `cfi` column is the plan) and hand it back to
    /// [`ReaderView::goto_locator`]; it finds the place again after a
    /// re-import, a re-parse, or a different edition of the book.
    ///
    /// Not free the first time: the engine counts every chapter's text
    /// once per session to place the position in the whole book. Ask for
    /// it on close and when the chapter changes, not on every page turn —
    /// [`ReadingPosition::fraction`] is the cheap one.
    pub fn locator(&self) -> Option<LayeredLocator> {
        self.inner.session.borrow().layered_locator()
    }

    // ---- Selection, highlights, words ----

    /// The current selection's text, if any — as a person would type it:
    /// the publisher's soft hyphens and zero-width spaces are gone and
    /// whitespace is collapsed, the same as the text in a `NewHighlight`
    /// and a `TappedWord`.
    pub fn selected_text(&self) -> Option<String> {
        self.inner
            .session
            .borrow()
            .selected_text()
            .map(|text| readable(&text))
    }

    /// Drop the selection (Escape, or after the chip's action ran).
    pub fn clear_selection(&self) {
        self.inner.session.borrow_mut().selection_clear();
        self.notify_selection();
        self.area.queue_draw();
    }

    /// Capture the current selection as a highlight for Kalam to store:
    /// the text and the two endpoints as durable locators, in Kalam's
    /// colour. **Nothing is stored or painted by this call.** Kalam writes
    /// its `annotations` row, then calls [`ReaderView::show_highlight`]
    /// with the row's id — that id is the handle for everything after.
    /// `None` without a selection.
    pub fn capture_highlight(&self, color: HighlightColor) -> Option<NewHighlight> {
        let result = {
            let mut s = self.inner.session.borrow_mut();
            let (start, end) = s.selected_range()?;
            let text = readable(&s.selected_text().unwrap_or_default());
            let start_locator = s.layered_locator_at(start)?;
            let end_locator = s.layered_locator_at(end)?;
            s.selection_clear();
            NewHighlight {
                color,
                text,
                start: start_locator,
                end: end_locator,
            }
        };
        self.notify_selection();
        self.area.queue_draw();
        Some(result)
    }

    /// Paint a highlight Kalam has on file, under Kalam's id. Called once
    /// per row after [`ReaderView::capture_highlight`], or for every row
    /// of the book at open — [`ReaderView::set_highlights`] does the
    /// latter in one go. Showing an id already shown replaces it.
    pub fn show_highlight(&self, id: i64, highlight: &NewHighlight) {
        self.inner
            .session
            .borrow_mut()
            .show_host_highlight(host_highlight(id, highlight));
        self.area.queue_draw();
    }

    /// Replace every highlight shown with Kalam's list for this book —
    /// what to call at open, or after Kalam's annotations table changed
    /// behind the widget's back (`AnnotationsReload`).
    pub fn set_highlights<'a>(
        &self,
        highlights: impl IntoIterator<Item = (i64, &'a NewHighlight)>,
    ) {
        let list: Vec<HostHighlight> = highlights
            .into_iter()
            .map(|(id, h)| host_highlight(id, h))
            .collect();
        self.inner.session.borrow_mut().set_host_highlights(list);
        self.area.queue_draw();
    }

    /// Repaint a shown highlight in another of Kalam's colours (Kalam has
    /// already updated its row).
    pub fn recolor_highlight(&self, id: i64, color: HighlightColor) {
        self.inner
            .session
            .borrow_mut()
            .recolor_host_highlight(id, Some(color.css()));
        self.area.queue_draw();
    }

    /// Stop painting a highlight (Kalam has already deleted its row).
    pub fn remove_highlight(&self, id: i64) {
        self.inner.session.borrow_mut().hide_host_highlight(id);
        self.area.queue_draw();
    }

    /// The highlights painted in the chapter on screen, with Kalam's ids
    /// and where each lands in the chapter's text — for lining a panel up
    /// against what is visible.
    pub fn highlights(&self) -> Vec<chapbook_reader::Highlight> {
        let mut s = self.inner.session.borrow_mut();
        let spine = s.spine();
        s.host_highlights(spine).to_vec()
    }

    /// The shown highlight under a point in widget coordinates — a tap on
    /// a marked passage, for a "recolour / delete" chip. `None` off any.
    pub fn highlight_at(&self, x: f64, y: f64) -> Option<i64> {
        self.inner
            .session
            .borrow_mut()
            .host_highlight_at(x as f32, y as f32)
    }

    /// Jump to a shown highlight by Kalam's id.
    pub fn goto_highlight(&self, id: i64) -> bool {
        let moved = self.inner.session.borrow_mut().goto_host_highlight(id);
        if moved {
            self.area.queue_draw();
        }
        moved
    }

    /// Ask the engine to save what it holds (its own settings record)
    /// before the window closes. Kalam's own records are Kalam's to save,
    /// from the position callback.
    pub fn flush(&self) {
        self.inner.session.borrow_mut().save_position();
    }

    /// Bytes of laid-out chapters and decoded images the engine holds
    /// right now (capped by [`ReaderOptions::cache_budget`]). For a memory
    /// readout; the rest of the process is GTK's and the binary's.
    pub fn cache_bytes(&self) -> usize {
        self.inner.session.borrow().cache_bytes()
    }

    // ---- Internals ----

    fn apply(&self, action: Action) {
        let outcome = self.inner.session.borrow_mut().apply(action);
        if outcome.needs_redraw() {
            self.area.queue_draw();
        }
    }

    fn notify_selection(&self) {
        let selected = {
            let s = self.inner.session.borrow();
            s.selected_range().and_then(|(start, end)| {
                let text = readable(&s.selected_text()?);
                let rect = union(&s.range_rects(start, end))?;
                Some(SelectedText { text, rect })
            })
        };
        if let Some(cb) = &self.inner.callbacks.borrow().selection {
            cb(selected.as_ref());
        }
    }

    /// Report the position if it moved since last time. Called from an
    /// idle after every draw — the draw is where every change funnels.
    fn report_position(&self) {
        let position = {
            let mut s = self.inner.session.borrow_mut();
            position_of(&mut s)
        };
        let key = (position.chapter, position.page, position.page_count);
        if self.inner.last_reported.replace(Some(key)) == Some(key) {
            return;
        }
        if let Some(cb) = &self.inner.callbacks.borrow().position {
            cb(&position);
        }
    }

    fn install_draw(&self) {
        let view = self.clone();
        self.area.set_draw_func(move |area, ctx, width, height| {
            if width <= 0 || height <= 0 {
                return;
            }
            let started = std::time::Instant::now();
            let size = (width, height, area.scale_factor());
            if view.inner.last_size.replace(size) != size {
                // Every new size is a full relayout of the chapter; a log
                // that shows two of these at start-up has found a second
                // of start-up time.
                log::info!("page area {width}x{height} at {}x", size.2);
            }
            let scale = size.2 as f32;
            let prefs = view.inner.prefs.get();
            // The column: never wider than Kalam's `column_px`, centred
            // when the widget is wider than that, with at least the
            // minimum side margin when it is narrower.
            let side = ((width as f32 - prefs.column_px) / 2.0).max(MARGIN_SIDE_MIN);
            let mut s = view.inner.session.borrow_mut();
            s.set_metrics(PageMetrics {
                size: Size::new(width as f32, height as f32),
                margins: EdgeSizes {
                    top: MARGIN_TOP,
                    right: side,
                    bottom: MARGIN_BOTTOM,
                    left: side,
                },
                dpi_scale: scale,
                rotation: Rotation::None,
            });
            let Some(pixmap) = s.render() else { return };
            drop(s);

            // Premultiplied RGBA → cairo ARGB32 (BGRA in little-endian).
            let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
            let mut data = pixmap.take();
            for px in data.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
            let stride = pw * 4;
            let Ok(surface) =
                cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, pw, ph, stride)
            else {
                return;
            };
            // The pixmap is device pixels; paint at 1/scale so it maps to
            // the widget's logical size.
            ctx.scale(1.0 / scale as f64, 1.0 / scale as f64);
            let _ = ctx.set_source_surface(&surface, 0.0, 0.0);
            let _ = ctx.paint();
            let took = started.elapsed();
            if took >= SLOW_FRAME {
                log::info!("frame took {} ms", took.as_millis());
            }

            // Every content change funnels through a draw, so this is the
            // one place the shell needs telling — from an idle rather than
            // inside the draw vfunc, and only when the position moved.
            let view = view.clone();
            glib::idle_add_local_once(move || view.report_position());
        });
    }

    fn install_keys(&self) {
        let view = self.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            let name = keyval.name();
            let outcome = match name.as_deref() {
                Some("Escape") if view.inner.session.borrow().selected_range().is_some() => {
                    view.clear_selection();
                    return glib::Propagation::Stop;
                }
                name => match name
                    .and_then(engine_key)
                    .and_then(|k| view.inner.keys.action(k))
                {
                    Some(action) => view.inner.session.borrow_mut().apply(action),
                    None => return glib::Propagation::Proceed,
                },
            };
            if outcome.needs_redraw() {
                view.area.queue_draw();
            }
            // A bound key the engine declined (Back with nothing to go
            // back to) reaches whatever is behind this widget.
            if outcome.consumed() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.area.add_controller(key);
    }

    fn install_pointer(&self) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        // Set when a press starts a selection rather than following a
        // link. A press that then never moves is a tap, and `drag_end`
        // decides what a tap there means: a word, or a page turn.
        let tap = Rc::new(Cell::new(false));
        {
            let view = self.clone();
            let tap = tap.clone();
            drag.connect_drag_begin(move |_, x, y| {
                view.area.grab_focus();
                tap.set(false);
                let mut s = view.inner.session.borrow_mut();
                // A press on a link follows it rather than starting a
                // selection there; one the engine will not follow (an
                // external URL) is the shell's.
                if let Some(href) = s.link_at(x as f32, y as f32) {
                    if s.follow_link(&href) {
                        drop(s);
                        view.area.queue_draw();
                        return;
                    }
                    drop(s);
                    if let Some(cb) = &view.inner.callbacks.borrow().link {
                        cb(&href);
                    }
                    return;
                }
                let had_selection = s.selected_range().is_some();
                s.selection_begin(x as f32, y as f32);
                tap.set(true);
                drop(s);
                if had_selection {
                    view.notify_selection();
                }
                view.area.queue_draw();
            });
        }
        {
            let view = self.clone();
            drag.connect_drag_update(move |gesture, dx, dy| {
                if let Some((sx, sy)) = gesture.start_point() {
                    let (x, y) = ((sx + dx) as f32, (sy + dy) as f32);
                    view.inner.session.borrow_mut().selection_drag(x, y);
                    view.area.queue_draw();
                }
            });
        }
        {
            let view = self.clone();
            drag.connect_drag_end(move |gesture, dx, dy| {
                let was_tap = tap.replace(false);
                let Some((x, y)) = gesture.start_point() else {
                    return;
                };
                // A press that wandered was a selection, and the drag
                // handlers already have it; say so once it is done.
                if !was_tap || dx.abs() > TAP_SLOP || dy.abs() > TAP_SLOP {
                    if view.inner.session.borrow().selected_range().is_some() {
                        view.notify_selection();
                    }
                    return;
                }
                let mut s = view.inner.session.borrow_mut();
                // The press anchored an empty selection; drop it before
                // anything else, so the anchor does not outlive the page.
                s.selection_clear();
                // A tap on a word is a dictionary lookup — Kalam's
                // tap-to-look-up — and takes precedence over the page-turn
                // zones, so a word near the edge is still a word.
                if let Some(word) = word_at(&mut s, x as f32, y as f32) {
                    drop(s);
                    view.area.queue_draw();
                    if let Some(cb) = &view.inner.callbacks.borrow().word {
                        cb(&word);
                    }
                    return;
                }
                let Some(metrics) = s.metrics() else { return };
                let Some(action) = view.inner.zones.action_at(x as f32, y as f32, &metrics) else {
                    return;
                };
                let outcome = s.apply(action);
                drop(s);
                if outcome.needs_redraw() {
                    view.area.queue_draw();
                }
            });
        }
        self.area.add_controller(drag);
    }

    /// The engine's background loader (image units) nudges the widget
    /// through a channel whose receiving half runs on the main context.
    /// EPUB chapters never come this way, but the wiring costs nothing and
    /// keeps the widget correct if a book ever does.
    fn install_loader_wakeup(&self) {
        let (wake_tx, mut wake_rx) = futures_channel::mpsc::unbounded::<()>();
        self.inner.session.borrow_mut().set_waker(move || {
            let _ = wake_tx.unbounded_send(());
        });
        let view = self.clone();
        glib::spawn_future_local(async move {
            use futures_util::StreamExt;
            while wake_rx.next().await.is_some() {
                let mut s = view.inner.session.borrow_mut();
                let redraw = s.poll_loaded();
                for event in s.drain_events() {
                    if let SessionEvent::UnitFailed { spine, message } = event {
                        log::warn!("chapter {} will not load: {message}", spine + 1);
                    }
                }
                drop(s);
                if redraw {
                    view.area.queue_draw();
                }
            }
        });
    }
}

/// A highlight as Kalam's `annotations` row holds it: what
/// [`ReaderView::capture_highlight`] hands out and what
/// [`ReaderView::show_highlight`] takes back. The id is Kalam's and
/// travels beside it, never inside it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewHighlight {
    pub color: HighlightColor,
    /// The highlighted text — Kalam's `text_excerpt`.
    pub text: String,
    /// Start and end of the highlighted span as durable locators — the
    /// JSON for Kalam's `cfi` column, or two columns of its own.
    pub start: LayeredLocator,
    pub end: LayeredLocator,
}

fn host_highlight(id: i64, h: &NewHighlight) -> HostHighlight {
    HostHighlight {
        id,
        start: h.start.clone(),
        end: h.end.clone(),
        color: Some(h.color.css().to_string()),
        text: Some(h.text.clone()),
    }
}

fn position_of(s: &mut Session) -> ReadingPosition {
    ReadingPosition {
        chapter: s.spine(),
        chapter_count: s.spine_len(),
        page: s.page(),
        page_count: s.page_count(),
        fraction: s.unit_fraction(),
    }
}

/// The word under a point, with its sentence and rectangle — `None` off
/// text or on punctuation.
fn word_at(s: &mut Session, x: f32, y: f32) -> Option<TappedWord> {
    let (start, end) = s.word_at_exact(x, y)?;
    let page = s.speakable_page()?;
    let span = page
        .words
        .iter()
        .find(|w| w.locator_start == start && w.locator_end == end)?;
    let text: Vec<char> = page.text.chars().collect();
    let word: String = text
        .get(span.text_start as usize..span.text_end as usize)?
        .iter()
        .collect();
    let word = readable(&word);
    if !word.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    let sentence = sentence_around(&text, span.text_start as usize, span.text_end as usize);
    let sentence = readable(&sentence);
    let rect = union(&s.range_rects(start, end))?;
    let highlight = s.host_highlight_at(x, y);
    Some(TappedWord {
        word,
        sentence,
        rect,
        highlight,
    })
}

/// The sentence containing `[start, end)` of the page text: back to the
/// previous sentence end, forward to the next. Capped at 600 chars like
/// Kalam's own `sentenceAroundText`.
fn sentence_around(text: &[char], start: usize, end: usize) -> String {
    const CAP: usize = 600;
    let is_end = |c: char| matches!(c, '.' | '!' | '?' | '…');
    let mut from = start;
    while from > 0 && !is_end(text[from - 1]) {
        from -= 1;
    }
    let mut to = end;
    while to < text.len() && !is_end(text[to]) {
        to += 1;
    }
    // Keep the closing punctuation.
    if to < text.len() {
        to += 1;
    }
    let sentence: String = text[from..to].iter().collect();
    let sentence = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
    if sentence.chars().count() > CAP {
        sentence.chars().take(CAP).collect()
    } else {
        sentence
    }
}

/// Text as a person would type it. Publishers' files are full of invisible
/// layout hints — soft hyphens inside words (`Har\u{ad}ry`), zero-width
/// spaces after hard hyphens, word joiners, stray byte-order marks — and
/// the engine's text keeps them, because its locators are offsets into
/// that raw text. What Kalam stores and shows (`text_excerpt`, a word
/// sent to the dictionary) must not: a lookup of "Har\u{ad}ry" finds
/// nothing. Zero-width joiners stay — they are letters in Devanagari and
/// Arabic, and hold emoji together. Whitespace runs collapse to one space.
fn readable(text: &str) -> String {
    let stripped: String = text
        .chars()
        .filter(|c| !matches!(c, '\u{00AD}' | '\u{200B}' | '\u{2060}' | '\u{FEFF}'))
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The bounding box of several rects; `None` of none.
fn union(rects: &[Rect]) -> Option<Rect> {
    let mut iter = rects.iter();
    let first = iter.next()?;
    let (mut x0, mut y0) = (first.origin.x, first.origin.y);
    let (mut x1, mut y1) = (x0 + first.size.w, y0 + first.size.h);
    for r in iter {
        x0 = x0.min(r.origin.x);
        y0 = y0.min(r.origin.y);
        x1 = x1.max(r.origin.x + r.size.w);
        y1 = y1.max(r.origin.y + r.size.h);
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// A GDK keyval name in the engine's key vocabulary — the same table as
/// the reference viewer's.
fn engine_key(name: &str) -> Option<Key> {
    Some(match name {
        "Right" => Key::ArrowRight,
        "Left" => Key::ArrowLeft,
        "Up" => Key::ArrowUp,
        "Down" => Key::ArrowDown,
        "Page_Down" => Key::PageDown,
        "Page_Up" => Key::PageUp,
        "space" => Key::Space,
        "BackSpace" => Key::Backspace,
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_text_drops_the_publishers_invisible_hints() {
        // Straight from a real book: soft hyphens inside words, a
        // zero-width space after a hard hyphen.
        assert_eq!(
            readable("Har\u{ad}ry ar\u{ad}rived  at the horse-\u{200b}like\nteeth."),
            "Harry arrived at the horse-like teeth."
        );
        // A joiner is part of the word in scripts that use it.
        assert_eq!(readable("\u{feff}क\u{94d}\u{200d}ष"), "क\u{94d}\u{200d}ष");
        assert_eq!(readable("  "), "");
    }

    #[test]
    fn sentence_is_cut_at_punctuation_and_collapsed() {
        let text: Vec<char> = "First one. The  bank\nwas closed! Third?".chars().collect();
        let start = text.iter().position(|&c| c == 'b').unwrap();
        assert_eq!(
            sentence_around(&text, start, start + 4),
            "The bank was closed!"
        );
        assert_eq!(sentence_around(&text, 0, 5), "First one.");
        let last = text.len() - 6;
        assert_eq!(sentence_around(&text, last, last + 5), "Third?");
    }

    #[test]
    fn union_boxes_every_rect() {
        let rects = [
            Rect::new(10.0, 10.0, 5.0, 5.0),
            Rect::new(20.0, 0.0, 5.0, 5.0),
        ];
        assert_eq!(union(&rects), Some(Rect::new(10.0, 0.0, 15.0, 15.0)));
        assert_eq!(union(&[]), None);
    }

    #[test]
    fn keys_translate_like_the_reference_viewer() {
        assert_eq!(engine_key("Right"), Some(Key::ArrowRight));
        assert_eq!(engine_key("space"), Some(Key::Space));
        assert_eq!(engine_key("N"), Some(Key::Char('n')));
        assert_eq!(engine_key("Shift_L"), None);
    }
}
