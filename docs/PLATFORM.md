# chapbook as an ereader platform

Chapbook started as an ereader and turned into the engine an ereader is built
on. This document takes that seriously: what "platform" has to mean here, what
already works, and what stands between the current workspace and someone
building a Kobo app, an Android app, and a desktop app against the same core.

## The test

A platform is judged by what a downstream app can **substitute without
forking**. Five axes matter:

| Axis | Substitutions that must be cheap |
|---|---|
| Display | CPU raster, GPU, e-ink (greyscale, partial refresh), platform canvas, headless/export |
| Input | Touch, mouse, physical page-turn buttons, keyboard, gestures |
| Host | Rust binary, Android (JNI), iOS (Swift), WASM, embedded Linux |
| Content | EPUB, comics, PDF, remote streams — and formats not yet supported |
| Backend | Local library, remote catalogs, position/annotation sync services |

Everything below is organized by which substitution is currently blocked.

## What already works

This isn't aspirational; the seams have been tested twice by accident.

- **The shell/session split holds.** `chapbook-viewer` (winit) is ~310 lines
  and `chapbook-viewer-gtk` is ~220, against ~1,200 lines of
  `chapbook-reader`. Two shells, two windowing stacks, no duplicated reading
  logic — and the split absorbed clipboard copy and highlighting without
  either shell growing reading logic of its own. A third shell is already
  cheap.
- **`Publication` predicted its own future.** The format-neutral book model
  went in ahead of need; CBZ and then PDF — a format with a completely
  different rasterization story — landed through it without disturbing core,
  layout, or the locator design.
- **The paint/layout inversion is load-bearing.** It was done so non-text
  producers could emit pages; comic units now fabricate a one-page
  `ChapterLayout` around a scaled image fragment, so navigation, the char
  map, and position persistence stay one code path.
- **Locators generalize.** Per-format progression units (chars for
  reflowable, pages for image books) absorbed comics with no schema change.
- **The layered record earns its complexity.** Highlights capture both
  endpoints as layered locators and re-anchor through the same chain
  positions use, so a stored highlight survives relayout, font-size changes,
  and a replaced edition. The design was speculative until something other
  than the reading position used it.
- **No native ball-and-chain.** Pure Rust, no webview, no MuPDF FFI. This is
  the precondition for every device target below; it is already paid for.

The gap list is therefore almost entirely **additive** — missing APIs and
unproven targets, not decisions to undo. The exceptions are called out.

## 1. The render seam (closed)

This was the gap with a clock on it, and it was paid at two in-tree shells
rather than at five across an FFI boundary.

`Session::frame()` returns the paint-neutral ops — the documented backend
contract — and `paint_resources()` hands back the font database its glyph
runs name faces in plus the image store its image ops key into, which is
everything a shell needs to rasterize for itself: a GPU backend, a platform
canvas, an e-ink panel, an exporter. A test reproduces `render()` through
that public API and asserts the pixels match, which is what makes it a seam
rather than an accessor. `render()` remains the convenience path.

A frame also carries its own provenance. `FrameIntent` — repaint,
selection, annotation, content arrived, page turn, unit change, reflow —
tells a panel whether it is looking at a full flash or a fast partial
refresh, and only the engine can know which. The intents are ordered by how
much of the page they disturb, so several changes before a frame collapse
to the strongest, and a backend that only distinguishes "small" from
"everything" can compare rather than match every variant. Damage is stated
where it beats repainting, which today means selection changes; `None`
means the whole page, always correct and sometimes pessimistic.

Panel colour is pipeline policy rather than per-shell improvisation:
`PixelFormat::Grey { levels, dither }` quantizes luminance to a panel's
2..=16 steps, diffusing the error Floyd–Steinberg when asked. Orientation
is a page metric: `PageMetrics::rotation` turns the output on its way to
the buffer without touching layout, and `panel_to_page` is its inverse for
input, so a rotated shell hands the session panel coordinates and the
session untwists them. Packing grey levels into a device's buffer layout
stays with the shell — only it knows the panel's word order.

One increment left: damage beyond selections. A page turn that only moves a
footer, or an image landing in a fixed rect, could both state their region
and don't.

## 2. The reading model above the page

The session can turn pages, change fonts, select, copy, and highlight. An
ereader app needs more, and most of what's left is not expressible through
the API, so every shell would reinvent it differently.

**Navigation (landed).** `goto`, `goto_anchor`, `goto_toc`, `link_at`,
`follow_link`, and a capped back-stack: jumps remember where they came from,
ordinary page turns don't, so a footnote returns to the sentence that sent
you. Fragments resolve through the `anchors: id→page` map layout already
computed, and a fragment the unit turns out not to have lands at its start
rather than failing. Links ride the locator offset space rather than the
fragment tree — paint carries no DOM types — which also means a link hit
test survives relayout for free, and that a tap in the margin beside a link
is not a tap on the link.

**Annotations (half-collected).** The write path is joined:
`Session::add_highlight` captures both selection endpoints as layered
locators, `highlights(spine)` re-anchors them per unit, and stored ranges
paint under the text in a color of their own. What remains is the rest of
the model — hit-testing a tap against an existing highlight, so a reader can
select, recolor, or delete one by touching it rather than by id; notes and
bookmarks (the schema has `AnnotationKind::Note` and `Bookmark`, and only
`Highlight` has a caller); a whole-book annotation list; and export.

**Search (missing, and nearly free).** In-book full-text search over locator
text is char-offset indexed already, and results are natively locators.
Library-level search across books is a second, separate want.

**Settings (harness-shaped).** `cycle_theme()` and `adjust_font(delta)` are
UI verbs, not a model, and they are the only two the session exposes:
`ReadingSettings` also carries `line_height`, `justify`, and
`publisher_styles`, which no shell can reach. There is no persistence —
font size does not survive a restart — no per-book overrides, and no
font-family selection at all. (Margins are fine: they live in `PageMetrics`,
supplied by the shell.) Wanted: settings in / settings out, persisted
globally with per-book overrides.

**Session lifecycle.** `open(source: &str)` sniffs a string. A platform wants
typed sources plus injectable I/O — Android content URIs, iOS
security-scoped bookmarks, in-memory books, and encrypted stores all fail the
string-path assumption. An observer/event model (layout invalidated, position
changed, book finished) should replace polling where shells need to react.

## 3. The portability layer

**FFI is the gate.** "Different devices" mostly means different host
languages, and there is no C API or bindings layer, so Android/Kotlin,
iOS/Swift, and WASM apps cannot be built at all. Designing this boundary is
also the forcing function for §2's API cleanups — a string-sniffing
constructor and imperative UI verbs do not survive the trip.

**Input abstraction.** Shells hand-translate events today. A platform wants a
gesture/intent model (tap zones, swipe, long-press select, pinch-zoom for
comics and PDF) and a key-mapping model — remembering that Kobo and
PocketBook have *physical page-turn buttons*, which desktop shells never
exercise.

**Lifecycle and power.** No suspend/resume, no "save state now, you are being
killed." Mandatory on mobile and e-ink.

**Cross-compilation is unproven.** No armv7/aarch64 device targets in CI.
These devices ship old glibc; binary size becomes a real budget, and font
provisioning (fontdb with no usable system fonts) needs a bundled-font
policy. Nothing about the architecture prevents this — but nobody has
demonstrated it, and unproven is a gap.

## 4. Features shells cannot add from outside

These belong to the engine by construction; a downstream app cannot implement
them itself. Hyphenation is the proof the category is real: dictionary-based
`hyphens: auto` had to go in the line breaker, and did.

- **TTS** — needs text extraction plus word-level highlight timing.
- **Dictionary lookup** — word-boundary selection semantics.
- **Bidi correctness** for Arabic/Hebrew — cosmic-text can do it; confirm it
  is exercised and tested rather than assumed.
- **Accessibility** — no a11y tree export, no screen-reader path. Also a
  legal requirement in some markets.

## 5. Breadth and sync

Formats today: EPUB, CBZ, PDF, and OPDS-PSE streams. App builders will ask
for MOBI/AZW3 (large existing libraries), CBR (blocked on pure-Rust RAR5
extraction, which does not exist), and FB2.

**Sync compounds at platform scale.** There is no reading-position sync
client and no annotation sync: comics resume via `pse:lastRead`, but text
positions never leave the device. Every app built on chapbook would otherwise
implement this separately, so it belongs in the platform. Two targets, both
cheap given the existing locator design:

- **OPDS Progression 1.0** — `progression` maps from `book_progression`;
  `references` from the quote layer as text fragments. The layered record was
  designed to serialize directly into this.
- **kosync** — four endpoints, the de-facto self-hosted standard; grants
  interop with third-party servers immediately. Its identity model is weak
  (file hash), so treat percentage as the reliable field and let the layered
  locator degrade over it.

For annotation interchange, serialize the W3C EPUB Annotations 1.0 /
Readium profile (import/export); no sync transport is standardized yet.

## 6. Platform hygiene

The unglamorous half, and the real distance between "modular codebase" and
"platform someone else can build on":

- **API stability policy** across fourteen crates: which are public surface
  (`core`, `reader`, `library`, `opds`, `paint`) versus internal
  (`dom`, `style`, `layout`)? Only the former need semver discipline.
- **Feature flags.** A minimal device build should exclude PDF, GTK, and
  OPDS. Only `chapbook-dom` has features today (`strict-xml`).
- **Docs for shell authors** — the missing genre. `ARCHITECTURE.md` explains
  the pipeline; nothing explains how to write a shell.
- **A shell conformance harness** — given a `Session`, assert a shell drives
  it correctly (page turns, resize/relayout, position save/restore,
  selection). Same spirit as the golden tests, applied to the seam.
- **A reference minimal shell** — smaller than the winit viewer, existing to
  be copied.

## Priorities

1. **Navigation, search, and the rest of annotations (§2)** — table stakes;
   the substrate exists and highlights proved it works; prevents divergent
   reinvention across shells.
2. **FFI boundary (§3)** — gate on the largest device markets; forces the
   session API into SDK shape.
3. **Sync clients (§5)** — belongs to the platform, not to each app.
4. **Hygiene (§6)** — continuous, never urgent, decides whether any of this
   is usable by anyone else.

The render seam (§1) came first and is closed; what remains of it — damage
for intents other than selection — is an increment, not a gate.

## Ceilings to decide deliberately

These are choices, not oversights — but a platform should make them
explicitly rather than by default:

- **Vertical writing and RTL flow** are currently out of scope. This is the
  one gap that is genuinely *hard* rather than merely unbuilt: it reaches
  into the layout crate, the largest in the workspace. It is also a hard
  ceiling for any CJK or manga-oriented app built on chapbook. Decide
  whether that ceiling is acceptable.
- **Fixed-layout EPUB** stays rejected. Reasonable for one app; a bigger
  deal for a platform, since comics-as-FXL-EPUB is common in the wild and
  the profile in practice is narrow (one full-bleed image per page). A
  special-cased image-book path could accept exactly that profile while
  still rejecting general FXL.
- **Scope discipline.** Velocity has gone into breadth — another format,
  another surface — because that work is more fun than hygiene. Breadth
  without §6 produces an excellent personal ereader whose code merely looks
  modular.
