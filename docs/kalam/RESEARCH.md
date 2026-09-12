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
  `LOCATOR_VERSION = 3` since R15; 2 at import): `{spine_href, spine_index, char_offset,
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

## R12b. Second report: the static pass (2026-09-10)

**Found.** After the swap, 28 `webkit` string hits remain in Kalam's
`src/epub_book.rs`, all inside three items with no callers:
`OpenBook::chapter_html` (:120–149), `inject_reading_shell` (:503–2884,
the ~2 400-line JS shell), `reading_css` (:2938–3795). Shared helpers
(`path_to_file_url`, `read_file_string`, `parent_zip_path`,
`join_zip_path`) sit between them. `Sense.example` is `Option<String>`
(`src/db/dictionaries.rs:86`). Kalam's branch head: `14c2cd9`.
`OpenBook` itself is still used by `pages/book.rs` for chapter titles
(R4), so the struct stays.

**Decision.** Dead code is deleted *with* a compiler, not before one:
ruled "accept as clean for the first build"; the three items (and any
helper that orphans) are removed in the build-fix commit, where
`dead_code` warnings from `-D warnings` will name exactly what is
unreferenced. The Kalam agent's three wording edits to `patch/engine.rs`
folded in here (this commit) so a regenerated bundle does not
revert them. Owner committed the ef11e46 bundle on calibre-alt `main`
instead of the agent's branch — relay instruction: check out the
agent's branch before copying and before every build.

## R12c. Third report: Kalam compiles without WebKit (2026-09-11)

**Found.**
- Making both repos public unblocked calibre-alt's Actions at once
  (private-repo minutes were the cause). Its first run then died in
  `gdk4-sys`'s build script: `ubuntu-latest` = 24.04 = GTK 4.14.5, and
  `kalam-reader`'s `v4_16` unifies with Kalam's `v4_12` → `gtk4 >= 4.16`
  required. Fixed there by `ubuntu-26.04`, same as this repo's ci.yml.
- With a compiler, clippy `-D warnings` named exactly 8 errors: 7
  `dead_code` in the old reader path (3 321 lines deleted from
  `epub_book.rs`, incl. the ~2 380-line JS shell; `read_file_string`,
  `parent_zip_path`, `join_zip_path`, `ReadingTheme`, the free
  `spine_index_for` are live and stayed) and 1 in the patch:
  `useless_conversion` on `Option::<String>::from(s.example.clone())`
  because `Sense.example` *is* `Option<String>`. Lesson: a
  "compiles-either-way" conversion is itself a lint under `-D warnings`
  once the type is known — write the type, do not hedge it.
- Owner's local `cargo build --release -j 2` was SIGTERM-killed
  compiling `stylo`: Kalam's `[profile.release]` is `lto = true`,
  `codegen-units = 1`, and 4 GB is not enough for that with stylo in
  the graph. This workspace uses `lto = "thin"` and built the demo on
  the same machine. Recipe now says: thin LTO, default codegen-units,
  `-j 1` if needed.
- Gate after Kalam's 73ed7cf (run 34531632536): clippy clean; 341 tests,
  339 passed, 2 ignored; debug and release builds succeed; calibre-alt
  has a headless *screenshots* job (139 books, `window_shown` 243 ms,
  library pages only — the reader still needs a human).
- `delete_saved_word_by_word` (dictionaries.rs) lost its only caller
  (the old popup's unsave toggle, js_bridge.rs:551). The new card has
  "Save word" → "Saved ✓" (disabled); unsave lives on the Words page.

**Decision.** Patch corrected (this commit). `delete_saved_word_by_word`:
delete; "unsave from the card" goes on the deferred list (WORKING.md §8)
and comes back only if the owner misses it. Next: the owner runs the
step-4 checklist on Kalam's branch.

## R12d. Fourth report: the reader runs headless in Kalam's CI (2026-09-11)

**Found.**
- Kalam 990fab7: `[profile.release]` is now `lto = "thin"` with no
  `codegen-units`; CI's release build is green with it.
- Kalam's CI has a headless screenshot job (sway + Cairo renderer, 139
  seeded books). `KALAM_ROUTE` now accepts `book-<id>` / `read-<id>`;
  the seeder writes a valid minimal EPUB (stored `mimetype` first,
  container.xml, OPF, EPUB 3 nav + NCX, six ~1 900-word chapters).
  Before that, the reader mounted on the open-error label — so that
  path (step 3's) renders without a panic. `KALAM_TIMING=1` prints
  `[timing]` lines.
- Run 34538345557, route `read-1`, Kalam 2b3050b: no panic, no widget
  warning; `[timing] window_shown 161.7 ms, startup_first_page 2.8 ms,
  book_open 4.2 ms`; peak RSS 121 MB with a book open vs 83 MB
  library-only → reading ≈ 38 MB under Cairo. Screenshot 203 KB, three
  samples byte-identical (the page settles; no repaint loop).
- Caveat: Cairo headless proves "lays out and paints without
  panicking", not "looks right". On the owner's desktop (GL renderer,
  real fonts, a real novel) expect higher absolute RSS — the demo alone
  measured 139–155 MB there (R7/R10); the figure that matters is
  reading minus library.
- The memory one-liner the Kalam agent gave the owner
  (`while sleep 1; do … done | sort -n | tail -1`) cannot print: Ctrl-C
  kills `sort` with the loop. Replacement: print every sample, read the
  peak by eye.

**Decision.** No engine change; bundle stays at 1058af3. Owner runs
step 4 on Kalam's branch (`-j 1`). Visual faults (colour, spacing,
blank areas, stuck frames) are engine-side reports even when no message
names `kalam_reader`.

## R12e. First step-4 numbers, and the TOC question (2026-09-11)

**Owner's run** (Kalam branch `arena/01a08cfb-calibre-alt`, `KALAM_TIMING=1`,
Arch, 4 GB, HDD, real library of ~150 books):
- `startup_db_open` 2 111.8 ms, `startup_first_page` 402.7 ms,
  `window_shown` 5 664 ms — Kalam's own start-up (SQLite on the HDD +
  GTK), unchanged by the swap.
- `book_open` **63.5 / 101.5 / 41.4 / 106.7 ms** on first opens of four
  books, **2.7–6.1 ms** on re-opens. The PLAN §10 "first page well under
  a second" criterion is met inside Kalam, not just in the demo.
- RSS: library page 216–218 MB; reading 309–345 MB (peak 345) →
  ≈100–127 MB over the library page against the <100 MB target. The
  engine's own cache is ~2 MB (R10); the rest is GTK texture memory
  for the page, fonts, and Kalam's sidebar data. Borderline; not a
  blocker. `view.close()` is wired in Kalam's `shutdown`
  (`src/pages/reader/mod.rs:1478`), so it does not grow per book.
- `Gtk-CRITICAL: Unable to connect to the accessibility bus` — no
  AT-SPI bus in the owner's session; Kalam's CI prints the same line.
  Environmental; `GTK_A11Y=none` silences it.

**TOC.** `chapbook toc` on the owner's novel (a Calibre Kindle→EPUB
conversion; files `CR%21…_split_NNN.html`, `#fileposNNNNN` anchors)
prints 29 entries one-per-file at split_008..036 whose labels are
"Acknowledgements", "The Shiva Trilogy", "Chapter", "2 :" … "26 :",
"Glossary", followed by 21 real chapter names that ALL point into
split_007 at anchors ~250 bytes apart — i.e. at the lines of the
book's own printed contents page, not at the chapters. Nesting is
unknown (the chat strips the two-space indentation). The owner reports
(d): wrong entries + wrong targets + garbled names, which is exactly
what a sidebar built from that list looks like.

Parser comparison, by reading:
- Old Kalam `parse_nav_or_ncx` (epub_book.rs, deleted at 73ed7cf): nav
  first if it yields anything (scans every `href=…</a>` after the first
  `<nav`, label = `strip_tags` of the inner HTML), else NCX (roxmltree,
  every `navPoint` flattened DFS, label = first `<text>` descendant);
  spine match lenient (`ends_with` / file-name equality).
- Engine (rbook 0.7.10): `toc().contents()` = the `epub:type="toc"` nav
  if present, else NCX `navMap`; labels = all text inside `<a>` /
  `<navLabel>` (`XmlReader::get_element_text` collects nested text);
  a tree; `convert_toc` matches `Href::path()` against `SpineItem.href`
  exactly — safe, because both come through rbook's `require_href` →
  `uri::encode` (`!` → `%21` on both sides).
- No content difference found. If the file's NCX says `<text>2 :</text>`,
  both readers showed "2 :". Old Kalam's spine-title rule was
  *last* TOC label wins; the engine's `chapter_titles` is *first* wins
  (matters only for split_007 here).
- Kalam-side inconsistency in the new `rebuild_toc`
  (`src/pages/reader/lists.rs:199`): rows are built from top-level
  entries only, while `toc_spine_indices` (active row, scroll centring)
  recurses. The old sidebar walked a flat list, so a nested TOC
  (parts → chapters) now loses its chapter rows. Harmless for flat
  TOCs; needs a recursive walk with depth → indent. Kalam jumps by
  `TocSelect(spine_index)` → `goto_chapter(idx, 0.0)`; fragments are
  ignored as before. `ReaderView::goto_toc(&entry)` honours fragments
  and is unused by Kalam.

**Open.** The NCX as written (owner to dump), whether it nests, and
how the old reader showed this book. **Decision.** No engine change
until the dump is read. Dictionary-card styling is the round after.

## R12f. TOC verdict, and the dictionary card's design debt (2026-09-11)

**TOC — closed, parity.** The owner dumped `toc.ncx`: calibre 0.8.29,
`dtb:depth` 2, flat `navMap`, labels literally `<text>Chapter</text>`,
`<text>2 :</text>` … `<text>26 :</text>` (the chapter *names* were in a
second element calibre's converter dropped) plus the printed contents
page as 21 `#filepos` entries into `split_007`. The old WebKit reader
showed the same list ("it also shows the same thing"). Nothing for
either side to fix; a "repair a calibre-mangled NCX" heuristic is out
of scope. The Kalam-side nesting bug in `rebuild_toc` (R12e) stands as
a separate, real defect for nested TOCs.

**Dictionary card — why it "looks completely off".** The recipe's
`build_dict_popover` was written as a *functional* stand-in: plain
`gtk::Box`es, six 0.9 rem CSS rules, "Save word"/"Close" text buttons.
The old popup (`inject_reading_shell`, dump :13307–13552 JS and
:14648–14925 CSS) was a designed component. Its spec, for the rebuild:
- Frame: 320 px wide (max 80 % of the window), radius 18, 1 px
  `@kalam_border`, background `@kalam_surface`, large drop shadow.
  Anchored below the word when there is room, above otherwise, never
  covering it.
- Header (padding 14/16/10/16, 1 px bottom border, slight shadow):
  left — word in **serif 22 px 600** (Georgia/DejaVu Serif), then
  pronunciation in **monospace 10 px** dim (hidden when absent; value
  is `/{ipa}` with no trailing slash — an old quirk, keep it), then a
  POS pill (10 px italic, accent text on 14 % accent, radius 999) only
  when the entry has fewer than two POS groups; right — three **26 px
  round icon buttons** on `@kalam_surface_2` with a border: ☆/✓ save
  (saved = accent border + 14 % accent fill), a magnifier "Find in
  chapter", ⧉ copy (word + numbered definitions to clipboard, glyph
  flips to ✓ for 900 ms).
- Body: max-height 300, scrolls with hidden scrollbar, 40 px bottom
  fade; padding 4/16/40/16. Section labels **9 px 700 uppercase,
  letter-spacing 0.08 em, dim** ("Definitions", "Synonyms", "Antonyms",
  "Idioms", "Did you mean" / "Words in this phrase").
- Senses: number in monospace 10 px accent, 16 px min width; text
  13 px / 1.55; example serif italic 12 px dim; hinted sense carries a
  "LIKELY HERE" pill (9 px 700 uppercase accent). First 3 shown, rest
  behind "Show N more" (11.5 px accent, toggles to "Show less").
  Senses grouped by POS with an uppercase divider row (noun, verb,
  adjective, adverb first, then first-seen order; unlabelled last, no
  divider) — the flat sense index is preserved for the hint.
- Chips (synonyms / antonyms / suggestions): radius 999, padding 3/11,
  11.5 px, 1 px border; synonyms accent-tinted, antonyms `#e06c75`
  tinted; click = lookup that word.
- Idioms: cards on `@kalam_surface_2`, radius 9, padding 8/11; phrase
  serif italic 12.5 px, definition 12 px / 1.5.
- Empty: "No entry for 'word'." 13 px dim, padding 18/4.
- Colours are the app tokens (`@kalam_surface`, `_surface_2`,
  `_border`, `_text`, `_text_dim`, `_accent`), so the card follows the
  app theme, not the reading theme.
- Behaviours the GTK card dropped: the header cross-fade on chip
  re-lookups, ↑/↓ sense focus + Enter-to-save, "Find in chapter"
  (needs a search the widget does not expose — `Session::search`
  exists but `ReaderView` has no `search()`; deferred), unsave toggle
  (R12c, deferred).

**Decision.** The card is Kalam UI, drawn with Kalam's CSS tokens —
its rebuild is the Kalam agent's work, spec above, with the engine's
`patch/engine.rs` updated to match once it lands (the patch is the
recipe's copy of Kalam's file, not the other way round). Engine
follow-up if the owner wants "Find in chapter": expose search on
`ReaderView` (WORKING.md §8).

## R12g. The card rebuilt; TOC nesting fixed (2026-09-11)

**Found.** Kalam 1e0dfd3 (+ three CI-driven build fixes, tip 4a3e6d6):
- `build_dict_popover` rebuilt to the R12f spec: `DictSense { pos, def,
  example, hinted }` per sense (so `DictCard.senses: Vec<DictSense>`,
  not the 3-tuple); POS grouping over the flat list with
  `POS_GROUP_ORDER`; header = word / pronunciation / conditional POS
  pill + three 26 px round buttons (☆→✓ save, disabled magnifier, ⧉
  copy via `Widget::clipboard()` with a 900 ms tick); body in a
  `ScrolledWindow` (`max_content_height(300)`,
  `propagate_natural_height`) under a 40 px gradient `Overlay`; extra
  senses behind a `Revealer` with `RevealerTransitionType::None` and a
  "Show N more"/"Show less" button; chips in a `FlowBox`. GTK CSS has
  no `max-width`, so 320 px is a `set_size_request` minimum. CSS: 37
  `kalam-reader-dict*` rules on the `@kalam_*` tokens; the popover's
  own frame is made transparent and the `> contents` node carries the
  radius-18 border, surface and shadow.
- `rebuild_toc` now walks the tree depth-first (`toc_rows`), indents 16
  px per level, draws target-less part headings as non-clickable rows,
  and shares that walk with the active-row/scroll logic; three unit
  tests; CI's seeded book 1 now has a two-part nested nav + NCX.
- The three compile errors were all in the new code (`String` vs
  `&&str` in a filter closure, a field name, `iter().any()` →
  `contains`); CI found each within one round. The relay's "one error
  per round, compiler on CI" loop works without the owner.
- Still stale in Kalam: `src/pages/settings.rs:1381` describes the
  cache as "Temporary files extracted for the WebKitGTK reader".

**Decision.** Kalam's `engine.rs` mirrored verbatim into
`patch/engine.rs` (the recipe's copy follows Kalam's file). No widget
change. Owner renders the card for the first time on the next run.

## R12h. The card is bad on screen; the mockup exists; tap-lookup was never on (2026-09-11)

**Found.**
- Owner's verdict on the first render of the GTK card: "extremely bad".
  Root cause is structural, not a slip: the old popup was a **web
  page** (HTML built in JS, browser CSS with `color-mix`, `flex`,
  `position: fixed`, box shadows) rendered by WebKit. GTK CSS is a
  dialect with none of those; the card had to be re-drawn as widgets,
  blind, by an agent with no display, from my R12f prose. The first
  render is the first feedback. Every reader-overlay element (chip,
  divider, dictionary, search hits) was HTML inside the WebView and
  falls in this class; the page text itself did not (the engine
  replaced WebKit's layout, and that part the owner has seen work).
  My earlier framing — "Kalam's reader logic is not WebKit-dependent" —
  was true of the logic and silent about the overlay UI; the owner
  planned on copy-paste and got a redesign.
- calibre-alt keeps the design source: `docs/files/
  kalam_dictionary_popup_v3.html` (the standalone mockup: 380×520,
  Fraunces 26 px 600 headword, IBM Plex Mono 11 px pronunciation, POS
  pill in the `--info` violet, 30 px round save button, body padding
  4/20/24, section labels 9 px with 10/20 margins, senses 13 px/1.6,
  chips 12 px 4/12, idiom cards radius 10 padding 10/12; palette
  `--bg #1b1e24 --surface #21242b --surface2 #282c34 --border #343842
  --text #abb2bf --dim #6b7280 --accent #61afef --danger #e06c75
  --info #c678dd`), `kalam_dictionary_popup_v3_preview.html` (the
  *shipped* popup's CSS+JS extracted verbatim from `epub_book.rs` by
  `gen_kalam_dict_preview.py`, with the app tokens substituted — the
  exact thing the owner used to see), and `test_kalam_dict_preview.js`.
  Also `reader.html`, `float_panel.html`, `kalam_annotations_panel.html`,
  `settings.html`, `book_detail.html`, `kalam_my_library_v5.html`,
  `style.rs`: mockups for the rest of the app. Kalam's `style.css`
  already references "Fraunces" (falls back to system serif; not
  bundled). The R12f spec was derived from the shipped CSS and is
  consistent with the preview file, but a picture beats prose: the
  agent can open neither, the owner can open both.
- **Tap-to-look-up**: on calibre-alt `main` the JS `fireTapLookup`
  exists but nothing arms it (no `setTimeout(.., tapDelay)` caller) —
  the owner removed the feature deliberately. INTEGRATION.md 3e wired
  the widget's `connect_word` to the dictionary as "Kalam's
  tap-to-look-up", reviving it. My error: written from the old code's
  functions, not from what `main` did. Widget side: `drag_end`
  (`view.rs` ~:1440) treats a tap on a word as a word *before* the
  page-turn zones, so with the callback unwired a tap on a word turns
  no page. Fix on the engine side: only claim the tap as a word when a
  word callback is connected; otherwise fall through to the zones.

**Decision.** (1) Kalam agent rebuilds the card against the mockup
files, with the owner's screenshots as the loop; the R12f spec is
demoted to a cross-check. (2) Kalam drops the `EngineWord` → dictionary
path; the chip's "Look up" is the only entry. (3) Engine: `drag_end`
falls through to the tap zones when no word handler is connected
(next widget change; bundle regeneration then). (4) WORKING.md gains
the lesson: overlay UI is a redesign, plan for screenshot rounds.

## R12i. Widget: a tap on text is a word only when someone is listening (2026-09-11)

**Change.** `ReaderView`'s `drag_end` (`view.rs` ~:1447) now asks
whether a word callback is connected before treating a tap on text as
a word; with none, the tap falls through to the page-turn zones as if
it had landed on margin. Behaviour for the demo (which connects one)
is unchanged; for Kalam, which will not connect one, a tap on a word
turns the page, matching `main`. INTEGRATION.md 3e and step-4 item 6
corrected. No API change; `connect_word` keeps its signature.

## R12j. Selection band, draggable handles, tap-clear notify (2026-09-11)

**Source.** The calibre-alt agent's `docs/kalam-reports/selection-experience.md`
(its branch, 2f9ca09) compared the old reader's selection UI with the
engine's and split the work: the chip is Kalam's (rebuilt at 46c4169),
the band and the handles are painted on the page and so are ours.

**What the old reader did** (calibre-alt `main` 4628dbf,
`src/epub_book.rs`: `positionSelectionBands` ~:1195, `.kalam-selection-band`
:3117, `.kalam-selection-handle` :3138, `ReadingTheme::selection_style`
:3836). Band per line: height = the paragraph's first-glyph height + 2 px
above and below (`selectionBandPadding = 2`), centred on the line rect,
`border-radius: 2px`, colour = theme `::selection` rgba (already in
`prefs.rs` `selection()`), `mix-blend-mode: multiply` on Light/Sepia and
`screen` on Dark/Ink, drawn in a layer *under* the text. Handles: a 2 px
bar in `handle_color` (`#0b0b0b` Light/Sepia, `#ffd166` Dark/Ink) as tall
as the band, at the start's left edge and the end's right edge; a 24 px
wide hit area reaching 12 px past either end; a 5 px teardrop grip
(`border-radius: 50% 50% 50% 0`, rotated) at the outer end — above the
start bar, below the end bar; `cursor: grab`; dragging moved *that* end
only, bands and chip following live.

**What the engine did.** `push_selection_rect` filled `fragment.rect`
(the whole line box) per span, square, source-over; at Kalam's
line-height 1.8 that fused a multi-line selection into one slab. No
handles; a press always `selection_begin`s (replaces the selection), so
no way to adjust one end. `view.rs`'s tap path cleared the selection
without `notify_selection` (harmless — the host had already heard `None`
at the press when a selection stood; documented in the code now).

**Engine facts found on the way.**
- `LineFragment` had `baseline` but no glyph-box metrics. cosmic-text's
  `LayoutLine` has `max_ascent`/`max_descent`; `LayoutRun` (what
  `layout_runs()` yields) carries `line_y`, `line_top`, `line_height`
  but not those two, and does not say which layout line of its buffer
  line it is. Its `glyphs: &[LayoutGlyph]` *is* the `LayoutLine`'s vector
  though, so `ptr::eq(line.glyphs.as_ptr(), run.glyphs.as_ptr())` finds
  the line; `buffer.lines[run.line_i].layout_opt()` is still populated
  right after `shape_until_scroll`. cosmic-text centres the glyph box in
  the line height (`LayoutRunIter::next`: `line_y = line_top +
  (line_height − (max_ascent + max_descent)) / 2 + max_ascent`).
- tiny-skia 0.12: `Paint { blend_mode: BlendMode::Multiply | Screen }`,
  `PathBuilder` cubics for rounded corners; `fill_rect` has neither.
- `Page::rects_for_range` feeds `range_rects`, `range_rects_on_page`,
  the frame's `damage_for`, the widget's `notify_selection`/`word_at`,
  and the reference viewer's TTS extents — it now returns the band
  rects, which is what every consumer actually wants (chips, damage,
  handles all sit on the visible band).
- `selection_drag` moves the *focus* (`selection.1`) and keeps the
  anchor (`selection.0`); `selected_range()` normalises. So a handle
  press only has to swap the pair so the grabbed end is the focus, and
  the existing drag handlers (both modes) do the rest.

**Change (this commit).**
- Paint: `LineFragment { ascent, descent }`, `band_extent(line_h)` =
  `[baseline − ascent − 2, baseline + descent + 2]` clamped to the line
  box; `DisplayOp::Band { rect, color, radius: 2, blend }`; `Blend
  { Normal, Multiply, Screen }`; `Selection.blend` (stored highlights
  `Normal`, live selection `Session::selection_blend()` = Rec. 601 luma
  of the palette ground < 128 → `Screen`, else `Multiply`). Math lines
  use the formula's ascent/descent; PDF hidden text 0.8/0.2.
- Backend: `fill_band` = rounded path + blend mode; `zoom.rs` scales it.
- Session: `selection_grab_end(start: bool) -> bool`.
- Widget (`kalam-reader`): new `handles.rs` (geometry, hit-test, painter
  — bar on whole device pixels, teardrop as tangents + four arcs);
  `KalamTheme::handle()` colours; `Inner.handles` remembers the last
  painted pair; `drag_begin` tests the handles before links/selection
  and grabs the end; `drag_end` reports the selection after a grab (the
  chip re-places itself); handles are painted after the page in paged
  mode and after the dividers in scrolled mode. `SelectedText` gains
  `start_rect`/`end_rect` (informational; Kalam reads `text`/`rect` by
  field so nothing breaks). Cursor: a `gtk::EventControllerMotion` on
  the area sets `set_cursor_from_name("grab")` over a handle's hit rect,
  `"grabbing"` from the press until release, `None` off the handles or
  on leave — set only on change (`Inner.cursor`).
- Tests: `page.rs` unit tests for `band_extent`; pagination test that
  the band is glyph box + padding at line-height 2.2 and the painted op
  matches `rects_for_range`; `bidi.rs` reads `Band` ops; session tests
  for grab-end (start fixed / ends cross) and blend by ground;
  `handles.rs` tests for placement, hit areas, overlap preference, and
  painted pixels; `view.rs` test for `ends()`.

**To watch.** Handle grips in a strip when the selection's first or
last line is scrolled off screen (the handle is simply not painted —
`ends()` sees only visible bands); a selection across two bands of the
same chapter works (rects come from every visible band).

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

## R15. Why a whole chapter rendered as one paragraph (2026-09-12)

The user compared the same book (Nyxia, Penguin Random House, EPUB 3) in
Kalam-with-engine and in the old WebKit reader (`docs/files/kalam-engine.png`,
`docs/files/webkit gtk.png`, `docs/files/Nyxia (Reintgen Scott).epub`).
In the engine the chapter heading, subtitle and every paragraph ran
together in one flush-left block with no indents; WebKit showed the
book's design (sans heading, bold grey subtitle, 1em indents, justified).
Two independent causes, both confirmed by reading the file.

**Cause 1 — the parser.** The chapter opens
`<body><a id="d1-d2s3d3s3"/><div class="page_top_padding"><span
epub:type="pagebreak" id="page3" title="3"/>`. EPUB content documents are
XML, and `<a …/>` is an empty element there. `parse_xhtml` used only
html5ever, the *HTML* parsing algorithm, in which `/>` on a non-void
element is ignored: the `<a>` opens and never closes, the `<div>` and
every `<p>` become its descendants, and since `<a>` is inline,
`build_block` sees no block child and `collect_inline` flattens the
whole chapter into a single inline run (`boxtree.rs`: "block-in-inline
… v1 flattens it into the surrounding inline flow"). WebKit picks its
parser by media type (`application/xhtml+xml` → XML) and never saw the
problem. Every PRH title uses this pattern (the `<a id>` is the
Adept/DRM anchor; the `<span epub:type="pagebreak"/>` is the print page
marker), and so do many other publishers, so this was not one book.

Fix: `parse_xhtml` runs xml5ever first over the same `TreeSink`, and
falls back to html5ever when the XML pass reported *any* parse error
(one recovered error means the tree may already be shaped by recovery
rules) or when the root is not an XHTML `<html>` (an XML parse of a
namespace-less document leaves every element in no namespace, where the
UA sheet and `is_html_element` cannot see them; the HTML tree builder
puts them in XHTML). The `strict-xml` feature that upstream documented
was never wired to anything — the crate had the dependency declared
optional and no `cfg` read it.

xml5ever facts that matter (crate source read, 0.39/0.40): namespaces
are bound properly (`process_namespaces` → `bind_qname`), so
`xmlns="http://www.w3.org/1999/xhtml"` puts elements in `ns!(html)`,
inline `<svg>`/`<math>` land in their own namespaces exactly as
html5ever's foreign-content rules did, and `xlink:href` keeps
`ns!(xlink)` (the SVG re-serializer relied on that). `epub:type`
attributes bind to the epub namespace, which `ElementData::attr()`
(empty-namespace lookup) simply does not see — same as before. Named
entities: xml5ever's `NAMED_ENTITIES` is markup5ever's full HTML table,
so `&nbsp;` does *not* error in the XML pass (the "named entity XML does
not define" case in the fallback test still falls back because of the
unclosed `<p>` and bare `&`). The tree builder drops whitespace-only
character tokens only at document level (before the root and after it);
whitespace between `<html>` and `<head>` is a real text node.

**Consequence — `LOCATOR_VERSION` 2 → 3.** That `<html>`–`<head>`
whitespace node is exactly what html5ever discards ("before head"
insertion mode drops whitespace), so the locator text of every
well-formed chapter gained a `\n` at the front: all seven golden anchors
in `locator_text.rs` moved by +1, and the three CLI layout snapshots'
`loc=` columns with them. For mis-nested files the shift is arbitrary
(the tree itself changes). Per `docs/LOCATORS.md` the version bumps and
old offsets take the quote path; Kalam's stored positions from the
engine builds so far (all pre-release) heal on first open.

**Cause 2 — the adapter's settings.** `KalamPrefs::reading_settings()`
hard-codes `publisher_styles: false`, `justify: false` and
`font_family: Some(BODY_FONT)`. `publisher_styles: false` makes
`chapbook-reader/src/layout.rs` pass no author sheets to the cascade,
so the book's margins, `text-indent`, `text-align`, font sizes and the
`.sans` heading family were all thrown away, and the flat block became
a *flush-left* flat block at one size. The old WebKit reader
(`epub_book.rs:3025–3070` on calibre-alt `main`) never did that: its
injected sheet overrides, with `!important`, only colours, `font-size`
and `line-height` on `html, body` and the block elements, `font-family`
on `body` only, heading `font-weight: 650; line-height: 1.25;
margin-top: 1.4em`, link decoration, and `img { max-width: 100% }`. The
book's own stylesheet stayed in force for everything else. So "Kalam's
theme wins over the publisher's" (PLAN §10) meant *colours and reading
typography*, not layout. This is round 2 of the fix (next entry).

What WebKit's screenshot shows that the engine must reproduce for this
book, for reference: `p.para-p { text-indent: 1em; text-align:
justify; margin-bottom: 0.2em }`, `p.para-pf` (first paragraph) no
indent, `.para-cda-alt-chap-pg { font-size: 1.57em }` + `.sans`
(Helvetica stack) for "DAY 1, 8:47 A.M.", `.para-cda1 { font-weight:
bold; font-size: 1.06em; color: #616265; margin-bottom: 3.91em }` for
"Aboard *Genesis 11*", `div.page_top_padding { margin-top: 10% }`.

## R16. The book's stylesheet wins; Kalam skins it (2026-09-12)

Round 2 of the formatting fix (cause 2 in R15). What changed, and the
facts that decided each piece.

**The setting.** `KalamPrefs::reading_settings()` now says
`publisher_styles: true`, `justify: false` (the book decides),
`font_family: Some("Literata")`, and a new `user_css: Some(skin)`.
`ReadingSettings` gained the `user_css: Option<String>` field
(`chapbook-core/src/page.rs`, default `None`, hashed into `cache_key`
so a skin change relayouts); `StyleEngine::new` appends it at user
origin *after* the theme and typeface sheets. Only one constructor in
the workspace built `ReadingSettings` without a `..` spread (prefs.rs),
so the field cost no other call site.

**Why a sheet and not more settings.** The old WebKit reader
(`epub_book.rs` `reading_css()` on calibre-alt `main`) expressed its
claims as CSS with `!important`, injected after the book's. The cascade
gives a user-origin `!important` declaration precedence over every
author declaration, `!important` or not, and a plain user-origin
declaration precedence over the UA sheet only. That is exactly the two
strengths a skin needs — "mine regardless" and "a default the book may
override" — and stylo implements it; a setting per property would have
re-implemented it badly. Tests in `tests/cascade.rs` pin both
strengths and the ordering after the typeface sheet.

**What the skin says** (`KalamPrefs::skin_css`, all `!important`):
`* { color: <ink>; background-color: transparent }` — the engine's
themes force colours only for `Dark` and let author colours through
for Sepia/Light, but the old reader forced them in every theme (the
grey `.para-cda1` subtitle was the ink in WebKit's screenshot);
`html, body { font-size: <px>; line-height: <lh>; margin: 0; padding:
0 }` — size on the roots only so the book's `em` sizes still scale;
`p, div, li, … { line-height: <lh> }` because prhStyle sets
`body { line-height: 1.2 }` and `div, span, blockquote { line-height:
inherit }`, which beat a UA-origin default on every PRH book;
headings `font-weight: bold; line-height: 1.25; margin-top: 1.4em`
(the old reader's `650` is not a weight the bundled faces have —
Literata ships Regular/Bold — so `bold` it is); `a { text-decoration:
none }`. Not carried over: `img { max-width: 100% }` (the paginator
fits images to the measure already), the WebKit-only
`-webkit-text-fill-color`, the selection/band/chip rules (native now).

**The typeface rule's scope.** Upstream's `font_family_css` wrote `*
{ font-family: X !important }`. With publisher styles on, that turned
the book's `.sans` heading into Literata — the old reader forced the
family on `body` only. The rule is now `html, body` (both, because
`body { font-family }` is where publishers put it and `:root` alone
would lose there — the trap upstream's comment warned about). The
monospace exemption stays. Two cascade tests were rewritten to the new
contract (`a_chosen_family_replaces_the_body_font_and_nothing_the_publisher_chose_directly`).

**Family lists.** `style_to_attrs::attrs_for` took the *first* family
of the computed list and handed it to cosmic-text, whose fallback is
per glyph, not per list: `Family::Name("Helvetica")` on a machine
without Helvetica matched nothing in `db.query`, and the iterator then
walked its platform list (`Noto Sans` heads the Linux one — that is
why the heading happened to come out sans here) and finally *any*
face in database order. `Georgia, Palatino, …, serif` would have gone
the same way — a serif list ending up in whatever face sorted first.
New `family_for(style, known)` walks the list as CSS says: first named
family the database has a face for, else the first generic, else
`serif`. The paginator answers `known` from `fonts.db().faces()` with a
per-layout `HashMap<String, bool>` cache. `register_font` aliases
embedded faces under their CSS family name, so publisher `@font-face`
families still match by name. Unit tests sit in `style_to_attrs.rs`.

**What to expect on Nyxia now** (verification for the next screenshot):
"DAY 1, 8:47 A.M." in Noto Sans at 1.57× the base size, left; "Aboard
*Genesis 11*" bold Noto Sans, the ink colour (not `#616265`), italic
title; ~4 em of space after it (`margin-bottom: 3.91em`); first
paragraph unindented, the rest indented 1 em and justified;
`div.page_top_padding { margin-top: 10% }` above the heading on page 1;
the reader's line height throughout the body. Kalam's `font_px` and
`line_height` sliders should visibly move the page; the theme cycle
should recolour every run including the subtitle.

**Open.** (1) The skin's `* { background-color: transparent }` also
clears a publisher's deliberate boxes (sidebars, code blocks) — the old
reader did the same, and the user asked for the old reader. (2)
`justify: false` means Kalam no longer offers justification of its
own; books that justify still do (Nyxia does). (3) The `.sans` heading
needs Noto Sans to be *known* — it is bundled, so it always is.

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
