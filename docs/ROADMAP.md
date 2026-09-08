# kalam-engine Implementation Roadmap

This roadmap tracks the step-by-step development of the `kalam-engine` library crate.

---

## Phase 1: Core Foundation & Data Types
- [ ] Define public API traits (`Engine`, `Document`, `Page`, `Locator`).
- [ ] Implement `LayeredLocator` data structures with `serde` serialization.
- [ ] Implement EPUB container parsing (`zip` + `quick-xml` for OPF manifest/spine).

## Phase 2: Normalization Pipeline (`lol_html`)
- [ ] Implement `lol_html` rewriter to strip publisher CSS and inline styles.
- [ ] Build HTML element to semantic token converter.

## Phase 3: Text Layout & Shaping (`cosmic-text`)
- [ ] Set up `cosmic-text` `FontSystem`, `SwashCache`, and `Buffer`.
- [ ] Implement line breaking and word wrapping math for dynamic viewports.
- [ ] Build `DisplayList` generator for page chunking.

## Phase 4: GTK4 Painter & Demo App
- [ ] Build `tiny-skia` to GTK `DrawingArea` render pipeline.
- [ ] Implement `examples/demo.rs` interactive GTK4 EPUB viewer app.
- [ ] Implement keyboard navigation (Left/Right arrows for instant page turns).

## Phase 5: Text Selection & Dictionary Hitting
- [ ] Implement `cosmic_text::Buffer::hit` word detection under cursor click.
- [ ] Implement text selection drag and highlight rect generation.
- [ ] Wire dictionary word extraction hooks for Kalam integration.

## Phase 6: Integration into Kalam (`calibre-alt`)
- [ ] Add `kalam-engine` as a path dependency in Kalam's `Cargo.toml`.
- [ ] Swap out `webkit6::WebView` in `src/pages/reader/` with `kalam_engine::ReaderWidget`.
