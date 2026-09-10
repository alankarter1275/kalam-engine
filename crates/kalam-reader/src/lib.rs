//! kalam-reader — the reading widget Kalam embeds.
//!
//! Kalam (the calibre-alt GTK4 app) used to show books in a WebKit view.
//! This crate is the replacement: one GTK4 widget, [`ReaderView`], with
//! the kalam-engine (a stripped Chapbook) behind it instead of a browser.
//! Same paper, same ink, same Literata, no JavaScript, no network, and a
//! first page in a fraction of the time on a slow disk.
//!
//! What the crate holds:
//!
//! * [`ReaderView`] — the widget. Open a book, put `widget()` in a
//!   container, connect the callbacks, forward Kalam's messages to its
//!   methods. Two [`ReadingMode`]s: one page at a time, or the whole book
//!   as one scrolling strip. See `view.rs`; the strip's arithmetic is in
//!   `scroll.rs`, the chapter divider it draws between chapters in
//!   `divider.rs`.
//! * [`KalamPrefs`] / [`KalamTheme`] / [`HighlightColor`] — Kalam's
//!   reading preferences and colours, copied from Kalam's own source, and
//!   the one function that turns them into engine settings. See
//!   `prefs.rs`.
//! * The bundled fonts (Literata and Noto Sans, compiled in). See
//!   `fonts.rs`.
//!
//! What it deliberately does not hold: a database, a dictionary, a menu, a
//! settings panel. Those are Kalam's; the widget reports and obeys.
//!
//! # Wiring it into Kalam (the short version)
//!
//! ```ignore
//! let view = ReaderView::open(&path, prefs, &ReaderOptions::default())?;
//! overlay.set_child(Some(view.widget()));
//! view.connect_position(move |pos| {
//!     catalog.save_progress(book_id, pos.chapter, pos.fraction);
//!     // and pos.locator, serialised, for the durable record
//! });
//! view.connect_word(move |w| sender.input(ReaderMsg::DictSearch(w.word.clone())));
//! // ReaderMsg::NextChapter => view.next_chapter(), and so on.
//! ```

mod divider;
mod fonts;
mod prefs;
mod scroll;
mod view;

pub use chapbook_core::{
    ChapbookError, LayeredLocator, Point, Quote, Rect, Size, TocEntry, LOCATOR_VERSION,
};
pub use chapbook_reader::Highlight;
pub use fonts::{font_source, SANS_FONT};
pub use prefs::{HighlightColor, KalamPrefs, KalamTheme, BODY_FONT};
pub use view::{
    NewHighlight, ReaderOptions, ReaderView, ReadingMode, ReadingPosition, SelectedText,
    TappedWord, DEFAULT_CACHE_BUDGET,
};
