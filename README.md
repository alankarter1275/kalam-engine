# chapbook

Core components for a lightweight ereader, in Rust.

No webview. No full HTML5 browser. Chapbook implements just the EPUB 3
standards — XHTML content documents and the EPUB 3 CSS profile — on top of
strong upstream crates: [stylo] (the CSS engine behind Firefox and Servo) for
the cascade, [cosmic-text] for shaping and line layout, [tiny-skia] for CPU
rasterization, and [rbook] for EPUB container handling. The layout engine is
pagination-first: pages, break rules, and widows/orphans are the core model,
not an afterthought bolted onto a scrolling browser.

[stylo]: https://crates.io/crates/stylo
[cosmic-text]: https://crates.io/crates/cosmic-text
[tiny-skia]: https://crates.io/crates/tiny-skia
[rbook]: https://crates.io/crates/rbook

## Workspace

The **tier** says how much the API is expected to hold still —
[docs/STABILITY.md](docs/STABILITY.md) explains where the lines fall and
why. A shell depends on `chapbook-reader` alone; it re-exports the rest.

| Crate | Role | Tier |
|---|---|---|
| `chapbook-core` | Shared primitives: geometry, page metrics, locators, errors | Contract |
| `chapbook-epub` | EPUB container/package/spine/TOC reading | Producer |
| `chapbook-layout` | Arena DOM + stylo trait bindings (`::dom`), cascade driver (`::cascade`), pagination-first block + inline layout via cosmic-text | Internal |
| `chapbook-paint` | Format-neutral page model + paint-neutral display list | Contract |
| `chapbook-render-tinyskia` | CPU rasterization backend | Backend |
| `chapbook-render-vello` | GPU rasterization backend (vello + wgpu) | Backend |
| `chapbook-panel-fbdev` | Linux framebuffer panel backend (`/dev/fb0`) | Backend |
| `opds-client` | OPDS 1.2/2.0 catalog client, bring-your-own-HTTP (no chapbook dependency) | API |
| `chapbook-opds` | Binds `opds-client` to chapbook: OPDS-PSE streamed comics as `Publication`s | Producer |
| `chapbook-cbz` | CBZ comic-book archive reading | Producer |
| `chapbook-pdf` | PDF reading, rasterized via hayro (pure Rust) | Producer |
| `chapbook-library` | Local bookshelf: metadata, positions, annotations (SQLite) | API |
| `chapbook-reader` | Shared reading session (open/layout/navigate/select/persist) | API |
| `chapbook-viewer` | Minimal reference viewer (winit + softbuffer) | Not a library |
| `chapbook-viewer-gtk` | GTK4 reference viewer (Linux only) | Not a library |
| `tools/chapbook-cli` | Dev/test CLI exercising each pipeline stage | Not a library |
| `chapbook-jni` | Android JNI binding — a spike, paired with `android/` | Spike |

## Android

`android/` is a Gradle project with an AAR library module and a demo app,
over `chapbook-jni`. It builds, draws a book, opens one from a `content://`
URI with no path and no extension, and passes the conformance harness on a
device — all five rungs of `docs/FFI.md`'s ladder. It is a spike whose purpose is to shape the portability
boundary rather than to be built on — read [docs/FFI.md](docs/FFI.md) before
touching it, including the prerequisites, which are not obvious.

```sh
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/<version>
./android/build-jni.sh release      # cargo-ndk into jniLibs, then check linkage
cd android && ./gradlew :demo:assembleDebug
```

## Getting started

Rust stable (MSRV 1.92; `rust-toolchain.toml` pins the channel). One system
library, and only for the GTK viewer:

```sh
sudo apt install libgtk-4-dev          # Debian/Ubuntu; gtk4-sys wants gtk4.pc
```

Everything else builds from source — SQLite is bundled, fontconfig is a
pure-Rust parser — so skipping `chapbook-viewer-gtk` needs no system
packages at all. Off Linux you skip it whether you meant to or not: `gtk4`
is a target-gated dependency, the crate compiles to a stub `main`, and
`cargo test --workspace` runs on macOS and Windows without GTK or
pkg-config installed.

```sh
cargo run -p chapbook-viewer -- <book.epub|comic.cbz|doc.pdf|opds-url>
cargo run -p chapbook-viewer -- --gpu <book.epub>   # vello + wgpu
cargo run -p chapbook-viewer-gtk -- <book.epub>     # GTK4, Linux only
cargo run -p chapbook-cli -- --help                 # the `chapbook` dev CLI
```

The checks CI runs, in order:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets   # CI sets RUSTFLAGS=-D warnings
cargo test --workspace
```

## Status

The core pipeline works end-to-end: open (or download via OPDS) an EPUB,
cascade its styles through stylo, paginate with cosmic-text, render pages
with tiny-skia, and read it in the reference viewer with positions that
survive relayout, font-size changes, and even replaced editions (see
[docs/LOCATORS.md](docs/LOCATORS.md)) — and exchange them as EPUB CFIs
(`chapbook cfi`). Embedded fonts (including obfuscated ones), images, and
text decorations render, with light/sepia/dark themes (sepia recolors
defaults; dark forces readability), and press-drag text selection that
copies to the clipboard (`c`) or becomes a stored highlight (`h`), kept in
the library as layered locators so it re-anchors like a position.

Comics and PDFs work too: local CBZ archives (with `ComicInfo.xml` for
metadata and bookmarks when present), OPDS-PSE page streams, and PDFs
(rasterized with the pure-Rust hayro engine, with a text layer so selection
works there too, and the document outline as a table of contents). They
read in the same viewers with page-unit positions — their pages load and
decode on a background thread.

Pages rasterize on the CPU with tiny-skia, or on the GPU through vello and
wgpu — the same session and the same display list, a different backend
consuming it. Panels — screens that are asked to change rather than
presented to — go through a `Panel` trait with a driver that picks the
update class, avoids writing under an in-flight refresh, and rations the
ghosting flash; `chapbook-panel-fbdev` implements it over a plain Linux
framebuffer (`--example probe` to see what a device reports, `--example
show` to read a book on it).

Explicitly out of scope:
fixed-layout EPUB, JavaScript/scripted content, MathML, vertical writing
modes, media overlays, DRM. Floated images and width-bearing asides wrap
text for real; hyphenation is dictionary-based (en-US); remaining niche
gaps: CSS counters in generated content, shrink-to-fit floats, `ex`/`ch`
units resolved by approximation.

## Docs

| | |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | The design: crate boundaries and why they fall where they do |
| [docs/SHELLS.md](docs/SHELLS.md) | Writing a shell against `Session`: the loop, the loader rule, and the conformance harness |
| [docs/STABILITY.md](docs/STABILITY.md) | Which crates carry semver discipline, which are internals, and why |
| [docs/FFI.md](docs/FFI.md) | The portability boundary: bindings, typed sources, input and the Android spike |
| [docs/LOCATORS.md](docs/LOCATORS.md) | Reading positions that survive relayout, and EPUB CFI |
| [docs/OPDS-INTEROP.md](docs/OPDS-INTEROP.md) | What the OPDS client must interoperate with, and how it was verified |
| [docs/PLATFORM.md](docs/PLATFORM.md) | Porting to real devices: panels, e-ink, cross-compilation, what is proven and what is not |
| [CONTRIBUTING.md](CONTRIBUTING.md) | The verification gate, the invariants, and the things that fail quietly |
| [NOTICE](NOTICE) | Third-party licences a binary carries |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Dependencies are not uniformly either — the CSS engine is MPL-2.0, the
rasterizer BSD-3-Clause, the EPUB parser Apache-2.0 only — so distributing
a **binary** carries terms beyond those two. [NOTICE](NOTICE) says which,
which builds they apply to, and what each obligates; `deny.toml` is the
allow-list CI enforces so the set cannot widen unnoticed.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
