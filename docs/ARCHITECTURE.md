# chapbook architecture

Core principle: **no webview, no full HTML5 browser.** Chapbook implements the
EPUB 3 standards — XHTML content documents and the EPUB 3 CSS profile (CSS 2.1
plus selected CSS3 modules) — with a purpose-built, pagination-first pipeline.

## Pipeline

```
.epub ──chapbook-epub──▶ XHTML bytes + CSS + resources
      ──chapbook-dom───▶ arena Document (html5ever parse)
      ──chapbook-style─▶ ComputedValues per element (stylo cascade)
      ──chapbook-layout▶ ChapterLayout { pages, anchors, char_map }  (cosmic-text)
      ──chapbook-paint─▶ DisplayList per page
      ──render backend─▶ pixels (tiny-skia CPU today; e-ink/GPU later)
```

Everything above the render backend works in CSS px and has no GPU or vsync
assumptions; hidpi scaling happens only inside a backend.

## Format extensibility (EPUB-first, not EPUB-only)

Chapbook's focus is the reflowable-EPUB pipeline above, but the seams are
placed so an image-per-page format (CBZ) can join later without a redesign:

- **Book model in core.** `BookMetadata`/`SpineItem`/`TocEntry`/`Resource`
  and the `Publication` trait live in `chapbook-core`. The library, reader
  session, and CLI program against that surface — and import it from core,
  never via an EPUB re-export, so `grep chapbook_epub` stays an honest
  coupling map. Four producers exist: `chapbook-epub` (rbook), `chapbook-cbz`
  (each archive image = one spine item, natural-sorted, media type guessed
  from extension — no manifest), `chapbook_opds::StreamedComic` (OPDS-PSE
  page streaming, one HTTP fetch per page through a 0-based `{pageNumber}`
  template, disk-cached), and `chapbook-pdf` (pages rasterized at 2×
  via hayro — pure Rust, CPU-only; encrypted PDFs rejected; MSRV floor is
  hayro's 1.92 — plus a text layer: a recording `Device` captures per-glyph
  Unicode and geometry during interpretation, reading order reconstructed
  by baseline clustering, surfacing as `HiddenText` fragments so selection
  works on PDF pages exactly as on reflowed text). The streamed producer is what the
  `Publication::unit_bytes` blocking contract was written for: it may take
  seconds and fail with `ChapbookError::Network`, and UI code must call it
  off the UI thread.
- **Page model in paint.** `chapbook-paint` owns `Page`/`Fragment`/
  `DisplayList`; `chapbook-layout` *produces* into it. A comic page becomes a
  single image-fragment `Page` (`chapbook_paint::image_page`) — no DOM, no
  stylo, no cosmic-text shaping — and the render backends and viewers never
  know the difference. Fragments reference their source via an opaque `u64`
  tag, never a DOM type.
- **Per-format progression units.** Locators count a format-defined unit:
  chars of locator text for reflowable EPUB; pages for image formats
  (`char_offset = 0`, empty quote layer — the resolve chain already degrades
  past it to the fraction layers). See `chapbook_core::locator` docs.
- **OPDS is already format-neutral** — acquisition links carry media types
  (`application/vnd.comicbook+zip` works the same as EPUB's).

Everything else — dom, style, layout — is deliberately text-specific and
stays that way.

## Crate boundaries

- **chapbook-core** — geometry, `PageMetrics`, `ReadingSettings`, the
  format-neutral book model (`Publication`, `BookMetadata`, `SpineItem`,
  `TocEntry`), and `Locator { spine_index, char_offset }` plus the layered,
  versioned persistence record (`LayeredLocator`: quote context, spine
  fraction, whole-book progression) and its resolve chain — see
  `docs/LOCATORS.md`. `char_offset` indexes the *raw locator text*
  (`chapbook_dom::locator_text`, versioned by `LOCATOR_VERSION`), not the
  collapsed display text. No heavy deps.
- **chapbook-epub** — wraps `rbook` for OCF/OPF/spine/TOC; adds relative
  resource resolution, fixed-layout detection (rejected), font
  de-obfuscation (M5). The wrapper boundary means rbook gaps can be patched
  with `zip` + `quick-xml` per-field without touching consumers.
- **chapbook-dom** — slotmap arena `Document`; the copyable `DomNode<'a>`
  handle carries all stylo trait impls (`TNode`/`TDocument`/`TElement`/
  `selectors::Element`), structured after blitz-dom's proven binding. The
  handle **must stay pointer-sized** (stylo's style sharing cache statically
  asserts it), so `Document` heap-boxes a `DocumentInner` with a stable
  address, every node carries a sealed back-pointer + self-id, and
  `DomNode<'a>` is a `&'a Node` newtype. Documents are static after parse:
  no incremental restyle, no snapshots, no shadow DOM, no animations, no
  scripting — which deletes most of stylo's invalidation surface. Parsing is
  lenient html5ever by default (real EPUBs contain HTML-isms); a
  `strict-xml` feature runs xml5ever on the same tree
  builder.
- **chapbook-style** — owns the `Stylist` + media `Device` ("screen"), embeds
  the UA stylesheet (`ua.css` — the profile boundary: what is not in the
  EPUB 3 CSS profile gets no UA support), registers author sheets from the
  chapter and user-origin override sheets (reader settings, themes), runs the
  restyle traversal, exposes `@font-face` rules for fontdb registration, and
  provides a cosmic-text-backed `FontMetricsProvider` (ex/ch units).
  Themes (`chapbook_core::Theme`): `Light` is the identity theme; `Sepia`
  recolors the defaults at user origin (publisher colors win); `Dark`
  forces text/background colors with `!important` for night-mode
  readability and flips the device's `prefers-color-scheme`. The page
  ground is the display list's first op, chosen by the caller from the
  theme.
- **chapbook-layout** — the differentiator. Box tree per CSS 2.1 §9.2
  (anonymous blocks, `::before`/`::after`), block flow, each inline formatting
  context laid out as one `cosmic_text::Buffer` (per-span `Attrs` from
  `ComputedValues`, `metadata` = span index for DOM mapping and locators).
  A streaming page cursor applies CSS fragmentation: forced
  `break-before/after: page` (+ legacy `page-break-*` aliases),
  `break-inside: avoid` (retry on a fresh page, else break anyway),
  widows/orphans (default 2/2), margins discarded at page boundaries,
  oversized monolithic boxes sliced graphically (first/last-slice flags gate
  border painting).
  **Fragmentation sidecar:** servo-mode stylo does not implement the
  fragmentation properties (`break-*`, `page-break-*`, `widows`, `orphans`
  are Gecko-only — they land in its counted-unknown bucket), so
  chapbook-layout runs its own mini-cascade for just those declarations:
  cssparser parses them out of the same sheets, and their selectors match
  through our existing `selectors::Element` impl with standard
  specificity/order rules. `hyphens` (also Gecko-only, inherited) rides in
  the same sidecar.
  **Floats:** `float: left/right` on images (intrinsic size) and on
  non-replaced blocks with an explicit CSS width (laid out in a detached
  sub-paginator, fragments translated into place) places the float against
  a content edge and shortens the line boxes of following inline content
  beside it (the IFC is split at the float's bottom edge and re-shaped —
  the same machinery as first-line indents, which also apply beside
  floats); `clear` works on any block; floats never cross a page
  boundary.
  **Hyphenation:** `hyphens: auto` inserts soft hyphens at embedded en-US
  Knuth-Liang dictionary points before shaping — cosmic-text's line breaker
  already treats U+00AD as a break opportunity and its shaper renders it
  invisible — and lines that break at one get a visible hyphen glyph
  appended, with justified lines re-tightened over their spaces.
  Output: `ChapterLayout` — chapbook-paint `Page`s plus the
  text-specific side tables (`anchors: id→page`, `char_map:
  char_offset→page`, fragment-tag→DOM-node mapping). One spine item = one
  layout run, cached by `(spine_idx, PageMetrics, settings_hash, css_hash)`;
  whole-book page numbers are computed lazily chapter-by-chapter.
  `style_to_attrs.rs` documents the supported `ComputedValues → Attrs` subset
  and its fallbacks.
- **chapbook-paint** — owns the format-neutral page model (`Page`/`Fragment`,
  fragments tagged with an opaque producer-defined `u64`, never a DOM type)
  and the dumb display ops: `FillRect`, `Border`,
  `GlyphRun { fontdb::ID, glyphs }` (no re-shaping at paint time), `Image`,
  `DecorationLine`, `PushClip`/`PopClip`. Layout produces into it; image
  formats will too.
- **chapbook-render-tinyskia** — swash glyph raster cache, `image`-decoded
  resources, scale applied here.
- **chapbook-opds** — blocking `ureq` + rustls (no async runtime). OPDS 1.2
  Atom as the canonical dialect, parsed at the XML level with namespace-aware
  `quick-xml` (NOT `atom_syndication`/`feed-rs`: both silently drop the
  foreign-namespace link attributes that facets and OPDS-PSE page streaming
  live in); OPDS 2.0 JSON via serde as a secondary parser. Pagination
  (`next`/`previous` + OpenSearch totals), search, facets, acquisition
  download (temp file + atomic rename; no Range resume assumed), HTTP Basic
  at any point in a flow plus OPDS Authentication Document login. Full
  requirements: `docs/OPDS-INTEROP.md`; wire-format fixtures:
  `fixtures/opds/`.
- **chapbook-library** — rusqlite (bundled, WAL): books/authors, positions,
  annotations, opds_sources. Positions are `Locator`s and survive relayout
  via the char_map.
- **chapbook-reader** — the shared reading session, extracted so viewer
  shells stay thin: source dispatch (`.epub`/`.cbz`/OPDS URL), per-unit
  layout+image caches (text units run the full pipeline; comic units
  fabricate a one-page layout around an image fragment), navigation,
  font/theme settings, selection (hit-testing via per-glyph locator offsets
  on `chapbook-paint` fragments; highlight painted under the text), and
  layered-locator persistence. Comics persist page-unit progression.
- **chapbook-viewer** / **chapbook-viewer-gtk** — winit+softbuffer and GTK4
  shells over `chapbook-reader::Session`; each translates input events and
  blits the session's rasterized page, nothing more.
- **tools/chapbook-cli** — `meta|toc|text|styles|layout|render|opds|lib`;
  each subcommand ships with its milestone and generates the snapshot inputs
  for that milestone's golden tests.

## Version policy

The stylo lockstep set (`stylo`, `stylo_traits`, `stylo_atoms`,
`stylo_static_prefs`, `stylo_dom`, `selectors`, `cssparser`) is pinned with
`=` in the workspace and upgraded all-at-once as a deliberate task, using
Blitz's corresponding upgrade diff as the migration guide. html5ever/
markup5ever/xml5ever must match the markup5ever minor that stylo's selector
types use. MSRV 1.92 (hayro's floor; stylo 0.20 needs 1.89), stable toolchain — no nightly.

## Explicitly out of scope

Fixed-layout EPUB (detected, rejected with a clear error), JavaScript
(spec-permitted omission for reading systems), MathML, vertical writing
modes, shrink-to-fit floated blocks (floated images and floated blocks
with an explicit width lay out for real; the rest stay in flow), CSS
counters in generated content, absolute positioning (treated as static),
media overlays, DRM.

## Milestones

M0 scaffold/CI → M1 EPUB+text extraction → M2 stylo cascade → M3 paginated
layout → M4 pixels (renderer + viewer) → M5 images/@font-face → M6 OPDS →
M7 library → M8 stretch (tables, CFI, themes, hyphenation, e-ink backend).
