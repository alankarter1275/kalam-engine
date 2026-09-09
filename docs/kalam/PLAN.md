# kalam-engine: the plan

*Written in plain language on purpose. The owner of this project is not a
software engineer and builds it with AI assistance. If you are an AI reading
this at the start of a session: this file is the agreed direction. Follow
it. Do not re-argue it unless the owner asks.*

---

## 1. What this is, in three sentences

Kalam (`calibre-alt`) is a desktop ebook app for Linux. Today it shows books
through WebKit, a full web browser, which is slow to load on low-end
hardware (4 GB RAM, hard disk). `kalam-engine` replaces that with a small,
fast, pure-Rust reading engine — and rather than build one from scratch, we
forked an existing one, **Chapbook**, and are stripping it down to what
Kalam needs.

## 2. How we got here (so nobody re-litigates it)

Three options were weighed:

| Option | Verdict |
|---|---|
| **Keep WebKit** | Works, but heavy: ~100 MB of libraries loaded from disk on every launch, 200–300 MB RAM, and bookmarks/dictionary need fragile JavaScript workarounds. That load time is the whole complaint. |
| **Build our own engine** (the original kalam-engine plan) | Text layout is one of the hardest problems in software. Months of work to reach "worse than Chapbook", and the failures are subtle and visual — the worst kind to debug without an engineer. Rejected. |
| **Fork Chapbook** | Someone already did the hard 90%. It is ~20× less code than WebKit, ~3–5× lighter at runtime, in the same language as Kalam. Chosen. |

Two ways to use Chapbook were also weighed: depend on it as an external
package (light touch, take the author's updates automatically), or **copy it
and own it** (complete control, a codebase small enough to keep in one head,
updates taken by hand). The owner chose to own it. That is the choice this
document serves.

Chapbook is itself developed with AI assistance, by a single author who
clearly knows the field deeply. Its code is full of deliberate decisions
that *look* simplifiable but are not. Respect that (see §5).

## 3. The three-word rule: delete, don't rewrite

We want to keep taking bug fixes from Chapbook after we fork. A fix from
upstream only applies cleanly if the file it touches still looks the way
the author left it. So:

- **Delete whole crates freely.** A crate that is gone cannot conflict.
- **Inside crates we keep, change as little as possible.** Every edit to an
  inherited file is a future merge conflict.
- **Kalam-specific code goes in *new* files and *new* crates**, never by
  editing theirs. The Kalam adapter (§6) is a new crate.
- If a change inside an inherited crate is truly needed, keep it small,
  keep it in its own commit, and say `kalam:` at the start of the commit
  message so it is easy to find later.

How to actually bring a fix over is in [`UPSTREAM.md`](UPSTREAM.md).

## 4. What stays and what goes

Chapbook tries to be an engine for every device and every format. Kalam is
one Linux desktop app with its own database. The author designed each
extra as a detachable piece, so the strip is mostly *deleting whole
folders*, not surgery.

### Stays (the engine)

| Crate | What it does | Notes |
|---|---|---|
| `chapbook-core` | Shared basics: geometry, page settings, reading positions (`LayeredLocator`), the book model | Contains a little e-ink plumbing (`mezzotint`). Leave it; it is small and woven in. |
| `chapbook-epub` | Opens the EPUB file: manifest, spine, table of contents, embedded fonts | Built on the `rbook` crate |
| `chapbook-layout` | The heart: parses the chapter, applies its CSS (**via stylo**, see §5), lays out pages with `cosmic-text` | The biggest crate. Do not touch. |
| `chapbook-paint` | The page as a list of drawing commands, independent of any screen | |
| `chapbook-render-tinyskia` | Turns that list into pixels on the CPU | |
| `chapbook-reader` | The "session": open / turn page / select text / remember position. **This is what Kalam talks to.** | Its feature switches turn formats off; see below |
| `chapbook-viewer-gtk` | Chapbook's small GTK4 window | Keep as the working reference and the starting point of Kalam's widget |
| `tools/chapbook-cli` | Runs each stage by hand from the terminal | Very useful for debugging; trim its format support along with the rest |
| `fixtures/` | Small test books and fonts | Keep the EPUB ones and the render goldens; drop `cbz/`, `pdf/`, `opds/` |
| `docs/` | Chapbook's design docs | Keep; they explain the code. `PLATFORM.md` can go once the platforms are gone. |

### Goes (in this order — least connected first)

| Step | Remove | Why it is safe |
|---|---|---|
| ~~1~~ | ~~`android/`, `ios/`, `windows/` folders; `crates/chapbook-jni`, `chapbook-ffi`, `chapbook-viewer-win32`~~ | Other platforms. Nothing in the engine depends on them. **Done.** |
| ~~2~~ | ~~`crates/chapbook-render-vello`~~ | GPU renderer. Desktop CPU rendering is instant already. **Done.** |
| ~~3~~ | ~~`crates/chapbook-opds`, `crates/opds-client`, `crates/chapbook-sync`, `crates/chapbook-annotations`~~ | Online catalogs and cloud sync. Takes all networking (TLS, HTTP) out of the build. Kalam does not fetch books. **Done.** |
| ~~4~~ | ~~`crates/chapbook-pdf`, `crates/chapbook-cbz`~~ | PDF and comics. Kalam handles those with native GTK components. **Done.** |
| ~~5~~ | ~~`crates/chapbook-app`, `crates/chapbook-app-gtk`~~ | Chapbook's own full app (bookshelf + reader). Kalam *is* the app. `chapbook-viewer-gtk` is the one to keep, not this. **Done** (same commit as 3 and 4 — it was their last user). |
| ~~6~~ | ~~`crates/chapbook-viewer` (the `winit` one)~~ | Second demo window. GTK one is the one we build on. **Done** (early — it was vello's only consumer). |
| 7 | `mathml` feature (and the 820 KB STIX font in `chapbook-layout/assets/`) | Math formulas. **Kept, by decision** (2026-09-08): it may be needed later, and it costs nothing at runtime for books without math. |
| ~~8~~ | ~~`crates/chapbook-library`~~ | Chapbook's own SQLite bookshelf: books, positions, highlights. Kalam has its own database. The *reading session* used it to save and restore your place and to store highlights, so it had to wait for the adapter's position and highlight hooks (§6). **Done** (2026-09-09): the engine now writes nothing to disk, and `rusqlite`/bundled SQLite left the build with it. |

Each step: delete the folder, remove its line from the workspace
`Cargo.toml` `members` list and `[workspace.dependencies]`, remove the
matching feature from `chapbook-reader/Cargo.toml` if it has one, run the
gate (§8), fix whatever complains, commit. One step per commit.

How it actually went: two commits (steps 1+2+6, then 3+4+5), because the
machine that compiles is the target box and one long build per round was
kinder to it than six. The reader's source files were not edited: its
`#[cfg(feature = "cbz")]`-style switches are simply never turned on any
more (a `check-cfg` line in its `Cargo.toml` tells the compiler those
names are known, so it does not warn about them). Tests that needed a
comic or PDF fixture were put behind the same switches, so they compile
out instead of failing to find their files; the few that could not be
switched off were removed.

Expected result, measured at import: Chapbook's own code is ~62 K lines of
Rust across 22 crates. The crates that stay total ~29 K lines (of which
`chapbook-layout` is 10.5 K and `chapbook-reader` 9.5 K), so the strip
removes a little over half. The third-party dependency count drops from
~240 to well under 200, and the whole networking stack leaves the build.

Actual result after round 2: 32 K lines across 9 crates (the extra 3 K
over the estimate was `chapbook-library` and the CLI), and `Cargo.lock`
went from 618 packages to 351. No HTTP, TLS, PDF or GPU code is left in
the build. `chapbook-library` followed in §7 step 4, taking SQLite with
it.

## 5. Things that must not be removed (and why)

- **stylo** (the CSS engine from Firefox), and with it `html5ever`,
  `selectors`, `cssparser`. Chapbook's layout is *built around* stylo;
  removing it means rewriting the layout engine, which is the "build our
  own" path we rejected. It is heavy to **compile** (the first build is
  slow) but light to **run** (milliseconds per chapter, a few MB). The
  original kalam-engine `RESTRICTIONS.md` banned stylo for reasons that
  were partly wrong (it does not need a C++ toolchain) and are now
  superseded. The bans that *still* hold: no WebKit, no browser engine, no
  JavaScript.
- **`cosmic-text`, `tiny-skia`, `rbook`** — the text, pixels, and EPUB
  layers. Obviously.
- **The `LayeredLocator` design** in `chapbook-core` and everything in
  `docs/LOCATORS.md`. This is how a bookmark survives a font-size change or
  a re-downloaded edition. It is the best part of Chapbook. Kalam's
  database should store these records whole, as-is.
- **The tests and golden files** (`fixtures/render/`, the `insta`
  snapshots). They are how we know a page still looks right after a
  change without an engineer eyeballing it.

## 6. The Kalam adapter (new code, ours)

After the strip, one new crate — working name `kalam-reader` — sits between
Kalam and the engine. It starts as a copy of `chapbook-viewer-gtk/src/`
(`linux.rs` is the loop, `page_area.rs` is the widget) and grows into:

- a GTK4 widget Kalam can drop into its reader page,
- keyboard and mouse handling (arrows turn pages, click-drag selects),
- applying Kalam's theme (font, size, line height, colors) through the
  session's `ReadingSettings`,
- a **dictionary hook**: on a word tap, hand the word to Kalam's
  `src/dict.rs`,
- a **position hook**: on every page turn, hand the `LayeredLocator` to
  Kalam's library database; on open, ask for the last one back. This is
  what lets `chapbook-library` go.

- a **restricted font set**. The reference viewer loads every font on the
  system (`FontSource::host()`); on the owner's machine that is 1031
  fonts, ~150 MB of mostly memory-mapped files, and a share of the
  5-second cold start on an HDD. Kalam should use `FontSource`'s `Dir`
  mode with a small bundled folder: the reader font, plus fallbacks for
  the scripts Kalam's users read (Devanagari, Arabic, CJK as needed).
  Chapbook's own `chapbook-core/src/font.rs` explains the three axes
  (faces, generics, fallback) and why they are explicit.

`docs/SHELLS.md` is Chapbook's contract for exactly this job. Read it before
writing the adapter.

**Status (2026-09-09):** first version written — `crates/kalam-reader`
(`ReaderView`, `KalamPrefs`, bundled Literata + Noto Sans compiled in with
`Faces::Bytes`, not a font folder) and `tools/kalam-reader-demo`. Paged
view; continuous scrolling is the next step. Its `README.md` is the
current description; where it and this section disagree, the README is
right. Two small engine additions were needed and are logged in
`UPSTREAM.md`: a `Palette` on `ReadingSettings` (Kalam's exact colours) and
`host_position.rs` (positions as values for Kalam's database). The
engine half of continuous scrolling (`scroll.rs`) followed on 2026-09-09;
the widget half is round H in §7.

## 7. The order of work

1. ~~**Verify on real hardware.**~~ *Done 2026-09-08.* Built in 15 min on
   the 4 GB / HDD Arch machine; real books open (5 s cold, 1 s warm);
   151 MB RSS (mostly the 1031-font host catalog — see §6); the only
   visual faults were publisher CSS and a Symbol-encoded font, both
   answered by the `f`/`s` overrides that Kalam's theme will apply
   permanently. One upstream bug found and fixed along the way
   (`publisher_styles` was never read). Verdict: the engine holds up;
   proceed.
2. **Strip**, steps 1–6 of the table in §4 (7 was kept). **Done** in two rounds, both built and run on the target machine.
3. **Build the adapter** (§6) against the stripped engine. **Written
   2026-09-09 and run on the target machine the same day**: everything
   works (themes, font size keeps the place, selection, highlights,
   tap-to-look-up, durable locator in 0.2 ms). The first run showed
   **3.9 s to the first page and 150 MB resident**, which looked like a
   failure of §10 — so the engine was made to log where every
   millisecond goes before anything was tuned. **The measurement
   cleared the engine**: it opens a 700-page novel in ~190 ms (of which
   ~200 ms was the library's file hash, gone with step 4), lays out a
   chapter in 34–42 ms and paints the first page in 58 ms. The other
   3.3 s is GTK/GDK starting up from a cold hard disk, and the memory is
   GTK's too (69 MB before the window exists, 143 MB with it; the
   engine's cache holds 2 MB). Kalam has already paid both by the time
   the reader opens, so the demo's numbers are not what Kalam will feel.
   Two bugs found by the run and fixed: selected text carried the
   publisher's soft hyphens (`Har\u{ad}ry`) into what Kalam would store,
   and the engine's 192 MB comic-sized cache default was inherited
   unchanged (now 32 MB). **Done; the engine is not to be tuned further
   without a new measurement that says so.**
4. **Remove `chapbook-library`** (step 8). **Done** (2026-09-09), in two
   commits: first highlights moved under Kalam's ids
   (`host_highlights.rs` — the engine paints, Kalam stores), then the
   crate, its feature, its tests and the CLI's `lib` commands went. The
   session now remembers nothing between runs: Kalam hands back the
   position (`goto_locator`) and the highlights (`set_highlights`) on
   open.
5. **Continuous scrolling** (owner's request, 2026-09-09: the whole book
   as one strip, done *before* the Kalam wiring so the widget arrives
   with both modes). Three rounds:
   - **G — engine** (`chapbook-reader/src/scroll.rs`, done): the by-page
     surface — trimmed page extents (`used_height`, and the `gap` a
     break discarded, recorded by the paginator), a frame and hit-tests
     for any page, `set_position` for the shell to report where the
     reader scrolled to, `settle` to land jumps without a frame,
     `layout_generation` to hear that a font change dropped the layouts,
     `pin_units` so the visible chapters are not evicted mid-frame.
     Paged mode untouched; tests in `tests/scroll.rs` and
     `chapbook-layout/tests/pagination.rs`.
   - **H — widget**: the strip in `kalam-reader` (GTK scrolling,
     estimated heights for chapters not yet laid out from
     `chapter_char_count`, corrected on layout; only the visible band
     drawn); a demo key to switch modes. The owner tests here.
   - **I — polish** after that run: a tile cache if drawing stutters,
     chapter separators, whatever the run shows.
6. **Integrate into Kalam**: path dependency, swap the WebKit view for the
   widget.
7. **Monthly**: review upstream and bring over fixes ([`UPSTREAM.md`](UPSTREAM.md)).

## 8. The gate

Before any commit — the same three commands Chapbook's CI runs, on the
pinned toolchain in `rust-toolchain.toml`:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets     # CI treats warnings as errors
cargo test --workspace
```

Chapbook's `CONTRIBUTING.md` explains the two kinds of golden tests and how
to update them deliberately. Its rule is ours: *a golden that changed for a
reason you cannot name is a bug you are about to bless.*

## 9. Hardware notes (the owner's machine)

4 GB RAM, hard disk, Arch Linux desktop. What that means in practice:

- First build: expect 30–60 minutes and 5–10 GB of disk under `target/`.
  Close the browser. Use `cargo build --release -j 2` so cargo does not run
  many compilers at once. Make sure swap is on. This is a one-time cost.
- Later builds: fast. **Never `cargo clean`** casually — it deletes the
  cache that makes them fast.
- **Always judge speed with `--release`.** Debug builds of text engines are
  5–10× slower and prove nothing.
- Python 3 must be installed (stylo's build step uses it). Arch already
  has it.
- **Arch Linux specifics.** Install Rust through `rustup` (`pacman -S
  rustup`), *not* the plain `rust` package — the repo pins an exact
  compiler in `rust-toolchain.toml` and only `rustup` can obey that; with
  the distro compiler cargo prints "toolchain '1.98.0' is not installed"
  and stops. `pacman -S gtk4` is the whole GTK requirement (headers are
  included; no `-dev` split on Arch). `pkgconf` and `base-devel` supply
  `pkg-config` and a C compiler, which some dependencies' build scripts
  need.

## 10. What the finished thing should feel like

Open a 400-page novel: the first page appears in well under a second.
Turning a page: instant. Memory while reading: under 100 MB. Bookmarks,
highlights, and your reading position survive changing the font size,
resizing the window, and re-importing the book. Kalam's theme always wins
over the publisher's. No browser, no JavaScript, nothing downloaded.

---

*History: the original kalam-engine (a ~100-line scaffold for an engine to
be written from scratch on `lol_html` + `cosmic-text`) and its docs are
archived under [`archive/`](archive/). They describe a design that was
abandoned in favour of this plan on 2026-09-08.*
