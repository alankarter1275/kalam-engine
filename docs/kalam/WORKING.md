# Working in this repository — for AI agents

Audience: an AI coding agent (or a human engineer) starting a session on
this repo. It is deliberately technical. The owner's plain-language
document is [`PLAN.md`](PLAN.md); read that for *why*. This file is
*how*: the constraints, the traps that have already cost time, the
feedback loop, and the standing instructions from the owner that are
not written anywhere else. The research behind each decision is in
[`RESEARCH.md`](RESEARCH.md).

Everything here was true on 2026-09-10. Verify before relying on a
version number.

## 1. The situation in one screen

- **What this is.** A soft fork of [ophymx/chapbook](https://github.com/ophymx/chapbook)
  (imported with full history at `ab14cb7`, 2026-09-07), stripped to an
  EPUB engine, plus two crates of our own: `crates/kalam-reader` (the
  GTK4 widget) and `tools/kalam-reader-demo` (a bare window around it).
  It exists to replace WebKitGTK inside **Kalam** (`calibre-alt`, a
  private GTK4/relm4 desktop ebook app owned by the same person).
- **Owner profile.** Not a software engineer; vibe-codes; wants complete
  control of the code (hence the fork, not a dependency). Machine: 4 GB
  RAM, HDD, Arch Linux, desktop only. Prefers building and running as
  two commands, not `cargo run`. Shell has `grep` aliased to ripgrep
  (`rg`) — give `rg` commands, never `grep -E`.
- **The owner is the compiler.** The sandbox agents run in has no Rust
  toolchain and no crates.io access (only github.com). GitHub Actions
  is the compiler for correctness; the owner's machine is the only
  place windows open and performance is measured. Every round ends with
  the owner running the build and reporting back.
- **Phase.** Engine stripped, widget built with paged + continuous modes,
  perf targets met on the engine side. Current step: wiring the widget
  into Kalam ([`INTEGRATION.md`](INTEGRATION.md)). After that: the
  deferred list (§8).

## 2. Hard rules (owner directives — do not relitigate)

These were stated explicitly by the owner. Several were reached after an
argument the agent lost; do not reopen them.

| Rule | Origin |
|---|---|
| **Fork and own; do not pin Chapbook as a dependency.** Argued twice for pinning; rejected both times. | owner, 2026-09-07 |
| **Keep MathML** (`mathml` feature, the formulary crate path, the STIX font). "It might be needed in future." | owner, 2026-09-08 |
| **Engine must stay lighter than WebKit** — no browser engine, no JavaScript, no network, no PDF/comics, no DB inside the engine. stylo (the CSS engine) is *allowed*. | PLAN.md §1, §5 |
| **Kalam owns persistence.** The engine writes nothing to disk; position and highlights are values handed to Kalam's SQLite. `chapbook-library` was removed for this. | PLAN.md §7 step 4 |
| **Whole-book continuous scroll**, not per-chapter. Per-chapter was proposed and rejected. | owner, 2026-09-09 |
| **Fonts: what Kalam already bundles/uses; do not ask the owner to choose.** Result: Literata (body) + Noto Sans (UI), embedded via `include_bytes!` in `kalam-reader/src/fonts.rs`. | owner ("see for yourself") |
| **Colours: read from Kalam's theme code (`ReadingTheme::swatch()`), never invent a palette.** | owner |
| **A visible chapter divider** in scrolled mode (hairline + title pill, as Kalam's WebKit reader drew). | owner ("yeah, I want one") |
| **Discuss before coding.** Present the plan, get "go", then write. Explanations must be plain language for the owner — jargon-heavy design talk was rejected as incomprehensible. (This file is the exception: it is for agents.) | owner, repeatedly |
| **Delete whole crates freely; rewrite inherited files rarely.** Kalam-specific code goes in *new* files/crates. A commit that edits an inherited file gets a `kalam:` prefix and a row in [`UPSTREAM.md`](UPSTREAM.md). | PLAN.md §3 |
| **Do not tune the engine for speed without a new measurement that says the engine is the cost.** Measured 2026-09-09: engine ≈ 60 ms to open a novel, ≈ 40 ms per chapter layout; the seconds and the megabytes are GTK/GDK start-up. | PLAN.md §7 step 3 |
| **Scratch branches from the owner are invisible to the agent.** Files the owner wants read must be committed to the arena branch temporarily (and removed after). Chat attachments do not reach the sandbox. | owner |
| **Keep rounds small enough to bisect** — the owner builds after each. | owner |

## 3. The build/CI loop

### What runs where

| Check | Where | Notes |
|---|---|---|
| `cargo fmt --all --check` | CI | No rustfmt in the sandbox. rustfmt's `small-heuristics` wrap chains/calls at ~60 cols; expect one formatting fix-up commit per round when hand-formatting. |
| `cargo clippy --workspace --all-targets` with `RUSTFLAGS=-D warnings` | CI | Traps hit so far: `type_complexity` on tuple-typed `let`s (introduce a type alias); `field_reassign_with_default` (use struct-init with `..Default::default()`); unused imports after a feature trim. |
| `cargo test --workspace` | CI | Golden tests exist in `chapbook-layout`; see `CONTRIBUTING.md` for the update rule. |
| **`lockfile` step** (`cargo metadata --locked`) | CI, first | **`Cargo.lock` is maintained by hand** (no cargo in the sandbox). Every new dependency or workspace member needs a hand-written `[[package]]` entry with the right `dependencies` list and checksum. When it fails, the step prints the diff cargo wanted — apply that verbatim. |
| Windows, performance, visual checks | owner's machine | `cargo build --release -j 2 -p kalam-reader-demo && ./target/release/kalam-reader-demo book.epub 2> log.txt`. Filter with `rg 'strip:\|frame took\|memory\|counted\|switched\|prefetched' log.txt`. |

Toolchain pinned in `rust-toolchain.toml` (`1.98.0`) and mirrored as
`RUST_PIN` in `.github/workflows/ci.yml` — bump both together.

### Reading CI from the sandbox

```sh
gh run list --branch arena/01a07f3f-kalam-engine --limit 1 --json databaseId,status,conclusion
gh run watch <id> --exit-status --interval 30
# The failing step's text (raw job logs live on Azure and are unreachable):
gh api repos/alankarter1275/kalam-engine/commits/<SHA>/check-runs \
  --jq '.check_runs[] | select(.name=="CI details") | .output.text'
```

The `report` job writes the first failing step's output into a check
run named **"CI details"** precisely because the raw logs cannot be
fetched from the sandbox. Cold CI ≈ 12–15 min; cached ≈ 1.5–3 min.

### Sandbox hazards

- **The sandbox resets to a stale clone between turns** (HEAD at the
  repo's first commit, dirty tree). First command of every session:
  `git fetch origin +refs/heads/<branch>:refs/remotes/origin/<branch> && git reset -q --hard origin/<branch>`
  (if the tree has real edits: `git reset --soft` and recommit). `/tmp`
  is lost too. Never force-push.
- The GitHub token expires now and then: `gh auth status` before a
  push; if it fails, commit locally and tell the owner in one sentence.
- Only github.com is reachable. docs.rs works through the page-fetch
  tool; crates.io's API does not. The crates.io index is readable via
  `gh api repos/rust-lang/crates.io-index/contents/<a>/<b>/<crate>`
  (base64 body, one JSON line per version).
- Commit as `git -c user.name="Arena Agent" -c user.email="agent@arena.ai" commit`.

## 4. Repository map (what matters, where)

```
crates/chapbook-core        inherited: Locator/LayeredLocator, Publication trait, KeyMap/TapZones, geometry, Palette
crates/chapbook-epub        inherited: Book::open (rbook; zip or unzipped dir), resources, fixed-layout detection
crates/chapbook-layout      inherited: stylo CSS + cosmic-text shaping + paginator; golden tests; STIX math font
crates/chapbook-paint       inherited: display lists
crates/chapbook-render-tinyskia  inherited: CPU rasterizer
crates/chapbook-reader      inherited core + our host_*.rs and scroll.rs additions (see UPSTREAM.md tables)
crates/chapbook-viewer-gtk  inherited reference GTK window (kept as a working example; not shipped to Kalam)
crates/kalam-reader         OURS. ReaderView widget: view.rs (paged+scrolled), scroll.rs (Strip model), divider.rs, fonts.rs, prefs.rs
tools/chapbook-cli          inherited CLI (layout/dump tools)
tools/kalam-reader-demo     OURS. Bare window; prints everything the widget reports to stderr as `demo: …`
fixtures/epub/              long.epub (8 ch), minimal.epub, illustrated.epub
docs/kalam/                 PLAN (why), WORKING (this), RESEARCH (findings), UPSTREAM (fork hygiene), INTEGRATION (Kalam recipe), patch/ (drop-in files for Kalam)
docs/*.md                   inherited Chapbook design docs; still accurate for inherited code, wrong wherever they mention removed crates
```

Where the widget's behaviour lives (`crates/kalam-reader/src/view.rs`):
`open()` builds `Inner`; `install_draw` is the draw func (relayout on
size change, centring by `column_px`); `after_draw` runs in an idle →
`sync_adjustment` / `report_position` / `schedule_char_count` /
`schedule_prefetch`; `install_keys` translates GDK key names to
`chapbook_core::Key` and applies the engine `KeyMap` (with `+ = - t m b`
unbound: those are Kalam's); `install_pointer` is one `GestureDrag`
(press → link follow or selection anchor; release without movement →
tap: word lookup, else tap-zone page turn); `install_scroll` is the
wheel (scrolled mode only); `close()` breaks the GTK↔view cycle.

## 5. Kalam (the host app) — what an agent needs to know without its source

Kalam's source is private and *not* in this repo. A snapshot was read
once via a temporary commit (`c7c02b2`, removed in `3f48904`; recover
with `git show c7c02b2:docs/kalam/kalam-src.txt` — 26k lines, `### FILE:`
blocks). Facts that shaped the widget's API, so nobody has to re-read it:

- Stack: relm4 0.9 / libadwaita 0.7 / gtk4 0.9 (`v4_12`) / glib+gio 0.20 /
  webkit6 0.4 / rusqlite 0.32 / serde_json. Must move to relm4 0.11 /
  libadwaita 0.9 / gtk4 0.11 / glib 0.22 to link against this widget
  (one `gtk4-sys` per binary).
- Reader page: `src/pages/reader/{mod.rs, mod_model.rs, types.rs,
  chapter.rs, chrome.rs, js_bridge.rs, settings_panel.rs, ui_prefs.rs,
  session.rs, lists.rs, panels.rs}` — the last three were **not** in the
  snapshot. The WebView is appended to a `web_host` `gtk::Box` inside a
  `reader_stage` box inside a root `gtk::Overlay`; chrome (back dock,
  bottom pill, hover edges, two `Revealer` sidebars) are overlay children.
- Preferences: `reader.theme|font_px|line_height|column_px` via
  `get_pref/set_pref/get_pref_i64`; any key with the `reader.` prefix
  is *global* (not per-library) by `libraries::is_global_pref`. New
  reader prefs must keep that prefix.
- Reading themes: `ReadingTheme::{Light, Sepia, Dark, Ink}` (default
  Sepia), `as_str()/from_str_lossy()`; ranges font 13–24 px (default
  17), line-height 1.3–2.5 (1.8), column 400–860 px (620). The widget's
  `KalamTheme`/`KalamPrefs` mirror these exactly.
- DB: `reading_progress(book_id PK, chapter_index, fraction, updated_at)`;
  `reading_bookmarks(id, book_id, chapter_index, fraction, label,
  created_at)`; `annotations(id, book_id, kind, chapter_index,
  start_path, start_offset, end_path, end_offset, color, text_excerpt,
  note, cfi TEXT (unused), created_at, updated_at)`. `HighlightColor`
  names: yellow/green/blue/pink/orange ("rose" → pink).
- Dictionary flow (kept as is): `catalog.lookup_entry(word) -> EntryData
  {word, pos, senses[{number,pos,def,example}], synonyms, antonyms,
  idioms, suggestions}`, `saved_word_exists`, `pronunciation_for`,
  `likely_sense_index` gated by pref `dict_sense_hint`.
- Fonts Kalam ships: Literata (body) and Noto Sans (UI) — the reason the
  widget bundles those two.

## 6. Design decisions an agent might be tempted to undo

- **Whole-book strip built from per-page rasters** (`kalam-reader/src/scroll.rs`),
  not one tall layout: unmeasured chapters get an estimated height from
  their char count × learned px/char; corrections keep the reading line
  (48 px below the viewport top) on the same text. This is why
  scrolling shows no jumps when the estimate is corrected.
- **Position = `(chapter_index, fraction)` for the cheap per-page save,
  `LayeredLocator` for the durable form.** Kalam's tables already hold the
  pair; the locator (quote context + fraction + book progression) goes
  where Kalam has a text column (`annotations.cfi` for highlights).
  `LayeredLocator` has **no serde**; Kalam serialises it by field.
- **Highlights are the host's**: the engine paints under the host's id
  (`set_host_highlights`), never stores them. Removing `chapbook-library`
  made this necessary and is why `host_highlights.rs` exists.
- **Overlay scrollbar** rather than a sibling: a sibling changes the
  widget width when shown → full relayout (70–85 ms) on every mode switch.
- **`ReaderOptions::cache_budget` default 32 MB**, not the engine's
  comic-sized 192 MB.
- **Char counting in an idle after the first frame** (round I): the
  whole-book count (111 ms on a 1.2 M-char novel) would otherwise stall
  the first scrolled frame.
- **Next-chapter prefetch in a `Priority::LOW` idle**, one chapter per
  idle, only in scrolled mode.

## 7. How a round goes

1. Owner states a goal (often one line). Agent researches (this repo +
   the Kalam snapshot + docs.rs), writes a short plain-language plan,
   waits for "go".
2. Agent writes code + doc updates + `Cargo.lock` entries, commits with a
   descriptive message, pushes, watches CI, fixes fmt/clippy/lock
   fallout in a follow-up commit.
3. Owner pulls, builds `--release -j 2`, runs, pastes filtered log
   lines or a verdict.
4. Agent records the measured numbers in PLAN.md §7 (and RESEARCH.md if
   they change a conclusion). Deferred items go to §8 below.

Never end a research-only turn without writing what was learned into
RESEARCH.md. The owner has explicitly asked that all research be
recorded in the repo for future agents; five consecutive read-only
turns happened once and were rightly called out as slow.

## 7a. The two-agent handoff (step 5 onwards)

Kalam's repo is worked on by a *second* agent that cannot see this one;
the owner relays `=== KALAM REPORT ===` / `=== ENGINE REPLY ===` blocks
between the two chats by copy-paste. Everything that side needs is
generated from this repo by `docs/kalam/handoff/make-bundle.sh`
(→ `./kalam-handoff/`, git-ignored, copied into calibre-alt as
`docs/engine-handoff/`): the recipe, the drop-in `engine.rs`, a
source-generated API reference (`api-reference.py`), the widget README
and the engine commit. Protocol for the other agent: `AGENT-BRIEF.md`;
for the owner: `RELAY.md`. Rules for this side: answer a report with a
reply block only; never ask the owner to interpret; when the widget
needs to change, change it here, push, and reply "engine updated to
<commit>; regenerate bundle". Record anything learned about Kalam's
real tree in RESEARCH.md (R14 open questions first).

## 7b. What was WebKit's and what was Kalam's (read before promising parity)

Kalam's old reader page was two things. The **text** — layout, fonts,
themes, pagination, highlights on the page — was WebKit's rendering,
and the engine replaces it; that swap is measured and done. The
**overlay UI** — selection chip, dictionary popup, chapter divider,
search hits, selection handles — was HTML+CSS+JS *inside* the WebView.
None of it can be copied: GTK CSS lacks `flex`, `color-mix`,
`max-width`, `position: fixed`, and no agent in the loop has a display.
Each overlay element is a redesign that the owner sees first, so plan
two or three screenshot rounds per element and say so up front. Design
sources: calibre-alt `docs/files/*.html` (R12h).

## 8. Deferred (known, not done, in no particular order)

- `ReaderView::search(&str)` — `chapbook_reader::Session::search`
  exists (R13) but the widget does not expose it. The old dictionary
  popup's "Find in chapter" button needs it (R12f).
- "Unsave" from the dictionary card. The old popup's bookmark toggle
  could forget a saved word; the new card only saves ("Saved ✓" is
  disabled afterwards) and unsaving lives on the Words page. Kalam's
  `delete_saved_word_by_word` was deleted with its last caller; a
  "Saved ✓ → tap to unsave" affordance is a small Kalam-side change if
  the owner misses it (R12c).
- Page-raster cache in scrolled mode (drawing was measured at ~8 ms per
  frame, so this is low value until proven otherwise).
- Kinetic (flick) scrolling in scrolled mode.
- Expose `Session::search(query, limit)` on `ReaderView` — it is
  blocking; must run off the UI thread or per unit in an idle.
- Remember the page within a chapter across `n`/`p`.
- Double first layout on window resize at start-up (logged as two
  size changes).
- Demo timing should start from window-shown, not process start.
- Painting WebKit-era highlights (DOM paths) in the engine — needs a
  DOM-path → text-offset mapper; probably never worth it.
- Remote-source placeholder chapters in Kalam (fetch-on-demand HTML):
  the engine reads the file, so those must be fetched at import time.
- Monthly upstream review (`UPSTREAM.md`); last done 2026-09-10.
