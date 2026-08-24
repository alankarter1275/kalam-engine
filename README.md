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

| Crate | Role |
|---|---|
| `chapbook-core` | Shared primitives: geometry, page metrics, locators, errors |
| `chapbook-epub` | EPUB container/package/spine/TOC reading |
| `chapbook-dom` | Arena DOM for XHTML content documents + stylo trait bindings |
| `chapbook-style` | Cascade driver: stylist, UA sheet, media device |
| `chapbook-layout` | Pagination-first block + inline layout via cosmic-text |
| `chapbook-paint` | Format-neutral page model + paint-neutral display list |
| `chapbook-render-tinyskia` | CPU rasterization backend |
| `chapbook-opds` | OPDS 1.2/2.0 catalog client + OPDS-PSE streamed comics |
| `chapbook-cbz` | CBZ comic-book archive reading |
| `chapbook-pdf` | PDF reading, rasterized via hayro (pure Rust) |
| `chapbook-library` | Local bookshelf: metadata, positions, annotations (SQLite) |
| `chapbook-reader` | Shared reading session (open/layout/navigate/select/persist) |
| `chapbook-viewer` | Minimal reference viewer (winit + softbuffer) |
| `chapbook-viewer-gtk` | GTK4 reference viewer |
| `tools/chapbook-cli` | Dev/test CLI exercising each pipeline stage |

## Status

The core pipeline works end-to-end: open (or download via OPDS) an EPUB,
cascade its styles through stylo, paginate with cosmic-text, render pages
with tiny-skia, and read it in the reference viewer with positions that
survive relayout, font-size changes, and even replaced editions (see
`docs/LOCATORS.md`) — and exchange them as EPUB CFIs (`chapbook cfi`).
Embedded fonts (including obfuscated ones), images, and text decorations
render, with light/sepia/dark themes (sepia recolors defaults; dark forces
readability), and press-drag text selection that copies to the clipboard
(`c`) or becomes a stored highlight (`h`), kept in the library as layered
locators so it re-anchors like a position. Comics and PDFs work too: local
CBZ archives, OPDS-PSE page streams, and PDFs (rasterized with the
pure-Rust hayro engine, with a text layer so selection works there too)
read in the same viewers with page-unit positions — their pages load and
decode on a background thread. `chapbook --help` for the dev CLI; `cargo
run -p chapbook-viewer -- <book.epub|comic.cbz|doc.pdf|opds-url>` (winit)
or `-p chapbook-viewer-gtk` (GTK4) to read.

See `docs/ARCHITECTURE.md` for the design. Explicitly out of scope:
fixed-layout EPUB, JavaScript/scripted content, MathML, vertical writing
modes, media overlays, DRM. Floated images and width-bearing asides wrap
text for real; hyphenation is dictionary-based (en-US); remaining niche
gaps: CSS counters in generated content, shrink-to-fit floats, `ex`/`ch`
units resolved by approximation.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
