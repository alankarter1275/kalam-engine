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
| `chapbook-opds` | OPDS 1.2/2.0 catalog client |
| `chapbook-library` | Local bookshelf: metadata, positions, annotations (SQLite) |
| `chapbook-viewer` | Minimal reference viewer (winit + softbuffer) |
| `tools/chapbook-cli` | Dev/test CLI exercising each pipeline stage |

## Status

The core pipeline works end-to-end: open (or download via OPDS) an EPUB,
cascade its styles through stylo, paginate with cosmic-text, render pages
with tiny-skia, and read it in the reference viewer with positions that
survive relayout, font-size changes, and even replaced editions (see
`docs/LOCATORS.md`). Embedded fonts (including obfuscated ones), images,
and text decorations render. `chapbook --help` for the dev CLI;
`cargo run -p chapbook-viewer -- <book.epub>` to read.

See `docs/ARCHITECTURE.md` for the design. Explicitly out of scope:
fixed-layout EPUB, JavaScript/scripted content, MathML, vertical writing
modes, media overlays, DRM. Known layout gaps: table rowspan, floated
non-image blocks (floated images wrap text for real; hyphenation is
dictionary-based, en-US).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
