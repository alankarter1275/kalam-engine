# Putting the engine into Kalam (step 5)

This is the recipe for swapping WebKit out of Kalam's reader page and
putting `kalam-reader` in. It is written to be followed top to bottom,
in the `calibre-alt` repo. Steps 0–3 form one build (see step 0 for
why); step 4 is the run. The
finished code for the new file ships beside this document (in the
handoff bundle: `patch/engine.rs`; in kalam-engine:
`docs/kalam/patch/src/pages/reader/engine.rs`); the rest are edits to
files you already have.

If something here does not match your tree, the tree wins — I worked
from the snapshot you committed (`kalam-src.txt`), and a few files
(`session.rs`, `lists.rs`, `panels.rs`, `db/annotations.rs`) were not
in it.

## What changes, in one paragraph

Today the reader page builds an HTML string per chapter, hands it to a
`webkit6::WebView`, and talks to it through JavaScript strings both
ways. Afterwards the page holds one `ReaderView` (a GTK widget) that
opens the EPUB itself, and talks to it by method calls: `next_chapter()`,
`set_theme(..)`, `goto_chapter(..)`. What the reader does — tap a word,
select text, turn a page — comes back through four callbacks. Nothing
else on the page changes: sidebars, settings panel, bookmarks, saved
words, dictionary lookups, reading sessions all stay as they are.

Things that go away: `webview_pool.rs`, `preload.rs`'s chapter-warming
half, most of `js_bridge.rs`, `WebViewHandlers`, `JsPayload`,
`chapter_html`/`reading_css` for the page (they can stay in
`epub_book.rs` for now — `pages/book.rs` still opens `OpenBook` for
chapter titles), and the two `webkit6`/`javascriptcore6` dependencies.

## Step 0 — the one decision: bump the GTK crates

`kalam-reader` is built on gtk4 **0.11** (glib 0.22). Kalam is on gtk4
0.9 (glib 0.20). Rust will not let two versions of `gtk4` share one
widget, so Kalam moves up. The pairing that works together:

| crate        | now                          | after                         |
|--------------|------------------------------|-------------------------------|
| relm4        | 0.9, `libadwaita`, `gnome_45` | **0.11**, `libadwaita`, `gnome_45` |
| libadwaita   | 0.7, `v1_4`                  | **0.9**, `v1_4`               |
| gtk4         | 0.9, `v4_12`                 | **0.11**, `v4_12`             |
| glib / gio   | 0.20                         | **0.22**                      |
| webkit6      | 0.4                          | *removed*                     |
| javascriptcore6 | 0.4                       | *removed*                     |

In `Cargo.toml`:

```toml
relm4 = { version = "0.11", features = ["libadwaita", "gnome_45"] }
libadwaita = { version = "0.9", features = ["v1_4"] }
gtk = { version = "0.11", package = "gtk4", features = ["v4_12"] }
glib = "0.22"
gio = { version = "0.22", package = "gio" }
kalam-reader = { git = "https://github.com/alankarter1275/kalam-engine", branch = "arena/01a07f3f-kalam-engine" }
# delete: webkit6, javascriptcore
```

**Why webkit6 goes in the same commit as the bump:** `gtk4-sys` 0.9
and 0.11 both declare `links = "gtk-4"`, and webkit6 0.4 is built on
gtk4 0.9, so Cargo refuses a graph that has the bump and WebKit at once
("multiple packages link to native library `gtk-4`") before compiling a
line. There is no intermediate state that builds: steps 0–3 are **one
build**. Keep them as separate commits for readability, but do not
expect anything to compile until step 3 is complete. Expect a handful of
small breakages from the gtk-rs 0.9 → 0.11 move elsewhere in the app
(renamed methods, `set_popover` now taking `impl IsA<Popover>`, a
`glib::Propagation` here and there) — fix those in step 3's build, on
their own commit.

`kalam-reader` asks gtk4 for `v4_16`; Cargo unifies that with Kalam's
`v4_12`, so the binary needs GTK ≥ 4.16 at build and run time (Arch has
4.20+).

**Build profile.** Kalam's `[profile.release]` has `lto = true` and
`codegen-units = 1`. On the owner's 4 GB machine that gets `rustc`
OOM-killed (`signal: 15, SIGTERM`, no error message) while compiling
`stylo`, the largest crate the engine brings in. Change it to
`lto = "thin"` and delete the `codegen-units` line; build with `-j 1`
if it is still killed. CI runners have the memory and never see this.

System side (Arch): `pacman -S gtk4 libadwaita` are already there.
`webkit2gtk-6.0` can be uninstalled at the end. The engine's git
dependency pulls stylo, which needs Python 3 at build time (Arch has
it); Kalam's toolchain must be ≥ 1.92 (the engine workspace's floor).

## Step 1 — the database: one column gains a meaning, one function is added

Highlights need a place to store the engine's position record (two
"locators", one per end of the highlight). The `annotations` table has
an unused `cfi TEXT` column; it is that.

In `db.rs`, next to `update_annotation_color`:

```rust
/// Store the engine's locator JSON for a highlight (the `cfi` column,
/// which the WebKit reader never used).
pub fn update_annotation_cfi(&self, id: i64, cfi: &str) -> Result<()> {
    let conn = self.conn();
    conn.execute(
        "UPDATE annotations SET cfi = ?1, updated_at = ?2 WHERE id = ?3",
        params![cfi, chrono_like_now(), id],
    )?;
    Ok(())
}
```

`Annotation.cfi` already exists (`Option<String>`, currently marked
`#[allow(dead_code)]` — remove the allow). Nothing else in the schema
changes: `reading_progress (chapter_index, fraction)` and
`reading_bookmarks` are exactly what the engine's `goto_chapter(index,
fraction)` takes.

**Old highlights.** Rows made by the WebKit reader hold DOM paths, not
locators; the engine cannot place them on a page. They stay in the
Highlights sidebar (text, colour, note, jump-to-chapter all still work)
but are not painted in the text. New highlights made with the engine
are painted and survive font changes, resizes and re-imports.

## Step 2 — drop the new file in

Copy `patch/engine.rs` (from the handoff bundle) to
`src/pages/reader/engine.rs` and add `mod engine;` to
`src/pages/reader/mod.rs`. It provides:

| function | what for |
|---|---|
| `engine_prefs(theme, font_px, line_height, column_px)` | Kalam's four stored prefs → the engine's `KalamPrefs` |
| `open_engine(&book.file_path, prefs)` | opens the EPUB; `Err` → show the old "Could not open book" page |
| `wire(&view, &sender)` | installs the four callbacks; each just sends a `ReaderMsg` |
| `highlight_of(&annotation)` / `show_all_highlights(&view, &rows)` | annotation rows → painted highlights |
| `range_to_json` / `range_from_json` | the `cfi` column's contents |
| `build_selection_chip(..)` / `build_dict_popover(..)` / `dismiss(..)` | the two things the JS used to draw |
| `DictCard::from_entry(..)` | flattens `EntryData` for the popover |
| `PREF_SCROLLED`, `mode_from_pref` | the new "continuous scroll" pref, `reader.scrolled` (global by the `reader.` prefix rule) |

It has two unit tests (`cargo test -p kalam engine::`) that check the
locator JSON round-trips and that old WebKit rows are rejected cleanly.

## Step 3 — the reader page, file by file

Work in this order; each is a compile point.

### 3a. `types.rs`

Delete `JsPayload` and `WebViewHandlers`. Delete
`ReaderMsg::JsRaw(String)`. Add these variants to `ReaderMsg`:

```rust
    /// From the engine: the chapter on screen and how far into it.
    EnginePosition(usize, f64),
    /// From the engine: a tapped word, for the dictionary popover.
    EngineWord { word: String, sentence: String, rect: gtk::gdk::Rectangle, highlight: Option<i64> },
    /// From the engine: a finished selection (text, where), or cleared.
    EngineSelection(Option<(String, gtk::gdk::Rectangle)>),
    /// From the selection chip.
    HighlightSelection(String),
    QuoteSelection,
    LookUpSelection,
    CopySelection,
    /// Settings: one page at a time, or one long strip.
    SetScrolled(bool),
```

`ReaderMsg` derives `Clone`; `gtk::gdk::Rectangle` is `Clone`, so that
still holds.

### 3b. `mod_model.rs`

Replace

```rust
    pub(crate) open: OpenBook,
    pub(crate) webview: webkit6::WebView,
    pub(crate) webview_handlers: Option<WebViewHandlers>,
    pub(crate) dict_lookup_rect_json: Option<String>,
```

with

```rust
    /// The reading widget; `None` when the book failed to open.
    pub(crate) view: Option<kalam_reader::ReaderView>,
    /// Chapter count and titles, kept so the page works without a view.
    pub(crate) chapter_count: usize,
    pub(crate) chapter_titles: Vec<String>,
    /// The chip over the current selection and the dictionary popover,
    /// so they can be taken down again.
    pub(crate) selection_chip: Option<gtk::Popover>,
    pub(crate) dict_popover: Option<gtk::Popover>,
    /// Where the last word tap was, for the popover that follows it.
    pub(crate) dict_anchor: Option<gtk::gdk::Rectangle>,
    /// One page at a time, or one long strip, and the strip's scrollbar
    /// (shown only in strip mode).
    pub(crate) scrolled: bool,
    pub(crate) strip_scrollbar: Option<gtk::Scrollbar>,
```

and drop the `OpenBook` import. Everything that read
`self.open.chapter_count()` reads `self.chapter_count`;
`current_chapter_title()` and `chapter_label()` read
`self.chapter_titles`. (`lists.rs`'s `rebuild_toc(&model.toc_list,
&model.open, ..)` is the one place outside the files I saw: pass it
`view.toc()` — a `Vec<kalam_reader::TocEntry>` with `label`,
`spine_index: Option<usize>`, `children` — or the `chapter_titles` list,
whichever it was walking.)

### 3c. `mod.rs` — `init`

Replace the block from `let webview = crate::webview_pool::acquire();`
through the `OpenBook::open(..)` match with:

```rust
        let catalog_theme = /* unchanged, moved up from below */;
        let catalog_font = ...;
        let catalog_line_height = ...;
        let catalog_column = ...;
        let scrolled = catalog.get_pref_i64(engine::PREF_SCROLLED, 0) != 0;

        let (book_meta, view, chapter, fraction) = if let Some(book) = book.clone() {
            let prefs = engine::engine_prefs(catalog_theme, catalog_font, catalog_line_height, catalog_column);
            match engine::open_engine(&book.file_path, prefs) {
                Ok(view) => {
                    let (ch, frac) = catalog
                        .get_reading_progress(book_id)
                        .ok()
                        .flatten()
                        .unwrap_or((0, 0.0));
                    let ch = ch.min(view.chapter_count().saturating_sub(1));
                    (book, Some(view), ch, frac)
                }
                Err(err) => {
                    eprintln!("kalam: open epub failed: {err:#}");
                    (book, None, 0, 0.0)
                }
            }
        } else {
            (Book { /* the same "Missing book" placeholder */ }, None, 0, 0.0)
        };
        let chapter_count = view.as_ref().map(|v| v.chapter_count()).unwrap_or(0);
        let chapter_titles: Vec<String> = match &view {
            Some(v) => {
                let toc = v.toc();
                (0..chapter_count)
                    .map(|i| toc_title(&toc, i).unwrap_or_else(|| format!("Chapter {}", i + 1)))
                    .collect()
            }
            None => Vec::new(),
        };
```

with this helper at the bottom of `mod.rs` (the engine's dividers use
the same rule, so the pill and the strip agree):

```rust
/// The first table-of-contents label that points at chapter `spine`,
/// searching through nested entries.
fn toc_title(entries: &[kalam_reader::TocEntry], spine: usize) -> Option<String> {
    for entry in entries {
        if entry.spine_index == Some(spine) && !entry.label.trim().is_empty() {
            return Some(entry.label.trim().to_string());
        }
        if let Some(found) = toc_title(&entry.children, spine) {
            return Some(found);
        }
    }
    None
}
```

The "Could not open book" HTML page: make it a `gtk::Label` (wrap on,
the error text, "Press Esc to go back") appended to `web_host` when
`view` is `None`.

In the `ReaderModel { .. }` literal: `view`, `chapter_count`,
`chapter_titles`, `selection_chip: None`, `dict_popover: None`,
`dict_anchor: None`, `scrolled`, `strip_scrollbar: None`; remove `open`, `webview`,
`webview_handlers`, `dict_lookup_rect_json`.

Replace `widgets.web_host.append(&webview);` with:

```rust
        if let Some(view) = &model.view {
            // The widget, with a scrollbar lying over its right edge for
            // strip mode. Over, not beside: beside it, showing the bar
            // would change the widget's width, and a new width is a
            // fresh layout of every chapter.
            let scrollbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(view.vadjustment()));
            scrollbar.set_halign(gtk::Align::End);
            scrollbar.set_valign(gtk::Align::Fill);
            scrollbar.add_css_class("kalam-reader-strip-bar");
            let overlay = gtk::Overlay::new();
            overlay.set_hexpand(true);
            overlay.set_vexpand(true);
            overlay.set_child(Some(view.widget()));
            overlay.add_overlay(&scrollbar);
            widgets.web_host.append(&overlay);

            engine::wire(view, &sender);
            if model.scrolled {
                view.set_mode(kalam_reader::ReadingMode::Scrolled);
            }
            scrollbar.set_visible(model.scrolled);
            model.strip_scrollbar = Some(scrollbar);
            view.widget().grab_focus();
        }
```

Delete the four `webview.connect_*` blocks and the
`model.webview_handlers = Some(..)` assignment. Replace

```rust
        if model.open.chapter_count() > 0 {
            load_chapter(&model);
            model.preload_next_chapter();
        }
```

with the first landing plus the highlights:

```rust
        if let Some(view) = &model.view {
            view.goto_chapter(model.chapter, model.fraction);
            engine::show_all_highlights(view, &model.all_book_annotations);
        }
```

Key controller: remove the `Key::Right` / `Key::Left` arms (the widget
turns pages on those itself, and in strip mode scrolls). Keep
`n`/`p`/`+`/`-`/`t`/`s`/`h`/`b`/`m`/`w`/`Escape` exactly as they are —
the widget leaves those keys alone and they reach the window's
controller as before.

### 3d. `mod.rs` — `update_with_view`

The pattern for every arm that used to `eval_js`: call the method on
`self.view` instead. Add a tiny helper on `ReaderModel`:

```rust
    pub(crate) fn with_view(&self, f: impl FnOnce(&kalam_reader::ReaderView)) {
        if let Some(view) = &self.view {
            f(view);
        }
    }
```

Then, arm by arm (only the changed lines shown):

* **`Close`** — unchanged, plus `engine::dismiss(self.selection_chip.take()); engine::dismiss(self.dict_popover.take());` before saving.
* **`TocSelect(idx) | JumpToChapter(idx)`**, **`JumpToLocation(idx, frac)`** — `self.go_chapter(idx, frac)` (see 3e); the guards use `self.chapter_count` and there is no `self.loading` any more (the engine never leaves the page in a half-loaded state) — delete the `loading` field and every `!self.loading` check.
* **`ToggleAnnotation(id)`** — where it called `self.go_chapter(idx, 0.0)` / `restore_pending_annotation()`, do `self.with_view(|v| { v.goto_highlight(id); })` when `engine::highlight_of(&annotation).is_some()`, else the old `go_chapter(idx, 0.0)` (a WebKit-era row can only jump to its chapter). Drop `pending_annotation_jump` entirely — the jump is immediate now.
* **`PrevChapter` / `NextChapter`** — `self.with_view(|v| v.prev_chapter())` / `next_chapter()`. The position callback updates `self.chapter`.
* **`Theme(theme)`** — `self.theme = theme; set_pref(..); self.with_view(|v| v.set_theme(engine::engine_theme(theme)));` — no reload, no `loading`. Keep `refresh_controls`/`refresh_stage`.
* **`FontDelta` / `LineHeightDelta` / `ColumnWidthDelta`** — same clamps as now, then `v.set_font_px(next as f32)` / `v.set_line_height(next)` / `v.set_column_px(next as f32)`. The engine keeps the reading position across the relayout.
* **`SetScrolled(on)`** (new) — `self.scrolled = on; set_pref(engine::PREF_SCROLLED, if on {"1"} else {"0"}); self.with_view(|v| v.set_mode(engine::mode_from_pref(on as i64)));` and `scrollbar.set_visible(on)` on `self.strip_scrollbar`. `refresh_controls = true`.
* **`JsRaw`** — delete the arm.
* **`Progress(frac)`** — keep (bookmarks list may still send it) or delete if nothing sends it.
* **`EnginePosition(chapter, fraction)`** (new) — this is the old `"progress"` + `"chapter-changed"` bridge messages in one:

  ```rust
            ReaderMsg::EnginePosition(chapter, fraction) => {
                let changed = chapter != self.chapter;
                self.chapter = chapter.min(self.chapter_count.saturating_sub(1));
                self.fraction = fraction.clamp(0.0, 1.0);
                self.save_progress();
                self.checkpoint_session();
                if changed {
                    self.reload_annotations();
                    self.reload_bookmarks();
                    self.reload_saved_words();
                    refresh_highlights = true;
                    refresh_bookmarks = true;
                    refresh_words = true;
                    refresh_toc = true;
                }
                refresh_sidebar_header = true;
                refresh_chrome = true;
            }
  ```

  `save_progress` on every page turn is what the JS did too (it throttled
  by 1 %); the engine only reports when the page actually changed.
* **`AnnotationsReload`** — replace `self.inject_highlights(); self.restore_pending_annotation();` with `if let Some(v) = &self.view { engine::show_all_highlights(v, &self.all_book_annotations); }`.
* **`RecolorAnnotation(id, color)`** — replace the `eval_js` with `self.with_view(|v| v.recolor_highlight(id, engine::engine_color(color_name)))`.
* **`DeleteAnnotation(id)`** — replace the `eval_js` with `self.with_view(|v| v.remove_highlight(id))`.
* **`EngineSelection(sel)`** (new):

  ```rust
            ReaderMsg::EngineSelection(sel) => {
                engine::dismiss(self.selection_chip.take());
                self.last_selection = sel.as_ref().map(|(text, _)| text.clone());
                if let (Some((_, rect)), Some(view)) = (&sel, &self.view) {
                    let chip = engine::build_selection_chip(view.widget().upcast_ref(), rect, &sender);
                    chip.popup();
                    self.selection_chip = Some(chip);
                }
            }
  ```

  The widget paints the selection band and the two drag handles itself
  (R12j); Kalam draws only the chip. `SelectedText` also carries
  `start_rect`/`end_rect` (the first and last line's band, widget
  coordinates — where the handles stand) for a host that wants its chip
  clear of them; `rect` is still the union. This message arrives again
  after a handle drag, with the new text and rects: dismiss and rebuild
  the chip as above.
* **`HighlightSelection(color_name)`** (new) — the old `"highlight"` bridge message:

  ```rust
            ReaderMsg::HighlightSelection(color_name) => {
                engine::dismiss(self.selection_chip.take());
                let Some(view) = self.view.clone() else { return };
                let color = engine::engine_color(&color_name);
                let Some(h) = view.capture_highlight(color) else { return };
                let cfi = engine::range_to_json(&h.start, &h.end);
                let inserted = self.service.catalog().insert_annotation(
                    self.book_id, "highlight", h.start.spine_index as i64,
                    "", 0, "", 0, color.name(), &h.text, "",
                );
                match inserted {
                    Ok(id) => {
                        crate::notify::report(
                            self.service.catalog().update_annotation_cfi(id, &cfi),
                            "Could not place the highlight",
                        );
                        view.show_highlight(id, &h);
                        self.reload_annotations();
                        refresh_highlights = true;
                    }
                    Err(e) => crate::notify::error("Could not save the highlight", &e.to_string()),
                }
            }
  ```

  (`start_path`/`end_path` are empty strings for engine rows — the
  column is `NOT NULL`, and nothing reads them for these rows.)
* **`QuoteSelection`** (new) — same shape with kind `"quote"`, colour `"yellow"`, and no `show_highlight`; `crate::notify::compact("Quote saved", "")`. Call `view.clear_selection()` after reading `view.selected_text()`.
* **`CopySelection`** (new) — `if let Some(text) = view.selected_text() { view.widget().clipboard().set_text(&text); view.clear_selection(); }`.
* **`LookUpSelection`** (new) — `let word = view.selected_text(); view.clear_selection();` then fall into the same code as `EngineWord` with `sentence = None` and the chip's rect as anchor (keep the rect from the last `EngineSelection` in `self.dict_anchor`).
* **`EngineWord { word, sentence, rect, highlight }`** — **do not wire this for Kalam.** Kalam's `main` had removed tap-to-look-up (the JS `fireTapLookup` has no caller), and a connected word handler makes a tap on text a lookup instead of a page turn. Leave `connect_word` unconnected; the chip's Look up is the only entry. The rest of this bullet is kept for a host that wants the feature: the old `"dict-lookup"` bridge message. If `highlight` is `Some(id)`, the tap landed on an existing highlight: open the Highlights sidebar on it (`sender.input(ReaderMsg::ToggleAnnotation(id))`) and return. Otherwise it is the body of the old `"dict-lookup"` arm verbatim (lookup_entry, saved_word_exists, sense hint, log_dict_lookup) ending in `self.show_dict(&data, saved, hint_index, rect, &sender)` (3f) instead of `show_dict_in_webview`. Set `self.dict_context = Some(sentence)`.
* **`DictSearchSelect(word)`** — replace `self.show_dict_in_webview(&word, None, None)` with `self.show_dict(..)` anchored at `self.dict_anchor` (or the widget's centre if `None`).
* **`ClearDict`** — replace the `eval_js` with `engine::dismiss(self.dict_popover.take());`.
* **`SaveCurrentWord`** — unchanged; after a save, rebuild the popover so the button reads "Saved ✓" (call `show_dict` again), or simply dismiss it.

Everything else (`AddBookmark`, sidebars, UI settings, sessions) is
untouched.

`shutdown`: replace the `stop_loading` / handler-disconnect /
`webview_pool::release` block with

```rust
        engine::dismiss(self.selection_chip.take());
        engine::dismiss(self.dict_popover.take());
        if let Some(view) = self.view.take() {
            view.close();
        }
```

`view.close()` matters: GTK holds the widget's draw function and
controllers, and those hold the view. Without the call each closed
book would stay in memory until Kalam quits.

### 3e. `chapter.rs`

Nearly all of it goes. What remains:

```rust
impl ReaderModel {
    pub(crate) fn current_chapter_title(&self) -> &str {
        self.chapter_titles.get(self.chapter).map(String::as_str).unwrap_or("Reading")
    }

    pub(crate) fn go_chapter(&mut self, idx: usize, frac: f64) {
        if idx >= self.chapter_count {
            return;
        }
        self.flush_annotation_note_draft();
        self.editing_annotation = None;
        engine::dismiss(self.selection_chip.take());
        engine::dismiss(self.dict_popover.take());
        self.with_view(|v| { v.goto_chapter(idx, frac); });
        // The position callback sets chapter/fraction, saves progress and
        // reloads the sidebars once the page is on screen.
    }
}

pub(crate) fn chapter_label(model: &ReaderModel, chapter_index: usize) -> String {
    model.chapter_titles.get(chapter_index).cloned().unwrap_or_else(|| format!("Ch {}", chapter_index + 1))
}
```

Delete `css()`, `preload_next_chapter()`, `extract_attr()`,
`load_chapter()`, and the `use crate::epub_book::reading_css`.

### 3f. `js_bridge.rs` (keep the file name — `engine.rs` calls `super::js_bridge::truncate_def`)

Keep: `lookup_dict`, `truncate_def` (+ its tests). Delete:
`inject_highlights`, `restore_pending_annotation`,
`show_dict_in_webview`, `handle_js_payload`, `eval_js`, `url_decode`,
`chrono_now`, and the `webkit6` import. Add the popover front-end:

```rust
impl ReaderModel {
    pub(crate) fn show_dict(
        &mut self,
        data: &crate::db::EntryData,
        saved: bool,
        hint: Option<usize>,
        rect: gtk::gdk::Rectangle,
        sender: &ComponentSender<ReaderModel>,
    ) {
        engine::dismiss(self.dict_popover.take());
        let Some(view) = &self.view else { return };
        let pronunciation = crate::db::pronunciation_for(&data.word).map(|p| format!("/{p}"));
        let card = engine::DictCard::from_entry(data, pronunciation, saved, hint);
        let popover = engine::build_dict_popover(view.widget().upcast_ref(), &rect, &card, sender);
        popover.popup();
        self.dict_anchor = Some(rect);
        self.dict_popover = Some(popover);
    }
}
```

The `"search-in-book"` action from the old popup is not wired in this
round (the engine has the search, the widget does not expose it yet).
The `"save-word"` / `"unsave-word"` paths are `SaveCurrentWord` and the
Words sidebar, as before.

### 3g. `settings_panel.rs`

Add one row to the Reading pane, after the column-width section, so the
strip mode is reachable without a key:

```rust
    let mode_section = reader_settings_section("Layout");
    let mode_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    mode_row.add_css_class("kalam-reader-setting-row");
    let mode_label = gtk::Label::new(Some("Continuous scroll"));
    mode_label.add_css_class("kalam-reader-setting-name");
    mode_label.set_hexpand(true);
    mode_label.set_halign(gtk::Align::Start);
    let mode_switch = gtk::Switch::new();
    mode_switch.set_active(scrolled);           // new fn argument: `scrolled: bool`
    mode_switch.set_valign(gtk::Align::Center);
    let tx = sender.input_sender().clone();
    mode_switch.connect_state_set(move |_, on| {
        let _ = tx.send(ReaderMsg::SetScrolled(on));
        glib::Propagation::Proceed
    });
    mode_row.append(&mode_label);
    mode_row.append(&mode_switch);
    mode_section.append(&mode_row);
    reading_page.append(&mode_section);
    reading_page.append(&reader_panel_divider());
```

### 3h. Elsewhere

* `src/main.rs`: remove `mod webview_pool;`. Keep `mod preload;` but
  delete `warm_chapter_file` / `next_chapter_file` from it (cover
  preloading stays).
* `src/webview_pool.rs`: delete the file.
* `src/app.rs` (or wherever the pool was warmed at start-up): remove
  the call.
* `resources/style.css`: the reader's old `webkit`-specific rules do
  nothing now; add classes for the chip and the popover
  (`kalam-reader-chip`, `kalam-reader-chip-dot-yellow` … `-orange`,
  `kalam-reader-chip-action`, `kalam-reader-dict*`, `kalam-reader-strip-bar`).
  The dictionary card's 37 `kalam-reader-dict*` rules are the old
  `#kalam-dict-popup` design translated to GTK CSS with the app's
  `@kalam_*` colour tokens; the reference copy is calibre-alt's
  `resources/style.css` from the branch that landed the card (Kalam
  1e0dfd3), and `patch/engine.rs` is the matching Rust.
  The five dot colours are the engine's `HighlightColor::css()` values:
  yellow `#f4d35e`, green `#8acb9c`, blue `#8bb7f2`, pink `#e99bbd`,
  orange `#f2ae72`.

## Step 4 — run it

```
cargo build --release -j 2
./target/release/kalam 2> log.txt
```

Open a long novel and check, in this order:

1. First page well under a second after the page opens (the engine's
   part is ~60 ms; the rest is GTK creating the page — watch
   `rg 'book_open|frame took' log.txt`).
2. `→`/`←`/Space/PageDown turn pages; `n`/`p` jump chapters; the pill
   shows "3 / 39" and the chapter title.
3. Close the book, reopen — it lands on the same page.
4. Settings: theme dots recolour instantly; font +/− and column width
   relayout without losing the place; the new Continuous scroll switch
   turns the book into a strip with the scrollbar on the right.
5. Drag-select text → chip; pick a colour → highlight painted, listed
   in the Highlights sidebar; change font size → highlight still on the
   same words; reopen the book → still there.
6. Select a word → chip → Look up → dictionary popover; Save word →
   Words sidebar. (A plain tap on a word turns the page by zone: Kalam
   removed tap-to-look-up before the swap, so do not call
   `connect_word`.)
7. Click a footnote link → follows in place; click a web link → browser.
8. Memory: `ps -o rss= -p $(pidof kalam)` while reading; the target is
   under 100 MB more than the library page alone.

Anything from this list that fails: paste the `rg`-filtered log lines
and the step number.

## What is deliberately not in this round

* Search in book (engine has it; the widget does not expose it yet).
* Old WebKit-era highlights painted in the text (listed, not painted).
* Remote-source "placeholder" chapters (the fetch-on-demand path in the
  old `js_bridge.rs`): the engine reads the EPUB file, so a placeholder
  chapter shows as the placeholder text. If that path matters, the
  fetch has to run at import time and write the real chapter into the
  file.
* Kinetic (flick) scrolling in strip mode; page-raster cache.
