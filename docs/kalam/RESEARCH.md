# Research record

Audience: AI agents and engineers continuing this work. Technical by
design. Each entry: **what was investigated, what was found (with
enough specifics to be checkable), and what decision it produced.**
Entries are grouped by topic, dated, and never deleted — a superseded
finding gets a *Superseded* note, so the reasoning trail stays intact.

Append to this file whenever a session researches something, even if
the conclusion is "nothing to do". Include version numbers, file paths
with line numbers at the time, and the measured figures. The owner has
asked for this explicitly (2026-09-10): the research is the reason the
decisions look the way they do, and it must outlive the session that
did it.

Companion documents: [`WORKING.md`](WORKING.md) (how to work here),
[`PLAN.md`](PLAN.md) (the plan, in plain language, with measured
results per round), [`UPSTREAM.md`](UPSTREAM.md) (inherited-file
changes and upstream reviews), [`INTEGRATION.md`](INTEGRATION.md) (the
Kalam wiring recipe).

---

## R1. Choice of foundation: Chapbook vs. writing an engine (2026-09-07)

**Investigated.** The original kalam-engine (a ~100-line scaffold; see
`archive/`) planned an engine from scratch on `lol_html` + `cosmic-text`.
Evaluated [ophymx/chapbook](https://github.com/ophymx/chapbook) as an
alternative: README, crate layout, licence, dependency graph, CI.

**Found.**
- Chapbook is a pure-Rust reading engine: stylo (Servo's CSS engine)
  for cascade, cosmic-text (rustybuzz + swash) for shaping, its own
  paginator, tiny-skia CPU raster, optional vello GPU raster; no
  browser, no JS. MIT OR Apache-2.0, author Jeffrey T. Peckham.
- Workspace of ~20 crates including platform shells (iOS/Android/
  Windows/WASM), OPDS networking, sync, PDF, comics, an SQLite
  bookshelf (`chapbook-library`), and a GTK4 reference viewer
  (`chapbook-viewer-gtk`) — the last being exactly Kalam's platform.
- Positions are "layered locators" (`docs/LOCATORS.md`,
  `LOCATOR_VERSION = 2`): `{spine_href, spine_index, char_offset,
  locator_version, quote{prefix, exact, suffix}, spine_fraction,
  book_progression}` with a resolve chain exact-offset → quote search →
  fraction. This directly satisfies the acceptance criterion
  "positions survive font change / resize / re-import".
- It already handles the hard parts the scaffold would have had to
  reinvent (CSS cascade with publisher styles, bidi, hyphenation,
  footnote links, MathML via a formulary path, fixed-layout detection).

**Decision.** Fork Chapbook. The owner rejected "pin it as a
dependency" twice — the requirement is complete control and the
ability to delete freely. Imported with full history
(`git merge --allow-unrelated-histories`) at `ab14cb7` so upstream
cherry-picks remain one-liners (`UPSTREAM.md`).

## R2. Hardware verification before stripping (2026-09-08)

**Investigated.** Whether Chapbook, unmodified, builds and runs
acceptably on the owner's machine (4 GB RAM, HDD, Arch Linux) before
any effort is spent on it.

**Found.** Built in ~15 min with `-j 2` (agent had predicted 30–60).
Real books open: ~5 s cold, ~1 s warm (later shown to be GTK start-up,
see R7). RSS 151 MB, most of it a 1031-font host font catalog scan.
Visual faults were publisher CSS and a Symbol-encoded font — both
fixed by the viewer's `f`/`s` overrides, which Kalam's theme applies
permanently anyway. Upstream bug found: `publisher_styles` was set but
never read (fixed in `bf2cbda`, recorded in UPSTREAM.md). A stale
html5ever foster-parenting warning was noisy on every book (filtered,
`379d47e`).

Mistakes the agent made here, recorded so they are not repeated: gave
Debian/apt instructions (machine is Arch: `pacman -S rustup gtk4
pkgconf base-devel`); predicted 30–60 min build (15); predicted
50–100 MB RAM (151).

**Decision.** Proceed with the fork; bundle fonts rather than scan the
host (R6).

## R3. What to strip, and the one thing not to (2026-09-08)

**Investigated.** Dependency edges between crates to find a removal
order that keeps the workspace compiling after each step; which
features are implied by which.

**Found.**
- Removal order that compiles at each step: platform shells → GPU
  renderer (vello) → winit viewer → networking/OPDS → sync → PDF →
  comics → Chapbook's own app → `chapbook-library` (last, because
  positions/highlights had to move to the host first — R8).
- **Feature-trim trap:** listing a dependant's features explicitly
  drops the implications the dependency graph used to provide → E0599
  on methods gated by a feature that was implicit before. Re-add the
  implied feature at the dependant.
- `mathml` (formulary path + 820 KB STIX font in
  `chapbook-layout/assets/`) costs nothing at runtime for books without
  math.

**Decision.** Strip in that order (done in two rounds, `cd6bb3a`,
`9108c6d`, `5702fc1`). **MathML kept by owner directive** ("don't drop
it, it might be needed in future"). Result: 7 inherited crates + CLI +
GTK viewer remain (see WORKING.md §4).

## R4. Kalam's source, read once (2026-09-08; re-read 2026-09-10 for step 5)

**Investigated.** The private `calibre-alt` repo, delivered as a 26 257-
line dump (`kalam-src.txt`, committed at `c7c02b2`, removed at
`3f48904`). Read: `Cargo.toml`, `style.css`, `app.rs`, `db.rs`,
`db/prefs.rs`, `epub_book.rs`, `main.rs`, `pages/book.rs`,
`pages/reader/*` (mod, mod_model, types, chapter, chrome, js_bridge,
settings_panel, ui_prefs), `settings.rs`, `preload.rs`, `style.rs`,
`theme.rs`, `webview_pool.rs`. **Not in the dump:** `reader/session.rs`,
`reader/lists.rs`, `reader/panels.rs`, `db/annotations.rs`, `service.rs`,
`models.rs`, `sources/`, `tasks.rs`.

**Found (facts that shaped the widget's API).**
- Deps: relm4 0.9 (`libadwaita`, `gnome_45`), libadwaita 0.7 (`v1_4`),
  gtk4 0.9 (`v4_12`), glib/gio 0.20, webkit6 0.4 (`v2_44`),
  javascriptcore6 0.4, rusqlite 0.32, serde/serde_json, async-channel,
  ureq, wasmtime 24, quick_xml, epub-builder 0.8.3, anyhow.
- webkit6 is used **only** by `src/pages/reader/*` and
  `src/webview_pool.rs` (+ `WebViewHandlers` in `types.rs`), so the swap
  is contained to the reader page.
- Reader architecture: per chapter, `OpenBook::chapter_html(idx, css,
  fraction)` builds an HTML document; `webview.load_html`; a JS bundle
  (~2 000 lines inside `epub_book.rs`) does continuous scrolling by
  appending/prepending chapters, selection chips, dictionary popup,
  highlight injection, and talks back via `kalam://` URIs / document
  title / script messages parsed as `JsPayload {type, fraction, text,
  color, startPath, startOffset, endPath, endOffset, tmpId, word,
  context, rect, definition, count, chapter, next, prev, andScrollTo,
  href, target}`.
- `preload.rs` warms the *next chapter file* into the page cache
  (`warm_chapter_file/next_chapter_file`, :236–260) — a WebKit-era
  optimisation that the engine's own prefetch replaces.
- Prefs: `reader.theme|font_px|line_height|column_px`; the
  `reader.` prefix makes a pref global via `libraries::is_global_pref`
  (:441).
- Themes `ReadingTheme::{Light, Sepia, Dark, Ink}` with `swatch() ->
  (bg, fg)` and `selection_style() -> (selection_bg, handle)`
  (`epub_book.rs` :3824/:3836 in the dump). The hex values were copied
  into `kalam-reader/src/prefs.rs` (its module doc says so; owner: never
  invent colours) — when Kalam changes a colour, change it there too.
- Ranges: font 13–24 (17), line-height 1.3–2.5 (1.8), column 400–860
  (620); clamps live in `ReaderMsg::FontDelta/LineHeightDelta/
  ColumnWidthDelta` handlers (`mod.rs` :904–946).
- DB tables (`db.rs` :566–616, :805–835): `reading_progress`,
  `reading_bookmarks`, `annotations` (with unused `cfi TEXT`),
  `ReadingBookmark` struct :120, `Annotation` struct :77 (`cfi:
  Option<String>` marked `dead_code`). `insert_annotation(book_id,
  kind, chapter, &sp, so, &ep, eo, &color, &text, "") -> Result<i64>`;
  `update_annotation_color(id, name)`; `delete_annotation(id)`;
  `get_reading_progress(book_id) -> Option<(usize, f64)>`;
  `set_reading_progress(book_id, chapter, fraction, chapter_count)`
  (also updates `books.progress` percent).
- Selection chip actions (JS, `epub_book.rs` :1884–1940): five
  highlight colours, quote, dictionary, copy. Dictionary popup payload
  (:2044–2094): `word, pos, senses[{number,pos,def,example}], synonyms,
  antonyms, idioms[{phrase,def}], suggestions, saved, hint,
  pronunciation`.
- Key controller on the reader root (`mod.rs` :716–768): Esc close;
  n/N/→ next chapter; p/P/← prev; +/= and − font; t TOC tab; s
  settings tab; h highlights; b bookmarks; m add bookmark; w words.
- `pages/book.rs` `chapter_titles()` (:17616 in the dump) opens
  `OpenBook` just to list spine titles — out of reader scope; left
  alone.
- Fonts Kalam bundles: Literata and Noto Sans.
- `settings_panel.rs`: `build_reader_settings_panel(sender, theme,
  font_px, line_height, column_px, ui_prefs, dict_sense_hint,
  dict_history_enabled) -> ReaderSettingsControls`, `reader_stepper_row`,
  `reader_settings_section`, `reader_panel_divider` — reusable as is.
- `chrome.rs`: `update_chrome_labels` reads `model.open.chapter_count()`
  and `model.current_chapter_title()`; `overlay_child_box(overlay, i)`.

**Decisions.** The widget's surface mirrors these exactly:
`KalamTheme`/`KalamPrefs` with the same names, ranges and colours;
`ReadingPosition {chapter, chapter_count, page, page_count, fraction}`
so `(chapter, fraction)` drops into the existing tables; `HighlightColor`
with Kalam's five names; `TappedWord {word, sentence, rect, highlight}`
carrying what the dict popup and the sense-hint need; keys `+ = - t m b`
left unbound in the widget because Kalam's controller owns them.
INTEGRATION.md maps every `ReaderMsg` and every `JsPayload.type` to a
widget call or callback.

## R5. GTK-rs version pairing (2026-09-08; re-verified 2026-09-10)

**Investigated.** Which gtk4-rs / relm4 / libadwaita versions can coexist
in one binary with the engine's GTK viewer, and what Kalam must move to.

**Found (docs.rs + crates.io index).**
- Engine `Cargo.lock`: gtk4/gdk4/gsk4 **0.11.4** (feature `v4_16` in the
  widget, `v4_12` is enough for Kalam), glib/gio/pango/graphene-rs
  **0.22.8**, cairo-rs/gdk-pixbuf **0.22.0**, system-deps 7.0.8,
  futures-channel/util 0.3.34. Family is internally consistent.
- relm4 **0.11.0** (2026-04): requires gtk4 ^0.11.2, libadwaita ^0.9.1
  (feature `libadwaita`), glib 0.22. Its `gnome_4x` features chain gtk
  `gnome_4x` + adw `v1_y` (gnome_45 → adw v1_4 — the feature Kalam uses
  still exists).
- gtk4 0.9.7's `AccessibleTextImpl` is gated on `v4_14` — irrelevant
  after the bump but was checked because Kalam is on `v4_12`.
- `gtk4-sys` can exist only once per binary (it exports the C symbols),
  so gtk4 0.9 and 0.11 cannot be linked together. Kalam must move.
- `serde_json` is *not* in the engine's lock file; Kalam has it, so
  locator JSON is built on Kalam's side.

**Decision.** Kalam bumps to relm4 0.11 / libadwaita 0.9 / gtk4 0.11 /
glib+gio 0.22 as INTEGRATION.md step 0, before touching reader code.
The engine's lock stays where it is.

## R6. Fonts (2026-09-08)

**Investigated.** Why the demo used 151 MB and 3–5 s to first page;
what fonts Kalam already ships.

**Found.** Chapbook's `FontSource::host()` scans every system font
(1031 on the owner's machine); on a cold HDD that scan is seconds and
tens of MB of fontdb metadata. Kalam bundles Literata (body, 4 faces)
and Noto Sans (UI, 4 faces).

**Decision.** `kalam-reader/src/fonts.rs` embeds those eight faces
(Regular/Italic/Bold/BoldItalic × 2 families, from `crates/kalam-reader/fonts/`,
via a `face!` macro over `include_bytes!`); `ReaderOptions::host_fonts`
defaults to `false`
("see for yourself" — owner declined to choose). STIX (math) stays in
`chapbook-layout` per R3.

## R7. Where the time and memory go (2026-09-09) — the engine is cleared

**Investigated.** The first adapter run showed 3.9 s to first page and
150 MB RSS, which looked like a failure of PLAN.md §10. Instrumented
every phase with `log::info!` timings before tuning anything.

**Found (owner's machine, 700-page / 39-chapter novel).**
- Engine open: ~190 ms, of which ~200 ms(!) was `chapbook-library`'s
  file hash — gone with R8 → **62 ms** after.
- Chapter layout 34–42 ms; first page paint 58 ms.
- Remaining 3.3 s: GTK/GDK start-up from a cold disk (69 MB RSS before
  the window exists, 143 MB with it); engine cache 2 MB.
- Two real bugs found by the run: selected text carried publisher soft
  hyphens (`Har\u{ad}ry`) — now stripped by `readable()` in `view.rs`;
  the engine's comic-sized 192 MB cache default was inherited — now
  `DEFAULT_CACHE_BUDGET = 32 MB`.

**Decision.** *The engine is not to be tuned further without a new
measurement that says the engine is the cost.* Kalam has already paid
GTK's start-up by the time the reader opens, so the demo's wall-clock
numbers are not what Kalam will feel.

## R8. Removing `chapbook-library`; host-owned positions and highlights (2026-09-09)

**Investigated.** What `chapbook-library` provided that the widget still
needed, and how to provide it as values instead of a database.

**Found.** It stored positions, marks (highlights) and a bookshelf in
SQLite, and hashed every file on open (R7). The resolve chain for
positions was `chapbook_core::locator`'s, reusable without the store.
Highlights were resolved the same way (by quote context) at open.

**Decision.** Two new inherited-crate files (both listed in
UPSTREAM.md): `chapbook-reader/src/host_position.rs`
(`layered_locator()`, `layered_locator_at(offset)`,
`goto_layered(&loc, same_edition)`, `unit_fraction()`,
`chapter_char_count(spine)`, `chapter_char_counts()`, `word_at_exact`)
and `host_highlights.rs` (`HostHighlight` + `set/show/recolor/hide_
host_highlight`, `host_highlight_at`, `goto_host_highlight`) — the
engine paints under the host's i64 id and never stores. Then the crate,
its feature, tests and CLI `lib` commands were deleted (`5702fc1`).

## R9. Continuous scrolling design (2026-09-09)

**Investigated.** Owner wanted the whole book as one strip (per-chapter
scrolling was proposed and rejected). Options: (a) one tall layout of
the whole book; (b) compose the existing per-page rasters into a
strip; (c) WebKit-style DOM append.

**Found.**
- (a) needs every chapter laid out before the first frame (seconds on
  the HDD machine) and breaks the page-based locator/highlight code.
- (b) reuses the paginator, caches, locators and highlights unchanged;
  needs from the engine only a *by-page* surface: trimmed page extents
  (`used_height` + the `gap` a break discarded), frame/hit-test for an
  arbitrary `(spine, page)`, `set_position`, `settle` (land a pending
  jump without a frame), `layout_generation` (hear that a font change
  dropped layouts), `pin_units` (visible chapters must not be evicted
  mid-frame). Cost: ~22 `pub fn`s in a new `chapbook-reader/src/scroll.rs`
  and small edits to four inherited files (UPSTREAM.md rows).
- Unmeasured chapters need a height estimate: char count × px/char,
  where px/char is learned from measured neighbours
  (`INITIAL_PX_PER_CHAR = 0.4`, `MIN_ESTIMATE_VIEWPORTS = 0.5`). This is
  why `chapter_char_counts()` exists on the session.
- Keeping the *reading line* (`MARGIN_TOP` = 48 px below the viewport
  top) on the same text across every correction/relayout is what makes
  estimate corrections invisible.

**Decision.** (b). Round G (engine surface, `9f0a1e9`, `b11c671`,
`174bc9d`), round H (widget `ReadingMode::Scrolled`, `Strip` model in
`kalam-reader/src/scroll.rs`, `1d0c9b2`). Owner-verified 2026-09-10:
"everything looks fine". Acceptance: continuous flow, scrollbar
reflects estimated length, no jumps when estimates correct,
positions/highlights/tap/selection work on any band, mode switch lands
on the same page, memory unchanged.

## R10. Round I polish — what the round-H log said (2026-09-10)

**Investigated.** Owner's filtered log from round H on the 39-chapter
novel.

**Found.** 8 ms per frame (so a raster/tile cache would buy nothing);
20–80 ms whenever a chapter was *first* laid out (felt at chapter
seams); 70–85 ms on every `s` mode switch — traced to the sibling
scrollbar changing the widget's width → full relayout; the whole-book
char count (111 ms on 1.2 M chars) landed on the first scrolled frame.

**Decision.** No tile cache. Next-chapter prefetch in a
`glib::Priority::LOW` idle (one chapter per idle, scrolled mode only);
scrollbar moved into a `gtk::Overlay` over the widget's right edge;
char count moved to an idle after the first frame (`c170e3d`); chapter
divider (hairline + uppercase title pill, `divider.rs`, painted with
tiny-skia — no new dependency) because the owner asked for one.
Measured after: open 9 ms, first page 143 ms, switch 27 ms, prefetch
2–48 ms in idles, RSS 155 MB (GTK), engine cache 2 MB.

## R11. Step-5 integration research (2026-09-10)

**Investigated.** Everything needed to write INTEGRATION.md and the
drop-in `engine.rs` without Kalam's build available: the exact widget
API, how Kalam's reader page is structured (R4), and several engine
details.

**Found.**
- `Session::open_with(impl Into<Source>, config)` (`chapbook-reader/src/open.rs`
  :254) takes `&Path`/`PathBuf`/`&str`/bytes/reader; `format_of_path`
  sniffs bytes, then extension, then defaults to EPUB → rbook's
  unzipped-directory EPUB (Kalam's cache dir layout) still opens. The
  glue passes `&book.file_path` directly.
- `LayeredLocator` derives only `Debug, Clone, PartialEq` — **no serde**.
  `Quote` derives `Default`. Hence `locator_to_json`/`locator_from_json`
  in `patch/src/pages/reader/engine.rs` build JSON by field.
- `SearchHit {locator, end, context, match_range}` and
  `Session::search(query, limit)` exist but block; not exposed on
  `ReaderView` yet (deferred).
- `Session::jump/land` keep a back-stack (cap 64) → `Action::Back`
  works; chapbook-epub keeps every spine entry (linear flag preserved),
  so engine spine indices equal Kalam's `chapter_index`.
- `KeyMap::default()` binds `n/p` (units), `b`/Backspace (Back), `+ = -`
  (font), `t` (theme), `m` (menu). The widget unbinds `+ = - t m` and,
  since `af3d843`, `b` (Kalam's bookmarks panel key).
- GTK↔view ownership cycle: `set_draw_func` closure, three controllers
  and the adjustment handler each hold a `ReaderView` clone, and the
  view holds the `DrawingArea` they hang on. Nothing breaks the cycle
  when a relm4 component is dropped → `ReaderView::close()` added
  (`af3d843`): resets the draw func, removes controllers, disconnects
  the adjustment handler, drops callbacks, replaces the loader waker,
  releases caches. The demo calls it on close-request.
- `gtk::Popover` with `set_parent` must be `unparent`ed before drop
  (GTK warning otherwise) → `engine::dismiss()` helper.
- Kalam's `EntryData.pos`/`Sense.example` optionality was not visible in
  the dump → `DictCard::from_entry` uses `Option::<String>::from(x.clone())`
  so it compiles whether those fields are `String` or `Option<String>`.
- The `annotations.cfi` column is `TEXT` nullable and unused → chosen to
  hold `{"kalam_locator":1,"start":{…},"end":{…}}` for engine-made
  highlights; WebKit-era rows (DOM paths in `start_path/end_path`) fail
  `range_from_json` cleanly and are listed but not painted.

**Decision.** INTEGRATION.md (step 0 dependency bump first; DB one
function; drop in `engine.rs`; reader page file by file; 8-point run
checklist) and the complete `patch/src/pages/reader/engine.rs`.

## R12. Upstream review 2026-09-10 (`ab14cb7..7ace24a`)

**Investigated.** `gh api repos/ophymx/chapbook/compare/ab14cb7...main`
and each PR's file list, filtered to crates we kept.

**Found.** PRs #32–#36, 42 files: FFI/JNI navigation + OPDS catalog
bindings, Swift reader surface, .NET parity, vello parity-test gating.
In kept crates only `chapbook-reader/src/{lib.rs, open.rs}`: new
`pub fn open_publication(path) -> Box<dyn Publication>` (format-sniffed
open without a session, for an importer) and `pub use chapbook_opds`
behind `opds`. No bug fixes; nothing in layout, EPUB, locators,
highlights, rendering. No open issues upstream.

**Decision.** Take nothing. Logged in UPSTREAM.md's review log. Candidate
for later: `open_publication` if Kalam's importer wants the engine to
read metadata/covers.

## R12a. First report from the calibre-alt agent (2026-09-10)

**Investigated.** The Kalam-side agent's static review of the recipe
against Kalam's real tree (it has the four files the snapshot lacked,
but no compiler either — see below).

**Found.**
- **`gtk4-sys` 0.9 and 0.11 both declare `links = "gtk-4"`** (verified
  on docs.rs: `gtk4-sys` 0.11.4 `Cargo.toml`), and webkit6 0.4 depends on
  gtk4 0.9. Cargo rejects a graph containing both before compiling
  anything ("multiple packages link to native library `gtk-4`"). So
  INTEGRATION.md's "step 0 builds with webkit still in" was impossible;
  steps 0–3 are necessarily one build. Recipe corrected.
- `EntryData.pos` is `Vec<String>` (`src/db/dictionaries.rs:96`), not
  `String`/`Option<String>`; the WebKit popup joined it with ` · `
  (`epub_book.rs:2107`). `patch/engine.rs` `DictCard::from_entry`
  corrected to `data.pos.join(" \u{00b7} ")`. `Sense.example` shape still
  unconfirmed; the `Option::<String>::from` form covers `String` and
  `Option<String>`.
- gtk4 0.11 narrows `PopoverExt::set_popover` to `impl IsA<Popover>` and
  drops six unused `connect_*_notify` names; relm4 0.11 changes no name
  Kalam uses; `Component::Input` is `Debug + 'static` (no `Send`), which
  the `gdk::Rectangle`-carrying `ReaderMsg` variants rely on.
- Cargo unifies `kalam-reader`'s `v4_16` with Kalam's `v4_12` → the
  binary needs GTK ≥ 4.16 at runtime. Fine on Arch.
- **Neither agent can compile Kalam**: the calibre-alt sandbox has the
  same no-toolchain/no-crates.io restriction as this one, and
  calibre-alt's GitHub Actions is refusing to start jobs ("recent
  account payments have failed or your spending limit needs to be
  increased") since 2026-09-10T20:20Z. kalam-engine's Actions still ran
  at 18:48Z the same day; whether it is affected is unknown until the
  next push here. The private-repo Actions minutes quota is the likely
  cause (public repos are free).
- Kalam's ReaderModel has 73 fields; the agent verified the literal in
  the rewritten `init` sets all of them, and that every
  `engine::`/`ReaderView` name used resolves against `e6a3c25`.

**Decision.** Recipe and patch corrected in this commit (bundle must be
regenerated). Verification path: the owner's machine is now the only
compiler for Kalam until Actions billing is fixed — owner runs
`cargo build --release -j 2` in calibre-alt and pastes the first error
block into the Kalam agent's chat; that agent fixes and reports here
only when the engine is implicated.

## R13. Tooling facts verified along the way

- **docs.rs cosmic-text 0.19.0:** `Buffer::new(&mut FontSystem, Metrics)`,
  `set_wrap`, `set_size`, `set_text(fs, text, &Attrs, Shaping,
  Option<Align>)`, `shape_until_scroll(fs, prune)`,
  `lines[i].layout_opt()`; `LayoutGlyph {x, y, w, font_id, glyph_id,
  x_offset, y_offset, font_size, …}`; `LayoutLine {w, max_ascent,
  max_descent, glyphs, …}` — used by `divider.rs` to shape the title pill.
- **docs.rs glib 0.22.8:** `glib::idle_add_local_full(priority, FnMut()
  -> ControlFlow + 'static) -> SourceId` (main-thread only);
  `glib::spawn_future_local`.
- **docs.rs tiny-skia 0.12.0:** `Pixmap::{fill_rect, fill_path,
  stroke_path}`, `Paint::set_color_rgba8`, `PathBuilder::{move_to,
  line_to, cubic_to, close, finish}`.
- **docs.rs rbook 0.7.10:** `Epub::open(path)` accepts an unzipped
  directory or a `.epub`; `Epub::read(Read + Seek)`; default features
  `write`, `prelude`, `threadsafe`.
- **GitHub Actions:** raw job logs are served from Azure blob storage,
  unreachable from the sandbox — hence the "CI details" check run.
- **crates.io index via `fetch_page https://index.crates.io/<a>/<b>/<crate>`:**
  paginated oldest-first in ~11 chunks; useless for "latest". Use the
  `gh api repos/rust-lang/crates.io-index/contents/…` route instead
  (needs a valid token).
- **`gh api` for PR files:** `repos/<o>/<r>/pulls/<n>/files?per_page=100`
  returns per-file `patch` text — enough to review small upstream
  changes without cloning.

## R14. Open questions (not researched yet)

- Does Kalam's `lists.rs::rebuild_toc` walk `OpenBook.spine` titles or a
  nested TOC? (Affects whether INTEGRATION.md 3b passes `view.toc()` or
  `chapter_titles`.) Resolve on first build error.
- Whether Kalam's `service.reader(book_id)` snapshot already includes
  every annotation of the book (`snap.annotations`) — assumed yes from
  `mod.rs` :398.
- Fixed-layout EPUBs (`Book::is_fixed_layout`, `chapbook-epub/src/book.rs`
  :171): the widget has not been exercised on one; Kalam's WebKit path
  rendered them as ordinary pages.
- Behaviour on a 4 GB machine when two readers are open (Kalam allows
  only one reader page at a time — believed, not verified).
