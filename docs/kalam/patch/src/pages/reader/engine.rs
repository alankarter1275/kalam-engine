//! The reading engine, wired to the reader page.
//!
//! This file replaces the old in-app browser page and its JS bridge. Every
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
/// model reacts in `update_with_view` like it did for the old payloads, so
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

/// One sense as the card draws it.
///
/// `pos` is per-sense (the old popup grouped senses by part of speech);
/// `hinted` is the Lesk "likely here" sense, which keeps its index into
/// the *flat* sense list however the card then groups them.
pub(crate) struct DictSense {
    pub(crate) pos: Option<String>,
    pub(crate) def: String,
    pub(crate) example: Option<String>,
    pub(crate) hinted: bool,
}

/// What the dictionary card shows — the same fields the JS popup was fed,
/// flattened to plain strings so this file does not depend on the exact
/// shape of `EntryData`'s optional fields.
pub(crate) struct DictCard {
    pub(crate) word: String,
    pub(crate) pronunciation: Option<String>,
    /// The word-level part-of-speech list, already joined with middots.
    pub(crate) pos: Option<String>,
    pub(crate) senses: Vec<DictSense>,
    pub(crate) synonyms: Vec<String>,
    pub(crate) antonyms: Vec<String>,
    pub(crate) idioms: Vec<(String, String)>,
    pub(crate) suggestions: Vec<String>,
    pub(crate) saved: bool,
}

impl DictCard {
    /// `Sense.example` is an `Option<String>` in this tree, so the empty
    /// filter applies to it directly; wrapping it in `Option::<String>::from`
    /// is a useless conversion, which `-D warnings` rejects.
    /// `EntryData.pos` is a `Vec<String>`, joined with middots below.
    pub(crate) fn from_entry(
        data: &crate::db::EntryData,
        pronunciation: Option<String>,
        saved: bool,
        hint: Option<usize>,
    ) -> DictCard {
        let senses: Vec<DictSense> = data
            .senses
            .iter()
            .enumerate()
            .map(|(i, s)| DictSense {
                pos: s.pos.clone(),
                def: s.def.clone(),
                example: s.example.clone().filter(|e| !e.is_empty()),
                hinted: hint == Some(i),
            })
            .collect();
        DictCard {
            word: data.word.clone(),
            pronunciation,
            // Kalam's `EntryData.pos` is the word-level list (`Vec<String>`),
            // not a lone `String`/`Option<String>`; the old WebKit popup
            // showed it joined with a middot, so this does too.
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

/// The parts of speech the old popup listed first, in this order. Anything
/// else keeps its first-seen order; senses with no part of speech land last
/// and get no divider row at all.
const POS_GROUP_ORDER: [&str; 4] = ["noun", "verb", "adjective", "adverb"];

/// How a sense's part of speech becomes a group key: trimmed, lowercased,
/// empty when the entry does not say.
fn pos_group_key(pos: Option<&str>) -> String {
    pos.unwrap_or("").trim().to_lowercase()
}

/// The distinct group keys in display order (the JS `buildPosGroups`).
fn pos_group_keys(senses: &[DictSense]) -> Vec<String> {
    let mut first_seen: Vec<String> = Vec::new();
    for sense in senses {
        let key = pos_group_key(sense.pos.as_deref());
        if !first_seen.contains(&key) {
            first_seen.push(key);
        }
    }
    // A plain loop, not `.iter().filter(...)`: the adapter hands the closure
    // `&&str` and the extra reference layer makes the comparison read as a
    // different type than it is (this cost two CI runs to get right).
    let mut ordered: Vec<String> = Vec::new();
    for known in POS_GROUP_ORDER {
        if first_seen.iter().any(|key| key.as_str() == known) {
            ordered.push(known.to_string());
        }
    }
    for key in first_seen {
        if !POS_GROUP_ORDER.contains(&key.as_str()) {
            ordered.push(key);
        }
    }
    ordered
}

/// For each sense, the divider label that precedes it, if any. A divider
/// only appears before the *first* sense of a labelled group, and the
/// unlabelled group never gets one — so its senses simply continue under
/// the last labelled heading.
fn pos_dividers(senses: &[DictSense]) -> Vec<Option<String>> {
    let mut dividers = vec![None; senses.len()];
    for key in pos_group_keys(senses) {
        if key.is_empty() {
            continue;
        }
        if let Some(first) = senses
            .iter()
            .position(|sense| pos_group_key(sense.pos.as_deref()) == key)
        {
            dividers[first] = Some(key.to_uppercase());
        }
    }
    dividers
}

/// A round 26 px icon button — the card's save / find / copy cluster.
fn icon_button(glyph: &str, tooltip: &str) -> gtk::Button {
    let btn = gtk::Button::with_label(glyph);
    btn.add_css_class("kalam-reader-dict-icon");
    btn.set_tooltip_text(Some(tooltip));
    btn
}

/// The header: word, pronunciation, part-of-speech pill, and the actions.
fn dict_header(
    host: &gtk::Widget,
    card: &DictCard,
    sender: &ComponentSender<ReaderModel>,
) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.add_css_class("kalam-reader-dict-head");

    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.add_css_class("kalam-reader-dict-head-left");
    left.set_hexpand(true);

    let word = gtk::Label::new(Some(&card.word));
    word.add_css_class("kalam-reader-dict-word");
    word.set_xalign(0.0);
    word.set_halign(gtk::Align::Start);
    word.set_wrap(true);
    left.append(&word);

    if let Some(pronunciation) = &card.pronunciation {
        let pron = gtk::Label::new(Some(pronunciation));
        pron.add_css_class("kalam-reader-dict-pron");
        pron.set_xalign(0.0);
        pron.set_halign(gtk::Align::Start);
        left.append(&pron);
    }

    // The pill is redundant once the senses carry their own dividers, so it
    // survives only for entries with a single group (the old popup's rule).
    if let Some(pos) = &card.pos {
        if pos_group_keys(&card.senses).len() < 2 {
            let pill = gtk::Label::new(Some(pos));
            pill.add_css_class("kalam-reader-dict-pos");
            pill.set_halign(gtk::Align::Start);
            left.append(&pill);
        }
    }
    header.append(&left);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.add_css_class("kalam-reader-dict-actions");
    actions.set_valign(gtk::Align::Start);

    // Save: a ☆ that is a ✓ (and disabled) once the word is in the sidebar.
    let save = icon_button(
        if card.saved { "\u{2713}" } else { "\u{2606}" },
        if card.saved { "Saved" } else { "Save word" },
    );
    if card.saved {
        save.add_css_class("saved");
    }
    save.set_sensitive(!card.saved && !card.senses.is_empty());
    if !card.saved {
        let tx = sender.input_sender().clone();
        save.connect_clicked(move |_| {
            let _ = tx.send(ReaderMsg::SaveCurrentWord);
        });
    }
    actions.append(&save);

    // Find in chapter: the engine's `ReaderView` exposes no search yet
    // (kalam-engine R12f defers it), so the button exists and is disabled
    // rather than silently missing.
    let find = gtk::Button::new();
    find.add_css_class("kalam-reader-dict-icon");
    find.set_tooltip_text(Some("Find in chapter (not available yet)"));
    let icon = gtk::Image::from_icon_name("system-search-symbolic");
    icon.set_pixel_size(14);
    find.set_child(Some(&icon));
    find.set_sensitive(false);
    actions.append(&find);

    // Copy: the word and its numbered definitions, the old popup's text.
    let copy = icon_button("\u{29c9}", "Copy");
    let text = card.clipboard_text();
    let host = host.clone();
    copy.connect_clicked(move |btn| {
        // `Widget::clipboard()` is this widget display's
        // `gdk::Display::clipboard()`, and is what the reader's own Copy
        // action uses. The glyph flips to a tick for 900 ms as feedback.
        host.clipboard().set_text(&text);
        btn.set_label("\u{2713}");
        let btn = btn.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
            btn.set_label("\u{29c9}");
        });
    });
    actions.append(&copy);

    header.append(&actions);
    header
}

impl DictCard {
    /// What the copy button puts on the clipboard: the word, then every
    /// sense numbered — the old popup's format, exactly.
    fn clipboard_text(&self) -> String {
        let mut text = self.word.clone();
        for (i, sense) in self.senses.iter().enumerate() {
            text.push('\n');
            text.push_str(&format!("{}. {}", i + 1, sense.def));
        }
        text
    }
}

fn section_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&text.to_uppercase()));
    label.add_css_class("kalam-reader-dict-section");
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label
}

fn pos_divider(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("kalam-reader-dict-divider");
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label
}

/// One numbered sense: number, optional LIKELY HERE badge, definition,
/// optional example.
fn sense_row(sense: &DictSense, number: usize) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("kalam-reader-dict-sense");

    let num = gtk::Label::new(Some(&format!("{number}.")));
    num.add_css_class("kalam-reader-dict-num");
    num.set_valign(gtk::Align::Start);
    row.append(&num);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);

    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    line.set_hexpand(true);
    if sense.hinted {
        let badge = gtk::Label::new(Some(&"LIKELY HERE".to_uppercase()));
        badge.add_css_class("kalam-reader-dict-badge");
        badge.set_valign(gtk::Align::Start);
        line.append(&badge);
    }
    let def = gtk::Label::new(Some(&sense.def));
    def.add_css_class("kalam-reader-dict-def");
    def.set_wrap(true);
    def.set_xalign(0.0);
    def.set_halign(gtk::Align::Start);
    def.set_hexpand(true);
    def.set_max_width_chars(34);
    line.append(&def);
    text.append(&line);

    if let Some(example) = &sense.example {
        let ex = gtk::Label::new(Some(example));
        ex.add_css_class("kalam-reader-dict-example");
        ex.set_wrap(true);
        ex.set_xalign(0.0);
        ex.set_halign(gtk::Align::Start);
        ex.set_max_width_chars(34);
        text.append(&ex);
    }
    row.append(&text);
    row
}

/// The definitions section: the first three senses, the rest behind
/// "Show N more", with the part-of-speech dividers the old popup drew.
fn append_definitions(body: &gtk::Box, card: &DictCard) {
    body.append(&section_label("Definitions"));
    let dividers = pos_dividers(&card.senses);

    let shown = card.senses.len().min(3);
    let first = gtk::Box::new(gtk::Orientation::Vertical, 10);
    first.add_css_class("kalam-reader-dict-senses");
    for (i, sense) in card.senses.iter().take(shown).enumerate() {
        if let Some(group) = &dividers[i] {
            first.append(&pos_divider(group));
        }
        first.append(&sense_row(sense, i + 1));
    }
    body.append(&first);

    let extra = &card.senses[shown..];
    if extra.is_empty() {
        return;
    }
    let hidden = gtk::Box::new(gtk::Orientation::Vertical, 10);
    hidden.add_css_class("kalam-reader-dict-senses");
    for (offset, sense) in extra.iter().enumerate() {
        let i = shown + offset;
        if let Some(group) = &dividers[i] {
            hidden.append(&pos_divider(group));
        }
        hidden.append(&sense_row(sense, i + 1));
    }
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::None);
    revealer.set_reveal_child(false);
    revealer.set_child(Some(&hidden));
    body.append(&revealer);

    let more = gtk::Button::with_label(&format!("Show {} more", extra.len()));
    more.add_css_class("kalam-reader-dict-more");
    more.set_halign(gtk::Align::Start);
    let count = extra.len();
    let showing = std::rc::Rc::new(std::cell::Cell::new(false));
    more.connect_clicked(move |btn| {
        let next = !showing.get();
        showing.set(next);
        revealer.set_reveal_child(next);
        let label = if next {
            "Show less".to_string()
        } else {
            format!("Show {count} more")
        };
        btn.set_label(&label);
    });
    body.append(&more);
}

/// A row of clickable chips (synonyms, antonyms, suggestions). FlowBox,
/// because the old popup's chips wrapped onto as many lines as they needed.
fn append_chips(body: &gtk::Box, words: &[String], antonym: bool, sender: &ComponentSender<ReaderModel>) {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .max_children_per_line(3)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .build();
    flow.add_css_class("kalam-reader-dict-chips");
    for word in words {
        let chip = gtk::Button::with_label(word);
        chip.add_css_class("kalam-reader-dict-chip");
        chip.add_css_class(if antonym {
            "kalam-reader-dict-chip-ant"
        } else {
            "kalam-reader-dict-chip-syn"
        });
        let tx = sender.input_sender().clone();
        let word = word.clone();
        chip.connect_clicked(move |_| {
            let _ = tx.send(ReaderMsg::DictSearchSelect(word.clone()));
        });
        flow.insert(&chip, -1);
    }
    body.append(&flow);
}

fn append_sections(body: &gtk::Box, card: &DictCard, sender: &ComponentSender<ReaderModel>) {
    if !card.senses.is_empty() {
        append_definitions(body, card);
    }
    if !card.synonyms.is_empty() {
        body.append(&section_label("Synonyms"));
        append_chips(body, &card.synonyms, false, sender);
    }
    if !card.antonyms.is_empty() {
        body.append(&section_label("Antonyms"));
        append_chips(body, &card.antonyms, true, sender);
    }
    if !card.idioms.is_empty() {
        body.append(&section_label("Idioms"));
        for (phrase, def) in &card.idioms {
            let item = gtk::Box::new(gtk::Orientation::Vertical, 3);
            item.add_css_class("kalam-reader-dict-idiom");
            let phrase_label = gtk::Label::new(Some(phrase));
            phrase_label.add_css_class("kalam-reader-dict-idiom-phrase");
            phrase_label.set_xalign(0.0);
            phrase_label.set_halign(gtk::Align::Start);
            phrase_label.set_wrap(true);
            item.append(&phrase_label);
            let def_label = gtk::Label::new(Some(def));
            def_label.add_css_class("kalam-reader-dict-idiom-def");
            def_label.set_xalign(0.0);
            def_label.set_halign(gtk::Align::Start);
            def_label.set_wrap(true);
            def_label.set_max_width_chars(34);
            item.append(&def_label);
            body.append(&item);
        }
    }
    if card.senses.is_empty() && !card.suggestions.is_empty() {
        // The old popup's wording: a phrase's pieces are not "did you mean".
        let label = if card.word.split_whitespace().count() > 1 {
            "Words in this phrase"
        } else {
            "Did you mean"
        };
        body.append(&section_label(label));
        append_chips(body, &card.suggestions, false, sender);
    }
    if card.senses.is_empty() && card.suggestions.is_empty() {
        let empty = gtk::Label::new(Some(&format!(
            "No entry for \u{2018}{}\u{2019}.",
            card.word
        )));
        empty.add_css_class("kalam-reader-dict-empty");
        empty.set_xalign(0.0);
        empty.set_halign(gtk::Align::Start);
        body.append(&empty);
    }
}

/// The dictionary card: the old WebKit popup's design, rebuilt as GTK
/// widgets (kalam-engine docs/kalam/RESEARCH.md §R12f).
///
/// Same input (`DictCard`), same three messages, same anchoring as before.
/// Two deliberate differences, both reported: "Find in chapter" is present
/// but disabled because `ReaderView` exposes no search, and the 320 px
/// width is a minimum rather than a maximum — GTK has no `max-width`, so a
/// very narrow window clamps the popover itself.
pub(crate) fn build_dict_popover(
    host: &gtk::Widget,
    rect: &gtk::gdk::Rectangle,
    card: &DictCard,
    sender: &ComponentSender<ReaderModel>,
) -> gtk::Popover {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("kalam-reader-dict");
    root.set_size_request(320, -1);
    root.append(&dict_header(host, card, sender));

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("kalam-reader-dict-body");
    append_sections(&body, card, sender);

    // The body scrolls at 300 px and never shows a scrollbar; a 40 px
    // gradient covers the cut so it reads as a fade, not a clip.
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .max_content_height(300)
        .propagate_natural_height(true)
        .child(&body)
        .build();
    scroll.add_css_class("kalam-reader-dict-scroll");

    let fade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fade.add_css_class("kalam-reader-dict-fade");
    fade.set_valign(gtk::Align::End);
    fade.set_size_request(-1, 40);
    fade.set_can_target(false);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&scroll));
    overlay.add_overlay(&fade);
    root.append(&overlay);

    let popover = gtk::Popover::new();
    popover.set_child(Some(&root));
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
    fn legacy_rows_are_not_mistaken_for_locators() {
        assert!(range_from_json("").is_none());
        assert!(range_from_json("epubcfi(/6/4!/4/2/1:0)").is_none());
        assert!(range_from_json("{\"kalam_locator\":2}").is_none());
    }
}
