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

## 1. The render seam (highest priority — and it has a clock)

`Session::display_list()` now returns the paint-neutral ops, and
`paint_resources()` hands back the font database its glyph runs name faces
in plus the image store its image ops key into — enough for a shell to
reproduce `render()` without tiny-skia, which a test asserts pixel-for-pixel.
`render()` remains the convenience path. That unblocks GPU backends,
platform canvases, and export harnesses.

Frames carry their own provenance too: `Session::frame()` returns the ops
with a `FrameIntent` — repaint, selection, annotation, content arrived,
page turn, unit change, reflow — and, for a selection change, the damaged
region. The intents are ordered, so when several things happen before a
frame is taken the strongest one describes it, and a backend that only
understands "small" versus "everything" can compare rather than match.
Damage is stated only where it is cheaper than repainting; `None` means the
whole page, which is always correct and sometimes pessimistic. Extending it
past selections (a page turn that only moves a footer, an image landing in
a fixed rect) is the obvious next increment.

Still required:

- **Pixel format policy.** Panels are 16-level grey or 1-bit; greyscale
  conversion and dithering belong in the pipeline, not in each shell.
- **Rotation/orientation** as a first-class metric rather than a shell
  concern.

**Why now:** there are two shells and both are in-tree, so reshaping this
costs an afternoon. At five shells — one of them across an FFI boundary in
another language — it costs a migration. This is the one gap whose price is
actively rising.

## 2. The reading model above the page

The session can turn pages, change fonts, select, copy, and highlight. An
ereader app needs more, and most of what's left is not expressible through
the API, so every shell would reinvent it differently.

**Navigation (missing entirely).** No goto-locator, no TOC jump, no internal
link following, no fragment-anchor resolution, no back-stack for returning
from a footnote. `ChapterLayout` already computes `anchors: id→page`, so the
data exists and only the API doesn't. This is table stakes.

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

1. **Render seam (§1)** — rising cost, unlocks every device target.
2. **Navigation, search, and the rest of annotations (§2)** — table stakes;
   the substrate exists and highlights proved it works; prevents divergent
   reinvention across shells.
3. **FFI boundary (§3)** — gate on the largest device markets; forces the
   session API into SDK shape.
4. **Sync clients (§5)** — belongs to the platform, not to each app.
5. **Hygiene (§6)** — continuous, never urgent, decides whether any of this
   is usable by anyone else.

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
