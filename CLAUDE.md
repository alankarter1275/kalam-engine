# chapbook

Lightweight ereader core in Rust. No webview, no browser: just EPUB 3 (XHTML +
the EPUB 3 CSS profile) plus CBZ/PDF, on stylo + cosmic-text + tiny-skia.
Design lives in `docs/ARCHITECTURE.md`, locators in `docs/LOCATORS.md`, OPDS in
`docs/OPDS-INTEROP.md`. Read the relevant doc before changing that subsystem.

## Verify

Every change must pass the same gate CI runs, in this order:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets   # CI sets RUSTFLAGS=-D warnings
cargo test --workspace
```

Render goldens are byte-exact; regenerate deliberately with
`UPDATE_RENDER_GOLDENS=1 cargo test -p ...` and eyeball the diff before
committing. Layout goldens use `insta`.

## Invariants

- **stylo lockstep pins.** `stylo`, `stylo_traits`, `stylo_atoms`,
  `stylo_static_prefs`, `stylo_dom`, `selectors`, `cssparser` are pinned with
  `=` and upgrade all-at-once as a deliberate task (Blitz's diff is the
  migration guide). html5ever/markup5ever/xml5ever must match the markup5ever
  minor stylo's selector types use. Never bump one alone.
- **MSRV 1.92, stable toolchain.** No nightly features.
- **No async runtime.** OPDS is blocking `ureq`; the loader is a plain thread.
- **Core stays GPU-assumption-free** for later e-ink ports.
- **`DomNode` must stay pointer-sized** — stylo's style sharing cache statically
  asserts it.
- **Never compact the spine.** Dangling idrefs keep their slot with an empty
  href; indices are locator identity.
- **Viewers never call `unit_bytes`/`resource` on the UI thread** — loader
  thread only (`chapbook-reader/src/loader.rs`).
- Model types come from `chapbook_core` only; fragments reference sources by
  opaque `u64` tags, never DOM types.
- servo-mode stylo lacks the CSS fragmentation properties, so `break-*`,
  `page-break-*`, `widows`, `orphans`, and `hyphens` run through
  chapbook-layout's own sidecar cascade. Add fragmentation properties there.

## Out of scope

Fixed-layout EPUB, JavaScript, MathML, vertical writing modes, absolute
positioning, media overlays, DRM.

## Gotchas

- Python/`sed` string replacement on rustfmt-reflowed code silently no-ops.
  Grep to confirm an edit landed.
- cosmic-text 0.19: `Buffer` methods don't take `FontSystem` except `new` and
  `shape_until_scroll`; clamp `line_height` above zero (real books ship
  `line-height: 0`).
- Tests that touch the library set process-global env — they serialize behind
  `ENV_LOCK` and use per-test dirs. Follow the existing `open_isolated` /
  `reopen_isolated` helpers.
