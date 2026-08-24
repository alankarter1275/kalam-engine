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
"everything" can compare rather than match every variant.

Damage accumulates *independently* of that ordering. Intent answers how
disturbing a change is; damage answers where it is; and tying the second to
the first meant a highlight discarded the region a live selection had
already named, repainting the whole page to add a mark to two lines.
Selections and highlights both state their region now, the frame reports
the union, and `None` — the whole page, always correct and sometimes
pessimistic — is reserved for changes that genuinely cannot say where they
went.

Panel colour is pipeline policy rather than per-shell improvisation:
`PixelFormat::Grey { levels, dither }` quantizes luminance to a panel's
2..=16 steps, diffusing the error Floyd–Steinberg when asked. Orientation
is a page metric: `PageMetrics::rotation` turns the output on its way to
the buffer without touching layout, and `panel_to_page` is its inverse for
input, so a rotated shell hands the session panel coordinates and the
session untwists them. Both live in chapbook-paint over plain RGBA rows, so
every backend applies the same policy — they belong to the target, not to
the rasterizer. Packing grey levels into a device's buffer layout belongs
one step lower still, to whoever addresses the hardware: `Panel::blit`.

**The update seam.** A panel is not presented to; it is *asked* to change,
and how it is asked decides how the change looks. `UpdateClass` — none,
monochrome, fast, quality, flash — is the vocabulary for that ask, and
`FrameIntent::update_class` is the join: chapbook names the kind of change,
a device names the waveform it calls that. Nothing device-specific appears
above the `Panel` trait and nothing about books appears below it.
`RefreshPolicy` tracks ghosting debt separately, because "what does this
change need" and "is the screen due for a clean" are different questions,
and only the second has a knob a user might want. Fast and monochrome
updates accrue debt but never trigger the flash themselves: flashing the
screen under a moving finger is worse than any amount of ghosting.

`PanelDriver` owns the rules a shell would otherwise have to remember,
and each is the kind that works on a desk and fails on a device. It never
writes under a live update, waiting only when the regions actually
overlap so an unrelated corner does not pay for a slow refresh elsewhere.
It repaints what a monochrome update degraded, on a `settle` call, because
the session cannot see the moment a gesture ends — it has no way to tell a
mid-drag `select_range` from the last one, and only the shell knows the
pointer came up. And it rations the flash through `RefreshPolicy`. All of
it is asserted against `RecordingPanel` on a build machine.

`Panel` splits `blit` from `submit` because a real controller does — a
memcpy into mapped memory, then an ioctl — and an update takes 100ms to a
second, so folding the wait into the submit would make page turns feel
broken. `submit` returns a token; the caller decides when it needs to know
the pixels landed. `PanelRect` rounds outward, once, for everybody: a
region trimmed by half a pixel leaves a stale sliver, and on e-ink a stale
sliver stays until something else disturbs it. `RecordingPanel` keeps a log
instead of a screen, so "a drag issued one update per pixel of travel" is
an ordinary assertion on a build machine with no panel attached.

The trait deliberately carries no rotation. `PageMetrics::rotation` is
already the one place a turn is decided and applied; a panel reporting its
own would be a second field meaning nearly the same thing with nothing to
say which wins.

**A second backend exists, and it earned its keep.**
`chapbook-render-vello` rasterizes the same display list on the GPU through
vello and wgpu. The translation is a transcription: vello's glyph API takes
pre-positioned glyph ids, so nothing is re-shaped on the way, and device
scale is a scene transform rather than scaled coordinates. A parity test
compares the backends by normalizing total ink away and finding the shift
that best aligns each page's row and column profiles — displacement is what
can actually go wrong across a seam, and that measure catches a 2px error
while tolerating the antialiasing difference that centroid and per-cell
coverage both mistake for movement.

`chapbook-viewer --gpu` is a shell over it: a wgpu surface on the window
instead of softbuffer, `Session::frame` instead of `Session::render`, and
the session unchanged and unaware. Panel policy (grey quantization,
rotation) is not wired on that path — those are e-ink properties, and
applying them to a swapchain means a compute pass rather than the row
operations in chapbook-paint. Worth doing when a GPU e-ink shell exists.

It found a real bug on its first honest comparison: every line of text was
rendering up to 1.6px above the baseline layout computed, because swash
applies cosmic-text's vertical sub-pixel bin in the opposite direction. One
backend could not see it — the pages looked fine and the goldens encoded
the error. That is the argument for a second implementation, stated better
than any amount of design review could.

Two smaller findings came with it: reading a face out of the session's font
database copies its bytes, so every backend ends up caching blobs by
`fontdb::ID` and the seam could hand out font data directly; and the ops
list is exactly three variants (`FillRect`, `GlyphRun`, `Image`) rather
than the six this document's sibling once claimed, because borders, rules,
and decorations all lower to fills first.

Increments left, in the order they will hurt:

- **A device implementation, which cannot be written from here.** `mxcfb`
  is not a generic e-ink API — it is NXP's i.MX driver, and the EPDC is a
  block in the SoC rather than anything the panel knows about. It looks
  universal only because one vendor's chip won a decade of the market.
  KOReader's `framebuffer_mxcfb.lua` carries eleven device-specific
  refresh functions, several `mxcfb_update_data` struct versions, and
  waveform constants that differ per vendor for the same logical update;
  Allwinner Kobos reach it through a shim and need their own backend,
  MediaTek is a third path, reMarkable 2 has no framebuffer at all.
  `chapbook-panel-fbdev` is the stand-in: a plain Linux framebuffer, no
  EPDC, `UpdateClass` accepted and ignored, which gives the trait a second
  implementor the way vello did for the display list and runs on hardware
  that exists. Everything above `Panel` is exercised by it end to end. The
  ioctl layer waits for a device, because code that runs is not evidence
  that it is right.

  Its packing is checked against pixel formats a kernel chose, not ones we
  typed: `scripts/fbdev-vm.sh` boots vesafb under QEMU at RGB565, at
  24bpp, and at 8bpp palette, and `--example selftest` writes known
  colours and reads them back. That found a real defect: an 8bpp palette
  framebuffer reports all three channels at offset 0 with length 8, which
  is not a layout at all, and packing to it collapsed every colour onto
  one value. Classification now comes from `fb_fix_screeninfo.visual`
  rather than from the shape of the bitfields, and palette and monochrome
  visuals are refused with a reason instead of drawn wrong. The harness is
  x86_64 only — the ARM targets are compiled, never run.

  It has a hole, and it is the one that matters most: vesafb offers
  *palette* at 8bpp, not greyscale, so `Encoding::Grey` — the path a real
  Kobo or Kindle is most likely to take, since KOReader forces 8bpp grey
  on devices that do not default to it — is still covered only by unit
  tests against constants we typed.
- **Damage beyond selections and highlights.** A page turn that only moves
  a footer, or an image landing in a fixed rect, could both state their
  region and don't.
- **RGBA end-to-end is the expensive assumption.** For a 1404×1872 panel:
  a 10.5 MB buffer, a full `quantize` pass, then `rotate` returning a fresh
  `Vec` rather than working in place — another 10.5 MB — to produce 1.3 MB
  for a 4-bit panel. Three passes and ~21 MB of churn per page turn on a
  ~1 GHz ARM core. Free on desktop, possibly a visible slice of the refresh
  budget on device. Measure on hardware before optimizing, but do not be
  surprised by it.
- **`PixelFormat::Grey` clamps `levels` to 2..=16 silently.** A 32-level
  panel gets quietly wrong output rather than an error, and colour e-ink
  (Kaleido, Gallery 3) is foreclosed by a `clamp` call rather than by a
  decision anyone wrote down.
- **`dither` is page-global.** Its own doc says images need it and body
  text does not, but one flag covers the whole page, so it cannot be both.
  The display list knows which ops are images; that information is being
  discarded. Mostly harmless at 16 levels, decisive at 2.
- **Quantizing above the panel is redundant on the hardware that matters,
  and the level count is attached to the wrong thing.** An EPDC quantizes
  and dithers itself — passthrough, Floyd–Steinberg, Atkinson, ordered,
  quant-only, with `quant_bit` setting the depth — so a CPU pass over a
  multi-megabyte buffer buys nothing on exactly the device where it costs
  most. Worse, the correct depth is a property of the *update*, not the
  session: A2 is two levels, GC4 four, GC16 sixteen. Quantize the buffer
  to sixteen and an A2 update re-quantizes it anyway; quantize to two and
  the next page turn is ruined. On an EPDC the right move is to hand over
  8-bit grey undithered and let the controller decide per update, which
  means `PixelFormat` wants to become a request a panel can decline rather
  than a decision made above it. That inverts part of the current design,
  so it is written down rather than acted on.
- **Sub-byte pixels are foreclosed.** `FbdevPanel::open` rejects any depth
  that is not a whole number of bytes, so a 1bpp framebuffer is turned
  away before anything else runs — and 1-bit packed is the *normal* case
  for a bare SPI panel, as well as what `PixelFormat::Grey { levels: 2 }`
  exists to serve.

### The targets the seam has to survive

Not a roadmap — a set of shapes to check designs against, because each one
pulls in a different direction and any two of them agreeing proves nothing.

| Target | Surface | Pixels | Update model |
|---|---|---|---|
| Kobo Clara, Kindle (i.MX, Carta) | fbdev + `mxcfb` ioctls | 8bpp grey preferred; RGB565 the common default | EPDC waveforms, hardware dither, `quant_bit` per update |
| Kobo colour (Kaleido, MTK) | fbdev + MTK ioctls | 32bpp forced — the driver has no 8bpp | waveforms; colour via a filter array over a mono panel |
| Desktop | swapchain (winit/GTK, tiny-skia or vello) | RGBA | none; present every frame |
| Android | JNI to a `Surface`; Onyx adds `EpdController` | ARGB_8888 | none, or Onyx's own DU/GC/A2/REGAL |
| Pi + Waveshare SPI | SPI transfer plus a BUSY pin | 1bpp packed; some panels 2 or 4 levels | whole-panel or window refresh commands |

The encouraging result is that `Panel` survives all five. A file
descriptor and an mmap, a JNI call, and an SPI transaction are the same
three operations — stage the pixels, ask for a change, find out when it
landed — and `blit`/`submit`/`wait` is that, with the token making the
asynchrony explicit rather than assumed.

Two of the gaps it exposed were contract wording rather than design, and
are settled. `blit` no longer says "into the panel's own memory" — true
for a mapped framebuffer and a locked platform surface, false for a panel
on the far end of a bus, where `blit` stages and `submit` transmits; the
obligation is only that the pixels are taken before it returns. And
`submit` now states that a panel may refresh **more** than it was asked to
and never less, since controllers impose alignment and bus-attached panels
refresh byte-aligned windows or the whole screen. Widening costs time;
narrowing leaves the screen showing something untrue, which on e-ink
persists. A panel that widens owns the consequence: the no-write-under-a-
live-update rule is enforced above against the region *requested*, because
that is all a caller knows, so a panel that went further must make its own
`blit` safe — which on a bus falls out for free, since it cannot transmit
while the controller is busy.

Three things the matrix says the abstraction still gets wrong.

**E-ink does not imply greyscale.** Kaleido is a colour filter array over a
mono panel — you send RGB, the array resolves it, and the driver forces
32bpp. `PixelFormat::Rgba` with a full set of waveforms is a real
combination. `PixelFormat` and `UpdateClass` being orthogonal already
handles it; the `2..=16` clamp and the "e-ink means grey" phrasing
throughout this document do not.

**Who quantizes is per-target.** The EPDC does it in hardware and better;
a Waveshare panel needs the host to do it *and* to pack to 1 bit; a
desktop needs none of it. One session-wide decision cannot serve three
answers — see the quantization item above.

**Android argues for panel-owned staging.** Its natural shape is lock a
surface, write, unlock, which is exactly `blit` into memory the panel
owns. Handing a borrowed slice across an FFI boundary every frame works
but fights the platform, and it is the same conclusion the SPI case
reaches from the other side. Worth settling before §3 fixes the FFI shape.

## 2. The reading model above the page

The session could turn pages and change fonts; everything else here was a
gap a shell would have had to reinvent. Most of it has since landed —
what's left is called out per item, and the one structural piece is the
session's own shape, which §3 forces anyway.

**Navigation (landed).** `goto`, `goto_anchor`, `goto_toc`, `link_at`,
`follow_link`, and a capped back-stack: jumps remember where they came from,
ordinary page turns don't, so a footnote returns to the sentence that sent
you. Fragments resolve through the `anchors: id→page` map layout already
computed, and a fragment the unit turns out not to have lands at its start
rather than failing. Links ride the locator offset space rather than the
fragment tree — paint carries no DOM types — which also means a link hit
test survives relayout for free, and that a tap in the margin beside a link
is not a tap on the link.

**Annotations (landed, minus export).** Highlights, notes, and bookmarks all
have callers; `highlight_at` finds the mark under a tap so a reader can
recolor or delete one by touching it rather than by id; `annotations()`
lists every mark in the book without resolving any of them, since the
stored record already carries its quote and progression; `goto_annotation`
jumps to one. Bookmarks are points and paint nothing. Colors are stored as
written and fall back to the theme when absent or unparseable. What's left
is interchange — serializing the W3C EPUB Annotations 1.0 / Readium profile
(§5), not more model.

**Search (in-book landed).** `search_unit` is the building block — a shell
wanting the whole book without blocking drives it unit by unit on a worker —
and `search` walks the spine for the impatient, with the same blocking
contract as `unit_bytes`. Hits are locators, so they feed straight into
`goto`, and `select_range` puts one on the page. Each carries a
whitespace-collapsed context snippet with the match's range inside it, since
locator text is raw source text and a results list can't show that. Matching
folds case one character at a time, which keeps every hit on an exact
offset; full case folding and diacritic folding would not.
Library-level search across books is a second, separate want.

**Settings (landed, minus font family).** `set_settings` takes the whole
`ReadingSettings` — including `line_height`, `justify`, and
`publisher_styles`, which no shell could reach before — and persists it to
the library, so font size survives a restart. `SettingsScope` picks whether
a change is the reader's default or this book's override; an override
outlives later changes to the default, and `clear_book_settings` hands the
book back. `cycle_theme` and `adjust_font` remain as conveniences over it.
Still missing: font-family selection, which needs a font-enumeration story
before it needs an API. (Margins are fine: they live in `PageMetrics`,
supplied by the shell.)

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

**Cross-compilation is partly proven.** CI now checks `chapbook-core` and
`chapbook-panel-fbdev` against armv7 and aarch64, which is what makes the
panel backend's kernel-struct assertions worth having: `fb_fix_screeninfo`
embeds two `unsigned long`, so its field offsets move with word size, and
a drifted transcription reads plausible garbage rather than failing. Those
are const-evaluated, so the check needs no linker, device, or emulator.

The rest is still unproven. Only two crates cross-compile today — the
others want sqlite, fontconfig and GTK — and nothing has been *run* on
ARM, only compiled. These devices ship old glibc; binary size becomes a
real budget, and font provisioning (fontdb with no usable system fonts)
needs a bundled-font policy. Nothing about the architecture prevents any
of it, but unproven is a gap.

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

1. **FFI boundary (§3)** — gate on the largest device markets; forces the
   session API into SDK shape, which is also what §2's remaining piece
   (typed sources and injectable I/O instead of `open(&str)`) needs.
2. **Sync clients (§5)** — belongs to the platform, not to each app, and
   annotation interchange rides along.
3. **Hygiene (§6)** — continuous, never urgent, decides whether any of this
   is usable by anyone else.

The render seam (§1) came first and is closed; §2 followed and is closed
apart from session lifecycle and font-family selection. What remains of §1
— damage for intents other than selection — is an increment, not a gate.

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
