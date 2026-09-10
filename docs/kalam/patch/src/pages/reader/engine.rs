//! The reading engine, wired to the reader page.
//!
//! This file replaces the WebKit `WebView` + JavaScript bridge. Every
//! line of JS the old reader shipped is now a method call on
//! [`ReaderView`], and everything the JS used to *send back* (progress,
//! taps, selections, links) arrives through the four callbacks
//! [`wire`] installs.
//!
//! Drop this file in as `src/pages/reader/engine.rs`, add `mod engine;`
//! to `src/pages/reader/mod.rs`, and follow docs/kalam/INTEGRATION.md
//! in the kalam-engine repo for the edits around it.
//!
//! Two things live here besides the wiring:
//!
//! * `locator_to_json` / `locator_from_json` — the engine's durable
//!   position record (`LayeredLocator`) written into and read back out
//!   of the `annotations.cfi` column, which was unused. The engine's
//!   type has no serde, so the JSON is built field by field.
//! * the GTK replacements for the two things the JS drew itself: the
//!   selection chip (highlight colours / quote / dictionary / copy) and
//!   the dictionary popover.

use super::mod_model::ReaderModel;
use super::types::*;
use crate::db::{Annotation, HighlightColor as DbColor};
use crate::epub_book::ReadingTheme;
use gtk::prelude::*;
use kalam_reader::{
    HighlightColor, KalamPrefs, KalamTheme, LayeredLocator, NewHighlight, Quote, ReaderOptions,
    ReaderView, ReadingMode, ReadingPosition, SelectedText, TappedWord, LOCATOR_VERSION,
};
use relm4::ComponentSender;
use std::path::Path;

// ---------------------------------------------------------------------
// Preferences: Kalam's stored values → the engine's
// ---------------------------------------------------------------------

/// Kalam's four reading theme names are the engine's four; the engine
/// took its colours from `ReadingTheme::swatch()`, so they match.
pub(crate) fn engine_theme(theme: ReadingTheme) -> KalamTheme {
    KalamTheme::from_name(theme.as_str()).unwrap_or_default()
}

pub(crate) fn engine_prefs(
    theme: ReadingTheme,
    font_px: u32,
    line_height: f32,
    column_px: u32,
) -> KalamPrefs {
    KalamPrefs {
        theme: engine_theme(theme),
        font_px: font_px as f32,
        line_height,
        column_px: column_px as f32,
    }
    .clamped()
}

/// Kalam's stored colour name → the engine's. Unknown names (and the
/// legacy "rose") fall back the same way `HighlightColor::from_str_lossy`
/// does: through Kalam's own parser first.
pub(crate) fn engine_color(name: &str) -> HighlightColor {
    HighlightColor::from_name(DbColor::from_str_lossy(name).as_str()).unwrap_or_default()
}

/// Open the book for reading. Bundled fonts only, the default cache
/// budget — the settings the engine was tuned with on the target
/// machine. `Err` for a file the engine cannot read; the caller shows
/// the "Could not open book" placeholder as before.
pub(crate) fn open_engine(
    file_path: &Path,
    prefs: KalamPrefs,
) -> Result<ReaderView, kalam_reader::ChapbookError> {
    crate::timing::span("book_open");
    let view = ReaderView::open(file_path, prefs, &ReaderOptions::default());
    crate::timing::span_end("book_open");
    view
}

/// The reader's "continuous scroll" preference, kept under the
/// `reader.` prefix so it is global like the other reading prefs.
pub(crate) const PREF_SCROLLED: &str = "reader.scrolled";

pub(crate) fn mode_from_pref(value: i64) -> ReadingMode {
    if value != 0 {
        ReadingMode::Scrolled
    } else {
        ReadingMode::Paged
    }
}

// ---------------------------------------------------------------------
// Wiring: what the engine tells the page
// ---------------------------------------------------------------------

/// Install the four callbacks. Each one only *sends a message*; the
/// model reacts in `update_with_view` like it did for `JsRaw`, so the
/// borrow rules stay simple (a callback never touches the model).
pub(crate) fn wire(view: &ReaderView, sender: &ComponentSender<ReaderModel>) {
    let tx = sender.input_sender().clone();
    view.connect_position(move |pos: &ReadingPosition| {
        let _ = tx.send(ReaderMsg::EnginePosition(pos.chapter, pos.fraction));
    });

    let tx = sender.input_sender().clone();
    view.connect_word(move |word: &TappedWord| {
        let _ = tx.send(ReaderMsg::EngineWord {
            word: word.word.clone(),
            sentence: word.sentence.clone(),
            rect: gdk_rect(word.rect),
            highlight: word.highlight,
        });
    });

    let tx = sender.input_sender().clone();
    view.connect_selection(move |sel: Option<&SelectedText>| {
        let _ = tx.send(ReaderMsg::EngineSelection(
            sel.map(|s| (s.text.clone(), gdk_rect(s.rect))),
        ));
    });

    view.connect_external_link(|href: &str| {
        if href.starts_with("http://") || href.starts_with("https://") {
            let launcher = gtk::UriLauncher::new(href);
            launcher.launch(None::<&gtk::Window>, gtk::gio::Cancellable::NONE, |_| {});
        }
    });
}

/// An engine rect (widget coordinates, f32) as the rectangle a
/// `gtk::Popover::set_pointing_to` wants.
pub(crate) fn gdk_rect(rect: kalam_reader::Rect) -> gtk::gdk::Rectangle {
    gtk::gdk::Rectangle::new(
        rect.origin.x.floor() as i32,
        rect.origin.y.floor() as i32,
        (rect.size.w.ceil() as i32).max(1),
        (rect.size.h.ceil() as i32).max(1),
    )
}

// ---------------------------------------------------------------------
// Highlights: annotation rows ↔ engine highlights
// ---------------------------------------------------------------------

/// The engine's highlight for a stored row, if the row was made by the
/// engine (its `cfi` column holds the two locators). Rows from the
/// WebKit reader have DOM paths instead and cannot be placed; they stay
/// in the sidebar list but are not painted.
pub(crate) fn highlight_of(annotation: &Annotation) -> Option<NewHighlight> {
    if annotation.kind != "highlight" {
        return None;
    }
    let (start, end) = range_from_json(annotation.cfi.as_deref()?)?;
    Some(NewHighlight {
        color: engine_color(&annotation.color),
        text: annotation.text_excerpt.clone(),
        start,
        end,
    })
}

/// Paint every placeable highlight of the book. Call after any change
/// to the annotations table that the widget did not make itself.
pub(crate) fn show_all_highlights(view: &ReaderView, annotations: &[Annotation]) {
    let placed: Vec<(i64, NewHighlight)> = annotations
        .iter()
        .filter_map(|a| highlight_of(a).map(|h| (a.id, h)))
        .collect();
    view.set_highlights(placed.iter().map(|(id, h)| (*id, h)));
}

// ---------------------------------------------------------------------
// Locator JSON — the `cfi` column's new contents
// ---------------------------------------------------------------------

/// One locator as JSON. Field names are the engine's, so the column
/// reads back with `locator_from_json` for as long as the engine keeps
/// them (it versions the offset with `locator_version`).
pub(crate) fn locator_to_json(l: &LayeredLocator) -> serde_json::Value {
    serde_json::json!({
        "spine_href": l.spine_href,
        "spine_index": l.spine_index,
        "char_offset": l.char_offset,
        "locator_version": l.locator_version,
        "quote": {
            "prefix": l.quote.prefix,
            "exact": l.quote.exact,
            "suffix": l.quote.suffix,
        },
        "spine_fraction": l.spine_fraction,
        "book_progression": l.book_progression,
    })
}

pub(crate) fn locator_from_json(v: &serde_json::Value) -> Option<LayeredLocator> {
    let s = |key: &str| v.get(key).and_then(|x| x.as_str()).map(str::to_owned);
    let quote = v.get("quote")?;
    let q = |key: &str| {
        quote
            .get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Some(LayeredLocator {
        spine_href: s("spine_href")?,
        spine_index: v.get("spine_index")?.as_u64()? as usize,
        char_offset: v.get("char_offset")?.as_u64()? as u32,
        locator_version: v
            .get("locator_version")
            .and_then(|x| x.as_u64())
            .unwrap_or(LOCATOR_VERSION as u64) as u32,
        quote: Quote {
            prefix: q("prefix"),
            exact: q("exact"),
            suffix: q("suffix"),
        },
        spine_fraction: v.get("spine_fraction").and_then(|x| x.as_f64()).unwrap_or(0.0),
        book_progression: v
            .get("book_progression")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0),
    })
}

/// The `cfi` text for a highlight: both ends, under a version tag so a
/// later format can tell itself apart.
pub(crate) fn range_to_json(start: &LayeredLocator, end: &LayeredLocator) -> String {
    serde_json::json!({
        "kalam_locator": 1,
        "start": locator_to_json(start),
        "end": locator_to_json(end),
    })
    .to_string()
}

pub(crate) fn range_from_json(text: &str) -> Option<(LayeredLocator, LayeredLocator)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("kalam_locator")?.as_u64()? != 1 {
        return None;
    }
    Some((
        locator_from_json(v.get("start")?)?,
        locator_from_json(v.get("end")?)?,
    ))
}

/// The `cfi` text for a *position* (one locator) — what `save_progress`
/// can store beside `(chapter_index, fraction)` if you add a column for
/// it later. Not used in v1; the pair is enough for the engine's
/// `goto_chapter`.
#[allow(dead_code)]
pub(crate) fn position_to_json(l: &LayeredLocator) -> String {
    serde_json::json!({ "kalam_locator": 1, "at": locator_to_json(l) }).to_string()
}

// ---------------------------------------------------------------------
// The selection chip and the dictionary popover (GTK, no JS)
// ---------------------------------------------------------------------

/// The chip that appears over a finished selection: five colours, then
/// quote, dictionary, copy — the same four actions the JS chip had.
/// Returns a popover already pointed at the selection; the caller keeps
/// it in the model so `None` (selection cleared) can pop it down.
pub(crate) fn build_selection_chip(
    host: &gtk::Widget,
    rect: &gtk::gdk::Rectangle,
    sender: &ComponentSender<ReaderModel>,
) -> gtk::Popover {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("kalam-reader-chip");
    for color in [
        HighlightColor::Yellow,
        HighlightColor::Green,
        HighlightColor::Blue,
        HighlightColor::Pink,
        HighlightColor::Orange,
    ] {
        let dot = gtk::Button::new();
        dot.add_css_class("kalam-reader-chip-dot");
        dot.add_css_class(&format!("kalam-reader-chip-dot-{}", color.name()));
        dot.set_tooltip_text(Some(color.name()));
        let tx = sender.input_sender().clone();
        dot.connect_clicked(move |_| {
            let _ = tx.send(ReaderMsg::HighlightSelection(color.name().to_string()));
        });
        row.append(&dot);
    }
    for (label, msg) in [
        ("Quote", ReaderMsg::QuoteSelection),
        ("Dictionary", ReaderMsg::LookUpSelection),
        ("Copy", ReaderMsg::CopySelection),
    ] {
        let btn = gtk::Button::with_label(label);
        btn.add_css_class("kalam-reader-chip-action");
        let tx = sender.input_sender().clone();
        btn.connect_clicked(move |_| {
            let _ = tx.send(msg.clone());
        });
        row.append(&btn);
    }
    let popover = gtk::Popover::new();
    popover.set_child(Some(&row));
    popover.set_parent(host);
    popover.set_autohide(false);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Top);
    popover.set_pointing_to(Some(rect));
    popover.add_css_class("kalam-reader-chip-popover");
    popover
}

/// What the dictionary popover shows — the same fields the JS popup
/// was fed, flattened to plain strings so this file does not depend on
/// the exact shape of `EntryData`'s optional fields.
pub(crate) struct DictCard {
    pub(crate) word: String,
    pub(crate) pronunciation: Option<String>,
    pub(crate) pos: Option<String>,
    /// (definition, example, is the hinted sense)
    pub(crate) senses: Vec<(String, Option<String>, bool)>,
    pub(crate) synonyms: Vec<String>,
    pub(crate) antonyms: Vec<String>,
    pub(crate) idioms: Vec<(String, String)>,
    pub(crate) suggestions: Vec<String>,
    pub(crate) saved: bool,
}

impl DictCard {
    /// `EntryData.pos` is a `Vec<String>` (`src/db/dictionaries.rs`);
    /// the WebKit popup joined it with middle dots and so does this.
    /// `Sense.example` is taken through `Option::<String>::from`, which
    /// accepts a `String` or an `Option<String>`.
    pub(crate) fn from_entry(
        data: &crate::db::EntryData,
        pronunciation: Option<String>,
        saved: bool,
        hint: Option<usize>,
    ) -> DictCard {
        let mut senses: Vec<(String, Option<String>, bool)> = data
            .senses
            .iter()
            .enumerate()
            .map(|(i, s)| {
                (
                    s.def.clone(),
                    Option::<String>::from(s.example.clone()).filter(|e| !e.is_empty()),
                    hint == Some(i),
                )
            })
            .collect();
        // The hinted sense first, marked; the rest in their own order.
        if let Some(h) = hint.filter(|h| *h < senses.len()) {
            let hinted = senses.remove(h);
            senses.insert(0, hinted);
        }
        DictCard {
            word: data.word.clone(),
            pronunciation,
            pos: if data.pos.is_empty() {
                None
            } else {
                Some(data.pos.join(" \u{00b7} "))
            },
            senses,
            synonyms: data.synonyms.iter().map(|s| s.to_string()).collect(),
            antonyms: data.antonyms.iter().map(|s| s.to_string()).collect(),
            idioms: data
                .idioms
                .iter()
                .map(|(phrase, def)| (phrase.to_string(), def.to_string()))
                .collect(),
            suggestions: data.suggestions.iter().map(|s| s.to_string()).collect(),
            saved,
        }
    }
}

/// The dictionary popover: headword, pronunciation, part of speech,
/// senses (the hinted one first and starred), synonyms, antonyms,
/// idioms, suggestions, and the save / close actions. Autohides on a
/// click outside, which sends `ClearDict` like the JS popup's close.
pub(crate) fn build_dict_popover(
    host: &gtk::Widget,
    rect: &gtk::gdk::Rectangle,
    card: &DictCard,
    sender: &ComponentSender<ReaderModel>,
) -> gtk::Popover {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.add_css_class("kalam-reader-dict");
    column.set_margin_top(10);
    column.set_margin_bottom(10);
    column.set_margin_start(10);
    column.set_margin_end(10);
    column.set_size_request(280, -1);

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let word = gtk::Label::new(Some(&card.word));
    word.add_css_class("kalam-reader-dict-word");
    word.set_halign(gtk::Align::Start);
    word.set_hexpand(true);
    head.append(&word);
    if let Some(p) = &card.pronunciation {
        let pron = gtk::Label::new(Some(p));
        pron.add_css_class("kalam-reader-dict-pron");
        head.append(&pron);
    }
    column.append(&head);

    if let Some(pos) = &card.pos {
        let pos_label = gtk::Label::new(Some(pos));
        pos_label.add_css_class("kalam-reader-dict-pos");
        pos_label.set_halign(gtk::Align::Start);
        column.append(&pos_label);
    }

    let senses = gtk::Box::new(gtk::Orientation::Vertical, 4);
    for (n, (def, example, hinted)) in card.senses.iter().take(6).enumerate() {
        let text = format!(
            "{}{}. {}",
            if *hinted { "★ " } else { "" },
            n + 1,
            super::js_bridge::truncate_def(def, 220)
        );
        let label = gtk::Label::new(Some(&text));
        label.add_css_class("kalam-reader-dict-sense");
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.set_max_width_chars(44);
        senses.append(&label);
        if let Some(example) = example {
            let ex = gtk::Label::new(Some(&format!("\u{201c}{example}\u{201d}")));
            ex.add_css_class("kalam-reader-dict-example");
            ex.set_wrap(true);
            ex.set_xalign(0.0);
            ex.set_max_width_chars(44);
            senses.append(&ex);
        }
    }
    if card.senses.is_empty() {
        let none = gtk::Label::new(Some(if card.suggestions.is_empty() {
            "No entry found."
        } else {
            "No entry found. Did you mean:"
        }));
        none.add_css_class("kalam-reader-dict-none");
        none.set_xalign(0.0);
        senses.append(&none);
    }
    column.append(&senses);

    for (title, words) in [
        ("Synonyms", &card.synonyms),
        ("Antonyms", &card.antonyms),
        ("Suggestions", &card.suggestions),
    ] {
        if words.is_empty() {
            continue;
        }
        let flow = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let cap = gtk::Label::new(Some(title));
        cap.add_css_class("kalam-reader-dict-cap");
        flow.append(&cap);
        for w in words.iter().take(6) {
            let btn = gtk::Button::with_label(w);
            btn.add_css_class("kalam-reader-dict-link");
            let tx = sender.input_sender().clone();
            let w = w.clone();
            btn.connect_clicked(move |_| {
                let _ = tx.send(ReaderMsg::DictSearchSelect(w.clone()));
            });
            flow.append(&btn);
        }
        column.append(&flow);
    }
    for (phrase, def) in card.idioms.iter().take(3) {
        let label = gtk::Label::new(Some(&format!(
            "{phrase} \u{2014} {}",
            super::js_bridge::truncate_def(def, 140)
        )));
        label.add_css_class("kalam-reader-dict-idiom");
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.set_max_width_chars(44);
        column.append(&label);
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let save = gtk::Button::with_label(if card.saved { "Saved \u{2713}" } else { "Save word" });
    save.add_css_class("kalam-reader-dict-action");
    save.set_sensitive(!card.saved && !card.senses.is_empty());
    let tx = sender.input_sender().clone();
    save.connect_clicked(move |_| {
        let _ = tx.send(ReaderMsg::SaveCurrentWord);
    });
    actions.append(&save);
    let close = gtk::Button::with_label("Close");
    close.add_css_class("kalam-reader-dict-action");
    let tx = sender.input_sender().clone();
    close.connect_clicked(move |_| {
        let _ = tx.send(ReaderMsg::ClearDict);
    });
    actions.append(&close);
    column.append(&actions);

    let popover = gtk::Popover::new();
    popover.set_child(Some(&column));
    popover.set_parent(host);
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_pointing_to(Some(rect));
    popover.add_css_class("kalam-reader-dict-popover");
    let tx = sender.input_sender().clone();
    popover.connect_closed(move |_| {
        let _ = tx.send(ReaderMsg::ClearDict);
    });
    popover
}

/// Take a popover down and off its parent. A popover with `set_parent`
/// must be `unparent`ed before it is dropped, or GTK complains.
pub(crate) fn dismiss(popover: Option<gtk::Popover>) {
    if let Some(p) = popover {
        p.popdown();
        p.unparent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(offset: u32) -> LayeredLocator {
        LayeredLocator {
            spine_href: "OEBPS/ch03.xhtml".into(),
            spine_index: 3,
            char_offset: offset,
            locator_version: LOCATOR_VERSION,
            quote: Quote {
                prefix: "the quick ".into(),
                exact: "brown".into(),
                suffix: " fox".into(),
            },
            spine_fraction: 0.25,
            book_progression: 0.1,
        }
    }

    #[test]
    fn a_range_survives_the_column() {
        let text = range_to_json(&sample(10), &sample(15));
        let (start, end) = range_from_json(&text).expect("parses");
        assert_eq!(start, sample(10));
        assert_eq!(end, sample(15));
    }

    #[test]
    fn webkit_rows_are_not_mistaken_for_locators() {
        assert!(range_from_json("").is_none());
        assert!(range_from_json("epubcfi(/6/4!/4/2/1:0)").is_none());
        assert!(range_from_json("{\"kalam_locator\":2}").is_none());
    }
}
