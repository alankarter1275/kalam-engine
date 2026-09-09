# kalam-reader

The reading widget Kalam embeds. One GTK4 widget (`ReaderView`) with the
kalam-engine behind it instead of WebKit. Kalam's paper and ink, Kalam's
Literata, no JavaScript, no network.

## Try it without Kalam

```sh
cargo build --release -j 2 -p kalam-reader-demo
./target/release/kalam-reader-demo book.epub
```

Keys in the demo: arrows / PageUp / PageDown / space turn pages, `n`/`p`
skip chapters, `t` cycles the four themes, `+`/`-` font size, `[`/`]` line
height, `{`/`}` column width, drag to select, `h` highlights the selection,
tap a word to "look it up" (printed to the terminal), `q` quits. Everything
the widget would tell Kalam is printed with a `demo:` prefix.

`--host-fonts` also loads the system's fonts (for CJK, Devanagari, Arabic,
emoji). Off by default because scanning them is the slow part on a cold
disk.

## What Kalam does with it

```text
Kalam                                  ReaderView
─────                                  ──────────
ReaderView::open(path, prefs, opts) ──▶ opens the book, lays out on first draw
overlay.set_child(view.widget())

ReaderMsg::NextChapter          ──▶ view.next_chapter()
ReaderMsg::Theme(t)             ──▶ view.set_theme(t)
ReaderMsg::FontDelta(d)         ──▶ view.set_font_px(prefs.font_px + d)
ReaderMsg::JumpToLocation(c, f) ──▶ view.goto_chapter(c, f)
ReaderMsg::TocSelect(entry)     ──▶ view.goto_toc(&entry)
"highlight yellow" chip button  ──▶ view.capture_highlight(Yellow) → NewHighlight
  INSERT INTO annotations … → id ──▶ view.show_highlight(id, &h)
open / AnnotationsReload        ──▶ view.set_highlights(rows)
Delete/RecolorAnnotation(id, c) ──▶ view.remove_highlight(id) / recolor_highlight(id, c)

save_progress(chapter, fraction) ◀── connect_position(|pos| …)
dictionary popover               ◀── connect_word(|word| …)   (word, sentence, rect, highlight?)
selection chip                   ◀── connect_selection(|sel| …)
open in browser                  ◀── connect_external_link(|href| …)
```

Positions come in two forms. Every page turn reports `chapter` +
`fraction` — exactly what Kalam's `reading_progress` table holds today, and
cheap. `view.locator()` gives the durable form, a `LayeredLocator` that
re-finds the text by its surrounding words after a re-import or a different
edition; ask for it on close and at chapter changes (the first call counts
the whole book's text once). Store both; restore with `goto_locator` when
you have one, `goto_chapter` otherwise.

Highlights live in **Kalam's** `annotations` table and nowhere else. The
widget captures one (`capture_highlight`: text + two `LayeredLocator`s, the
JSON of which fits the unused `cfi` column), Kalam inserts the row, and the
widget paints it under Kalam's row id (`show_highlight`). Recolour, delete
and jump all take that id; at open, `set_highlights` takes every row for the
book. Re-anchoring after a re-import is the engine's, by the quote context
in the locator. A tap on a highlighted word reports the word *and* the
highlight's id, so Kalam can offer "recolour / delete" instead of a lookup.

Text the widget hands over — a selection, a highlight's excerpt, a tapped
word and its sentence — is cleaned first: publishers' files are full of
soft hyphens inside words and zero-width spaces after hard hyphens, which
the engine keeps (its positions are offsets into the raw text) and which
would send `Har\u{ad}ry` to the dictionary. Store what the widget gives
you; nothing to strip on Kalam's side.

Memory: the engine keeps laid-out chapters and decoded images up to
`ReaderOptions::cache_budget`, 32 MB unless Kalam says otherwise (the
engine's own default is 192 MB, sized for comics). `view.cache_bytes()`
says how much it holds right now.

## What it needs from Kalam

* **gtk4-rs 0.11** (Kalam is on 0.9). gtk4-sys can only exist once in a
  binary, so Kalam moves to gtk4 0.11 / libadwaita 0.9 / relm4 0.11 when it
  takes this crate. Mechanical; the API differences are small.
* Nothing else. No WebKit, no font files on disk, no network, and no
  database of its own: the engine writes nothing to disk. Kalam's
  database is the only store, fed from the position callback and the
  highlight calls above.

## Where the numbers come from

Every colour and range in `src/prefs.rs` is copied from Kalam's source:
`ReadingTheme::swatch()` and `selection_style()` for the paper, ink and
selection tints; the `.kalam-hl-*` rules for the five highlight colours; the
`reader.*` preference defaults and clamps for font size (13–24, default 17),
line height (1.3–2.5, default 1.8) and column width (400–860, default 620).
When Kalam changes one, change it here.

## Fonts

`fonts/` holds Literata and Noto Sans, four faces each, SIL OFL 1.1 (licence
texts alongside). They are compiled into the binary with `include_bytes!`.
`serif` is Literata, every other CSS generic is Noto Sans; books are always
laid out in Literata because the widget forces the reader font
(`ReadingSettings::font_family`) and switches publisher stylesheets off —
Kalam's theme wins, as before.

## Not yet

* Continuous scrolling (this is paged; scrolling is the natural next step).
* A converter for Kalam's existing highlight rows (node-path anchors) to
  layered locators — planned to match by `text_excerpt`.
* Search in chapter, bookmarks list — the engine has the calls, the widget
  does not expose them yet.
