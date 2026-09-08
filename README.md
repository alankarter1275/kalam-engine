# kalam-engine

The reading engine for **Kalam** (`calibre-alt`): opens an EPUB, lays it out
into pages, draws the pages, remembers where you are. Pure Rust, no web
browser inside.

It is a fork of [ophymx/chapbook](https://github.com/ophymx/chapbook),
imported with its full history at commit `ab14cb7` (2026-09-07) and being
stripped down to what a Linux desktop ebook reader needs. Chapbook is
MIT/Apache-2.0 by Jeffrey T. Peckham; its license files and `NOTICE` are
kept as-is, and the parts of this repo that came from it stay under those
terms.

> **New here? Read [`docs/kalam/PLAN.md`](docs/kalam/PLAN.md) first.**
> It is written in plain language and explains why this repo exists, what
> gets removed, what must not be touched, and how we keep up with Chapbook.
> Every AI-assisted session should start there too.

## Status

| | |
|---|---|
| Imported from Chapbook | done — `ab14cb7` |
| Verified on Kalam's target hardware (4 GB, HDD, Arch) | done, 2026-09-08 — builds in 15 min, opens real books, `f`/`s` overrides confirmed working |
| Stripped to the Kalam subset | **next** — see `docs/kalam/PLAN.md` §4 |
| Kalam adapter (GTK widget, theme, dictionary/position hooks) | not started |

Until the strip happens, this repo builds and behaves exactly like Chapbook
did on 2026-09-07. Chapbook's own docs below are still accurate for it.

## Getting started

Rust via **`rustup`** (not the distro's `rust` package — the repo pins an
exact compiler version in `rust-toolchain.toml`, and only `rustup` can honor
that), Python 3 (used by the CSS engine's build step), and GTK4 for the
viewer:

```sh
# Arch Linux
sudo pacman -S rustup gtk4 pkgconf python base-devel
rustup default stable      # one-time; the repo then pins its own version on top

# Debian / Ubuntu
sudo apt install libgtk-4-dev python3
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

On Arch, GTK headers ship with the `gtk4` package itself (there is no
separate `-dev` package), and Arch's GTK is new enough for the viewer's
accessibility features (needs 4.16+; Arch has had it for a long time).

Build first, run later — two separate steps:

```sh
# 1. Build (slow the first time: 30–60 min; fast afterwards)
cargo build --release -j 2 -p chapbook-viewer-gtk

# 2. Open a book in the reference viewer (instant once built)
./target/release/chapbook-viewer-gtk path/to/book.epub
```

Step 1 succeeded if the last line starts with `Finished`. If it ends with
`error:` lines instead, that is a setup problem — nothing to do with any
book.

Always test with `--release`. A debug build of a text engine is 5–10× slower
and gives a false impression. On a machine with little RAM, add `-j 2` to
the cargo command so it does not compile everything at once; the first build
is slow (the CSS engine is large), every build after that is fast. Do not
run `cargo clean` unless something is truly broken — it throws that cache
away.

Keys in the viewer: arrows / PageUp / PageDown / space turn pages, `+`/`-`
change text size, `t` cycles the theme, `q` quits. Two Kalam additions:
**`f`** forces the reader's own typeface over the publisher's (fixes books
whose embedded font renders as Greek — see below), **`s`** switches the
publisher's stylesheets off entirely (their margins, indents, fonts).
Both are what Kalam's theme will do permanently; in the viewer they are
toggles so you can compare.

> **"My book's contents page looks Greek."** Some (notably Indian-published)
> EPUBs embed a copy of the old *Symbol* font, which draws Greek letters
> in place of Latin ones. The text is fine; the font is lying. Browsers
> carry a decades-old hack for it; Chapbook honours the font literally.
> Press `f`. A proper fix (detect and skip Symbol-encoded fonts) is on
> the list.

> **`html5ever: warn: foster parenting not implemented`** in the terminal is
> harmless. The HTML parser prints it when a book's table has stray text
> between its rows (invalid markup, common in published EPUBs); despite
> the wording it does handle the case — the message is stale. The viewer
> now filters it out of the log.

## Layout of the repo

```
crates/          the engine, one crate per stage — see docs/kalam/PLAN.md
                 for which ones stay and which ones go
tools/           chapbook-cli, a dev tool that runs each stage by hand
fixtures/        small test books and fonts used by the tests
docs/            Chapbook's design docs (still accurate) + docs/kalam/ (ours)
android/ ios/ windows/   Chapbook's platform shells — scheduled for removal
```

## Documentation

**Kalam's:**

| | |
|---|---|
| [docs/kalam/PLAN.md](docs/kalam/PLAN.md) | The plan, in plain language: why, what stays, what goes, the rules |
| [docs/kalam/UPSTREAM.md](docs/kalam/UPSTREAM.md) | How to bring over fixes from Chapbook (commands included) |
| [docs/kalam/archive/](docs/kalam/archive/) | The original kalam-engine docs, kept for the record. **Superseded.** |

**Chapbook's (inherited, still accurate until the strip):**

| | |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crate boundaries and why they fall where they do |
| [docs/LOCATORS.md](docs/LOCATORS.md) | Why reading positions are layered and versioned |
| [docs/SHELLS.md](docs/SHELLS.md) | Driving the engine from a UI (what Kalam's adapter will do) |
| [docs/STABILITY.md](docs/STABILITY.md) | Which crates are stable API and which are internals |
| [docs/PLATFORM.md](docs/PLATFORM.md) | Chapbook's multi-platform ambitions — most of this is what we remove |
| [CONTRIBUTING.md](CONTRIBUTING.md) | The verification gate and the invariants. Still applies. |

## License

kalam-engine as a whole is distributed under the GPL-3.0-or-later, in line
with Kalam. The code inherited from Chapbook remains available under its
original MIT OR Apache-2.0 terms ([LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE)), which GPL-3 is compatible with.
[NOTICE](NOTICE) lists third-party licenses a built binary carries.
