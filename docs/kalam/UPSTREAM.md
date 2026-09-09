# Keeping up with Chapbook

We forked Chapbook but want its bug fixes. This is the routine. Once a
month is plenty. Each step is a command you can paste.

## The facts

| | |
|---|---|
| Upstream repo | https://github.com/ophymx/chapbook |
| Imported at | commit `ab14cb78d2f7e63e8e2f7bc066bfc8ee57318cc9`, 2026-09-07 |
| How it was imported | `git merge --allow-unrelated-histories` — the full history is in this repo |
| Last reviewed up to | `ab14cb7` *(update this line every time you finish a review)* |

Because the history was merged rather than copied, Git knows exactly which
upstream commits we already have. That is what makes everything below
one-liners.

## One-time setup (per clone)

```sh
git remote add upstream https://github.com/ophymx/chapbook.git
```

## The monthly review

**1. Fetch what's new.**

```sh
git fetch upstream main
```

**2. List the commits we don't have yet, but only the ones touching crates
we kept.** This ignores everything about Android, iOS, Windows, PDF,
comics, catalogs, sync — i.e. most of the noise.

```sh
git log --format='%h  %ad  %s' --date=short --reverse HEAD..upstream/main -- \
    crates/chapbook-core crates/chapbook-epub crates/chapbook-layout \
    crates/chapbook-paint crates/chapbook-render-tinyskia \
    crates/chapbook-reader crates/chapbook-viewer-gtk tools/chapbook-cli \
    fixtures/epub fixtures/render fixtures/fonts docs
```

Empty output means nothing to do this month. Update the "last reviewed"
line above and stop.

**3. Sort each commit.** Read the message; the author writes long,
explanatory ones. Ask: *is this a fix or improvement to something we
kept?* Three buckets:

| Bucket | Example | Action |
|---|---|---|
| **Take** | "Fix a page break landing inside a heading", "Handle an EPUB whose spine points at a missing file" | cherry-pick it (step 4) |
| **Skip** | Anything about e-ink, Android, iOS, Windows, WASM, PDF, CBZ, OPDS, sync, GPU | nothing |
| **Unsure** | It touches `chapbook-reader` but mentions comics | look at the diff: `git show <hash> --stat`. If it only touches files we deleted, skip. If mixed, cherry-pick and delete the parts that don't apply. |

**4. Bring a commit over.**

```sh
git cherry-pick -x <hash>
```

The `-x` writes "(cherry picked from commit …)" into the message so we
always know where it came from. Three things can happen:

- **Applies cleanly.** Run the gate, commit is already made. Done.
- **Conflict in a file we deleted.** Git says something like
  `CONFLICT (modify/delete)`. That part of the fix is for a crate we
  removed. Resolve with `git rm <that file>` and `git cherry-pick --continue`.
- **Conflict inside a file we kept.** This means we changed that file
  ourselves — which §3 of `PLAN.md` says to avoid, and this is why. Open
  the file, look for the `<<<<<<<` markers, keep both sides' intent, remove
  the markers, `git add` it, `git cherry-pick --continue`. If it is a mess,
  `git cherry-pick --abort` and note the hash in the "deferred" list below
  instead of fighting it.

**5. Run the gate** after each cherry-pick, or after a batch:

```sh
cargo fmt --all --check && cargo clippy --workspace --all-targets && cargo test --workspace
```

If a test fails after a cherry-pick, the fix depended on another upstream
commit you skipped. `git log --oneline <hash>~5..<hash>` shows its
neighbours; usually the missing one is right before it.

**6. Update the "last reviewed" line** at the top of this file and commit.

## Deferred

Upstream commits we wanted but could not take cleanly. Revisit when there
is time; delete the line if it stops mattering.

*(none yet)*

## Things we changed inside inherited files

Kept short on purpose — see `PLAN.md` §3. Every entry here is a future
conflict. Prefix such commits with `kalam:`.

| File | What | Why |
|---|---|---|
| `.github/workflows/ci.yml` | Replaced Chapbook's 9-job matrix with a single Linux job; a `lockfile` step (`cargo metadata --locked`) fails the run when `Cargo.lock` is not what cargo would write | We ship to one platform; the lock file is edited by hand from a sandbox without cargo, and drift there blocks the owner's `git pull` |
| `README.md` | Replaced | Describes this fork, not Chapbook |
| `crates/chapbook-reader/src/layout.rs` | Honour `ReadingSettings::publisher_styles` (pass no author sheets when off) | Upstream flag was persisted but never read — a bug. **Candidate to send upstream.** |
| `crates/chapbook-viewer-gtk/src/linux.rs` | `f` (force reader font) and `s` (publisher styles off) toggles | Testing aid; Kalam's adapter sets both permanently |
| `crates/chapbook-core/src/diagnostics.rs` | Filter html5ever's stale "foster parenting not implemented" warning from the stderr logger | It fires per malformed table row in real books and means nothing |
| `Cargo.toml`, `crates/*/Cargo.toml`, `tools/chapbook-cli/Cargo.toml` | Removed members, dependencies and features that belonged to deleted crates | Strip, PLAN §4. Expect conflicts in these on every cherry-pick that touches a manifest; resolve by hand |
| `crates/chapbook-reader/Cargo.toml` | `cbz`/`pdf`/`opds`/`ureq` features removed; a `[lints.rust] unexpected_cfgs` check-cfg line names them so the untouched `cfg` sites in `src/` and `tests/` stay warning-free | Strip round 2. `src/` itself is unchanged |
| `crates/chapbook-reader/tests/*.rs` | Tests that open a CBZ/PDF fixture gated with `#[cfg(feature = "cbz")]`/`"pdf"` (same style as the two upstream already gated); `sources.rs` splits the `Format` import the same way | Fixtures are gone; the tests compile out instead of failing to find files |
| `crates/chapbook-core/tests/sniff.rs` | CBZ/PDF fixture tests removed; the "bytes beat the extension" test keeps its EPUB half | Fixtures are gone; `Format::sniff` itself is untouched |
| `tools/chapbook-cli/src/{main,commands}.rs`, `tests/`, `examples/show.rs` | `opds`, `lib add`, `lib sync` subcommands and image-book paths removed; `open_publication` is EPUB-only | The crates behind them are gone |
| `crates/chapbook-viewer-gtk/src/linux.rs` | Usage string says `<book.epub>` | Only format left |
| `crates/chapbook-core/src/page.rs`, `src/lib.rs` | New `Palette` type; `ReadingSettings.palette: Option<Palette>` (+ `palette()` accessor, hashed into `cache_key`) | Kalam's four themes have exact paper/ink colours; upstream's `Theme` is three fixed presets. `None` everywhere upstream constructs settings, so upstream behaviour is unchanged. **Candidate to send upstream.** |
| `crates/chapbook-layout/src/cascade/engine.rs` | `theme_css` takes `&ReadingSettings`, uses the effective palette's colours; `Light`+palette gets the Sepia-style sheet | Same feature |
| `crates/chapbook-reader/src/frame.rs` | Background / selection / highlight colours from `settings.palette()` | Same feature |
| `crates/chapbook-layout/tests/pagination.rs` | One new test (`a_palette_supplies_the_colours_and_the_theme_the_rules`) | Covers the feature; appended, nothing existing touched |
| `crates/chapbook-reader/src/lib.rs` | `mod host_position;` and `mod host_highlights;`; `Highlight` lives here (was in `annotations.rs`); `UnitState.resolved_host_highlights`, `Session.host_highlights`; the `library` field set, `OpenedBookId`, `book_id()`, `with_library_dir` and `SessionConfig.library_dir` removed; `save_position()` removed (the host saves what `layered_locator()` returns); `SettingsScope` kept but documented as inert | Host-owned highlights and positions; the library is gone (PLAN §7 step 4) |
| `crates/chapbook-reader/src/frame.rs` | Paints the host's highlights (the library's are gone); `mark_range`/`mark_rect`/`range_damage` ungated | Same |
| `crates/chapbook-reader/src/open.rs` | Library handshake removed (`shelve`, `restored_start`, fingerprinting, stored-annotation load, settings load, OPDS URL branch); every book opens at unit 0 with `ReadingSettings::default()`; initialises `host_highlights` | Same. **This is now the file most likely to conflict on a cherry-pick**; resolve by keeping ours and re-applying only the non-library part of the upstream change |
| `crates/chapbook-reader/tests/cache_budget.rs` | `PAGE` constant no longer gated on the removed `cbz` feature (the EPUB test uses it too) | Strip leftover; the file did not compile until CI ran the tests |
| `docs/STABILITY.md` | Rewritten for the eleven crates that remain, with `kalam-reader` and the demo placed in tiers | The `stability` test in `tools/chapbook-cli` checks the doc against the workspace; upstream's text named twelve crates the strip removed |
| `tools/chapbook-cli/tests/stability.rs` | Member-count floor 15 → 11 → 10 | Same test, same strip (then the library) |
| `crates/chapbook-reader/src/layout.rs`, `src/open.rs` | One `info` log line per chapter laid out (parse / fonts / images / style / paginate, in ms, plus pages and KB) and one per session opened (total, and the font system's share) | Finding where a slow first page spends its time; silent at the default `warn` level |
| `crates/chapbook-reader/src/annotations.rs` | **Deleted** (the library's highlight/note/bookmark storage) | Replaced by `host_highlights.rs`; `UnitCharContext`/`unit_char_context` moved to `host_position.rs` |
| `crates/chapbook-reader/src/cache.rs` | `suspend()` only releases caches; `library_mut()` removed | No database to close |
| `crates/chapbook-reader/src/nav.rs` | `persist_settings` is a no-op; `clear_book_settings` removed | Nothing to persist to |
| `crates/chapbook-reader/src/zoom.rs` | `view_rect`/`map_rect` ungated | They were gated on `library` only because their one caller was |
| `crates/chapbook-reader/src/conformance.rs` | `PositionSurvivesARestart` carries a `LayeredLocator` across the reopen (`layered_locator` → `goto_layered`) instead of `save_position` | Same check, the host's road |
| `crates/chapbook-reader/tests/{common/mod.rs,bidi,cache_budget,conformance,diagnostics,events,frames,lifecycle,positions,session,settings,sources,text_surface}.rs` | Library-dir plumbing removed; tests that asserted library behaviour (persistence across sessions, the shelf, per-book settings) deleted or rewritten as session-scoped / host-driven; `annotations.rs` deleted, `host_highlights.rs` added | Same |
| `crates/chapbook-viewer-gtk/src/linux.rs` | `h` key prints a note and clears the selection (no store to add to); `save_position` calls removed | Same |
| `tools/chapbook-cli/src/{main,commands}.rs`, `tests/lib_shelf.rs` | `lib` subcommand family removed; `lib_shelf.rs` deleted | Same |
| `Cargo.toml`, `crates/chapbook-reader/Cargo.toml`, consumers' `Cargo.toml` | `crates/chapbook-library` member, `rusqlite`, the `library` feature and `chapbook-library` dependencies removed | Same |

New files inside inherited crates (no conflict risk, listed for completeness):

| File | What |
|---|---|
| `crates/chapbook-reader/src/host_position.rs` | `Session::layered_locator()`, `layered_locator_at()`, `goto_layered()`, `unit_fraction()`, `chapter_char_count()`, `word_at_exact()` — positions as *values* for a host with its own database, and the tap hit-test |
| `crates/chapbook-reader/src/host_highlights.rs` | `HostHighlight` + `Session::set_host_highlights()`, `show_host_highlight()`, `recolor_host_highlight()`, `hide_host_highlight()`, `host_highlights()`, `host_highlight_at()`, `goto_host_highlight()` — highlights the host stores itself, held in memory and resolved like stored annotations; not gated on `library` |

## If upstream goes quiet or goes a direction we dislike

Nothing breaks. The last version we took keeps working forever; we simply
stop doing the review. That is the point of owning the copy.
