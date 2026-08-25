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
Selections, highlights and landed page images all state their region now,
the frame reports the union, and `None` — the whole page, always correct
and sometimes pessimistic — is reserved for changes that genuinely cannot
say where they went. Three of them can't: a page turn, a unit change and a
reflow each replace the page. The footer case this section used to
imagine — a turn that moves only the running head — does not arise,
because the engine paints no page furniture at all; there is no header,
no footer and no page number in a `DisplayList`, so nothing survives a
turn to be damaged around.

The general answer would be to diff consecutive display lists and damage
only the ops that differ. It is deliberately not taken: glyph ink extents
are not in the list — a `GlyphRun` carries advances, not bounds — so a
diff-derived rect could under-cover and leave stale pixels on a panel,
which is the one failure mode damage must not have.

The larger win was the opposite of stating a region: *not* reporting a
change. A prefetched unit landing used to raise `ContentArrived`, which
on e-ink spent a full-page Quality update to show a page that had not
moved. `poll_loaded` now answers "did the page on screen change", and
prefetches land silently.

Panel colour is pipeline policy rather than per-shell improvisation:
`PixelFormat::Grey { levels, dither }` quantizes luminance to a panel's
steps — floored at two, with no upper bound, since a cap there is a claim
about which panels exist rather than a property of the arithmetic —
diffusing the error Floyd–Steinberg when asked. It is orthogonal to
`UpdateClass`: e-ink does not imply greyscale, and a colour e-ink panel
takes `Rgba` alongside a full set of waveforms. Orientation
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
- **RGBA end-to-end is the expensive assumption**, and now partly
  measured. `cargo run -p chapbook-cli --example timings` breaks the
  pipeline down; `-p chapbook-paint --example quantbench` isolates the
  panel conversion. On x86 at Clara geometry, per page turn: render
  1.17ms, quantize 6.45ms, rotate 0. `rotate` no longer allocates — an
  unrotated page borrows — so that half of the churn is gone, but
  `quantize` is now essentially the whole cost, and it is at its serial
  floor (see the note in the function: three micro-optimizations tried,
  the two that helped were not bit-exact).

  **Possible later, deliberately not done now:** `quantize` runs over the
  whole page while `present` blits only the damage rect, so a selection
  drag converts ~1.5M pixels to update perhaps 50k. Scoping it to damage
  would cut that roughly by the damage ratio. The catch is that error
  diffusion is not local — a scoped pass starts from zero error at its
  edges, leaving a faint dither seam at the boundary — so it would want
  restricting to `dither: false`, or to the provisional classes that a
  later Quality update corrects anyway.

  This is probably premature. The numbers above are x86; on a Clara they
  would be several times larger, but the e-ink refresh they feed is
  ~450ms, so quantize is plausibly a tenth of perceived latency rather
  than the 80% of CPU time it looks like here. Trading a visible seam
  against that is not a judgement to make without the hardware in hand —
  and on an EPDC the right answer is likely the bullet below instead: hand
  over undithered grey and let the controller do it.
- **`dither` was page-global.** Its own doc said images need it and body
  text does not, but one flag covered the whole page, so it could not be
  both — and the display list, which knows which ops are images, was
  having that discarded at the seam. `DisplayList::dither_regions` now
  hands those rects to `chapbook_paint::quantize_regions`, which diffuses
  inside them and quantizes plainly everywhere else; `Session::render`
  and the fbdev example both go through it.

  The seam objection above applies here too and is answered by *where*
  the seam falls. Error diffusion scoped to a region starts from zero at
  its edges, which is a discontinuity — but an image's boundary is
  already a hard content edge, so a discontinuity there is invisible in a
  way the same one mid-paragraph would not be. That is the difference
  between this and scoping to a damage rect, whose edges fall wherever
  the last glyph happened to move.

  Mostly harmless at 16 levels, decisive at 2 — which is no longer
  hypothetical now that a 1bpp panel can be opened at all.
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
- **Sub-byte pixels** were foreclosed — `FbdevPanel::open` rejected any
  depth that was not a whole number of bytes, so a 1bpp framebuffer was
  turned away before anything else ran, and 1-bit packed is the *normal*
  case for a bare SPI panel as well as what `PixelFormat::Grey { levels: 2 }`
  exists to serve. It is now supported: `Encoding::Mono` packs eight
  pixels to a byte MSB-first, and a partial byte at either end of a damage
  rect is read-modified-written rather than overwritten, because damage
  rects come from glyph geometry and are aligned to nothing.

  Two details are load-bearing. Polarity comes from the kernel's visual
  (`FB_VISUAL_MONO10` versus `MONO01`) rather than from a convention,
  because guessing wrong produces a flawless negative and no error to
  notice it by. And depth is classified before `grayscale`, because a mono
  framebuffer may well set that flag and `Encoding::Grey` writes a whole
  byte per pixel — eight pixels' worth of memory for every one.

  A mono panel is also the one case where a non-`Rgba` default is honest:
  it asks for `Grey { levels: 2, dither: true }`, so the reduction happens
  where the error can be diffused rather than one pixel at a time in the
  blit's threshold. 2bpp and 4bpp are still out, and for a reason rather
  than an oversight: nothing in `fb_var_screeninfo` or `fb_fix_screeninfo`
  says which end of the byte their pixels start at.

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

E-ink implying greyscale was a third, and is settled: a colour e-ink panel
sends RGB through a filter array, so `Rgba` with a full set of waveforms is
an ordinary combination, and `PixelFormat` says so rather than leaving it
to be inferred. The `2..=16` cap that contradicted it is gone — floored at
two, which is arithmetic, with no ceiling, which was policy.

The remaining two turned out to be the same question, and the seam could
already answer it — nothing said so.

**Who reduces is the panel's call, not the session's.** An EPDC quantizes
and dithers in hardware; a bus-attached panel needs the host to do it and
to pack to one bit; a desktop needs none of it. `PanelInfo::format` is now
stated as a *request* rather than a description of the hardware, and it is
the only thing a caller consults. `Rgba` therefore does not mean "colour
screen" — it means **do not reduce, I will**, which is the right answer
for a controller with hardware dithering, for a greyscale framebuffer
whose `blit` takes luminance anyway, and for the awkward case below.

**Depth belongs to the update, not the session.** Two levels for a fast
waveform, four for a shallow one, sixteen for a full one. No single
`PixelFormat` expresses that, and pre-reducing to any one of them is wrong
in both directions. The resolution needs no new API: such a panel asks for
`Rgba`, keeps what `blit` staged, and reduces in `submit`, which is the
first point where the `UpdateClass` is known — legitimate because `blit`
is defined as *taking* the pixels rather than copying them into a mapping,
so panel-owned storage is a valid destination. That is also exactly the
shape Android wants (lock, write, unlock) and the only shape SPI allows,
so the three converge. A test panel does it, resolving one staged ramp to
2, 4 and 16 levels by class, so the arrangement is demonstrated rather
than asserted.

What stays open is narrower than it looked: whether a panel should be able
to hand *out* its staging buffer so a shell renders straight into it and
skips a copy. That is a throughput change, it belongs with the RGBA
end-to-end item above, and it wants a real backend and a measurement
rather than a guess.

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

Designed in [docs/FFI.md](FFI.md), which is where the boundary, the input
model and the Android spike plan now live. What follows is the statement of
the problem; that document is the proposed answer.

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
killed." Mandatory on mobile and e-ink. On Apple platforms it also has to
release file locks, not merely persist — see FFI.md.

**The boundary is not the only thing that assumes a desktop.** Designing it
surfaced the same assumption in the OPDS transport, the credential store and
the library's file custody. Those are §7.

**Cross-compilation is proven, except GTK.** CI checks `chapbook-core` and
`chapbook-panel-fbdev` against armv7 and aarch64, which is what makes the
panel backend's kernel-struct assertions worth having: `fb_fix_screeninfo`
embeds two `unsigned long`, so its field offsets move with word size, and
a drifted transcription reads plausible garbage rather than failing. Those
are const-evaluated, so the check needs no linker, device, or emulator.

Beyond `check`, everything except `chapbook-viewer-gtk` now cross-*builds
and links* for `aarch64-unknown-linux-gnu` — the CLI, the winit viewer,
and the panel examples, `ring` and bundled SQLite and hayro included. The
linked binary needs `libc`, `libm` and `libgcc_s` and nothing else.

Two worries recorded here were simply wrong. SQLite is `bundled`, so it
compiles from source with the cross toolchain instead of wanting a target
sysroot. And "fontconfig" is `fontconfig-parser`, a pure-Rust reader of
fontconfig's *config files* — libfontconfig is never linked, and never
was. GTK is the real exception, and it is a packaging problem rather than
a code one: `gobject-sys` wants a pkg-config sysroot for the target.

**It has been run.** The fbdev backend has been exercised on a Raspberry
Pi 4 with an 800x480 RGB565 DSI panel, cross-built as above: `probe`
reports what the kernel reports, `selftest` passes all seven packing cases
against read-back, and `show` renders and turns pages. What that did and
did not settle:

- The panel is `vc4drmfb` — **fbdev via DRM emulation, not a native fbdev
  driver**. That is how most modern ARM boards and a good few readers
  expose a framebuffer at all, so it is the more important case to have
  working, and it does.
- The struct assertions matching is reassuring but not news: aarch64
  shares pointer width and endianness with x86-64, so const-eval there
  already implied it. What was untested until now is the surrounding
  code — the ioctls, and classifying a real driver's `visual` and
  bitfields rather than QEMU's.
- Binary size is a real budget, as recorded: the all-formats `show` is
  18.8 MB. That is what §6's feature flags are for.
- Font provisioning did not bite, because a general-purpose distro ships
  dozens of fonts. The concern is narrower than written here: it applies
  to a stripped device rootfs, not to any Linux with a desktop lineage.
- **The refresh policy is still unproven.** That hardware is an LCD: no
  EPDC, no waveforms. `UpdateClass` resolves and the plumbing is
  exercised, but `RefreshPolicy`'s whole reason for existing — rationing
  the ghosting flash, refusing to flash under a moving finger — needs an
  e-ink panel and still has none.

The glibc worry also stands only for readers, not for boards: a current
distro is current, a Kobo is not.

## 4. Features shells cannot add from outside

These belong to the engine by construction; a downstream app cannot implement
them itself. Hyphenation is the proof the category is real: dictionary-based
`hyphens: auto` had to go in the line breaker, and did.

- **TTS** — needs text extraction plus word-level highlight timing.
- **Dictionary lookup** — word-boundary selection semantics.
- **Bidi correctness** for Arabic/Hebrew — cosmic-text can do it; confirm it
  is exercised and tested rather than assumed.
- **Accessibility** — no a11y tree export, no screen-reader path. Also a
  legal requirement in some markets. §7 turns this from a feature into a
  scheduling constraint: the display list is the material an a11y tree is
  built from, so keeping it out of the first C ABI decides that v1 cannot
  have one.

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

The local half is already built, which is easy to miss: `positions` carries
`updated_at` and the other user tables carry `deleted` soft-delete flags, so
the schema can answer "what changed since" without migration. See §7.

## 6. Platform hygiene (closed)

The unglamorous half, and the real distance between "modular codebase" and
"platform someone else can build on". All five bullets below are now done;
they are kept rather than deleted because each records a decision, and the
last two record a failure that motivated one.

- **API stability policy** — done: `STABILITY.md` sorts all seventeen
  workspace members into six tiers. The proposal here was public
  (`core`, `reader`, `library`, `opds`, `paint`) versus internal
  (`dom`, `style`, `layout`), leaving the render backends, the panel
  backends and the viewers unplaced. What settled them was asking about
  blast radius rather than call frequency: `core` and `paint` are
  *Contract*, because a change to either breaks every shell and every
  backend at once, including ones outside this repository; the backends
  are stable in the direction that matters, which is the trait they
  implement and not the crate implementing it. A test in the CLI crate
  keeps the document from silently omitting a member.
- **Feature flags.** Mostly done. `chapbook-reader` now gates `cbz`, `pdf`
  and `opds`, all on by default; a device build turns off what its hardware
  will never open. Measured on the `chapbook-panel-fbdev` `show` example,
  which is the device-shaped binary: **20.4 MB with all three, 12.5 MB with
  none — 39% smaller**, and 237 third-party crates down to 196. What leaves
  is the whole TLS stack (rustls, ring, webpki) and the whole PDF stack
  (hayro with its JBIG2, JPEG2000, CCITT and PostScript decoders). CBZ
  costs nothing on its own — its `zip` is already in the graph for EPUB.
  GTK was never the problem: it is a separate crate you simply do not
  depend on. Every shell in this workspace asks for all three, so CI checks
  the narrow configurations directly (`reader-features`) — otherwise they
  rot unnoticed.

  Knobs deliberately left alone, each wanting a device to justify it:
  cosmic-text's `fontconfig` default (fontdb falls back to scanning the
  usual font dirs without it, which is what a device has anyway);
  `rusqlite`'s `bundled`, which compiles SQLite from C and so needs a cross
  C toolchain; `hayro`'s `embed-fonts`/`embed-cmaps`, which are correctness
  for PDF; and wgpu's backend set behind `chapbook-render-vello`, which no
  device build links at all.
- **Docs for shell authors** — was the missing genre; now `SHELLS.md`.
  `ARCHITECTURE.md` explains the pipeline, this explains how to sit on top
  of it: the five-step shape of a shell, metrics in reading orientation,
  what `frame()` carries and the fact that taking one consumes the change
  record, the loader rule, position, panel policy, and what to depend on.
- **A shell conformance harness** — done: `chapbook_reader::conformance`.
  Given a session factory it asserts eleven rules a shell relies on, with
  `Skipped` a first-class outcome so a comic's missing text layer cannot
  masquerade as a pass; `examples/conform.rs` is the same thing from a
  terminal.

  The concrete instance behind it: the first end-to-end run of the fbdev
  example on real hardware turned exactly one page and stopped, and the
  defect was in the *shell*, not the engine — it compared `session.page()`
  across a turn, but `next_page` crosses into the next spine item by
  resetting the page to 0, so a unit change read as "did not move". Every
  book that opens on a single-page cover hit it. Nothing in the test suite
  could have caught it, because it is not about what `Session` computes but
  about how a shell drives it.

  The harness proves the rule; the API now also makes it hard to get
  wrong. `next_page`/`prev_page`/`next_unit`/`prev_unit` return whether the
  position moved, and `Session::position()` returns the `(spine, page)`
  pair as one value, so the broken comparison is no longer the obvious one
  to write.
- **A reference minimal shell** — done:
  `chapbook-viewer/examples/minimal.rs` — 185 lines against the viewer's
  389, and a good share of those are commentary. Selection, links,
  clipboard, touch and the GPU backend are all stripped out, so what is
  left is only what every shell must get right. Its six numbered comments
  are `SHELLS.md`'s five steps plus the loader rule.

## 7. The edges assume a desktop

The iOS assessment in [docs/FFI.md](FFI.md) asked a larger question than the
FFI: is this engine the right shape to build an Apple app on, or should an
iOS app lean on Apple's frameworks instead? The answer splits cleanly, and
the split is the same one §2 found in the constructor.

**The engine is right. The edges are wrong.** Keep the pipeline, the locator
design, the schema and the format parsers. Push transport, credentials, file
custody, fonts and accessibility metadata out to the platform. Every item
below is the same defect §2 named — an environment assumption baked into a
constructor — and the list there is two entries short.

### The render pipeline earns its place, measured

The obvious substitution is a `WKWebView`, which is what most iOS readers
are. It loses on the numbers, and it loses worse on locators.

A synthetic twelve-chapter book of dense body text, release build, Apple M2:

| target | device px | first page | repaint | page turn | bitmap |
|---|---|---|---|---|---|
| e-ink 800×480 @1x | 800×480 | 56 ms | 0.26 ms | 0.28 ms | 1.5 MB |
| iPhone 393×852 @2x | 786×1704 | 38 ms | 0.83 ms | 0.84 ms | 5.1 MB |
| iPhone 393×852 @3x | 1179×2556 | 39 ms | 1.76 ms | 1.79 ms | 11.5 MB |
| iPad 834×1194 @2x | 1668×2388 | 40 ms | 2.51 ms | 4.33 ms | 15.2 MB |

"First page" is open, parse, cascade, paginate and rasterize together. A page
turn inside an already-paginated chapter is under 2 ms at iPhone @3x. Assume
a phone core is three times slower than this one and it is still ~5 ms, for
work that happens once per gesture rather than once per frame. CPU
rasterization is not the constraint, and no performance argument for a web
view survives contact with these numbers.

The argument that actually matters is `LayeredLocator`. A web view's notion
of where you are is a function of its own line breaking, which moves under
you when the OS updates. The quote-context, spine-fraction and progression
record — versioned by `LOCATOR_VERSION`, and the reason §5's sync targets
are cheap — is what makes a position survive a font change, a rotation and a
different device. It cannot be rebuilt above a web view; a reader built that
way spends its life fighting one.

**The honest counterweight,** because this is a ceiling and belongs with the
others: `chapbook-layout` is 3,873 lines of layout over 421 lines of
cascade driver over stylo. That is a real subset of what publishers ship,
and a web view gets the long tail — MathML, ruby, broken markup — for free.
The pipeline is right *for a controlled-typography reader*. It is the wrong
tool for an app whose job is rendering arbitrary publisher EPUBs faithfully,
and no amount of FFI work changes that.

### Transport belongs to the host, and it is 273 lines

`chapbook-opds` reaches the network through `ureq` with its own rustls stack
and `webpki-roots`. On iOS that means bypassing `URLSession`, and the losses
are not cosmetic:

- **Background transfer.** A large download dies when the app suspends. A
  background `URLSession` is the only thing that finishes it, and there is no
  Rust equivalent — the OS continues the transfer, not the process.
- **System trust.** A bundled root store ignores MDM-installed roots and
  Apple's revocation policy.
- **App Transport Security, proxies, per-app VPN, Wi-Fi-only and the
  cellular-data toggle.** ATS governs `NSURLSession` and CFNetwork, not raw
  sockets, so the app's declared network posture silently does not cover this
  traffic.

The same argument gives Android background downloads through WorkManager and
gives WASM `fetch`, which it has no choice about.

The shape of the fix is already visible in the crate: of 1,727 lines of
source, `client.rs` is **273**. Everything else — `atom.rs`, `opds2.rs`,
`pse.rs`, `href.rs` — is format knowledge that should never move. Split the
client into a parser half and an injected transport trait, and the desktop
keeps `ureq` as one implementation of it.

**Credentials go with it.** `opds_sources.auth_secret` is plaintext in
SQLite, and `Session::open` reads `CHAPBOOK_OPDS_USER` and
`CHAPBOOK_OPDS_PASSWORD` from environment variables that do not exist on a
phone. Both belong behind an injected credential store, which is Keychain on
Apple platforms and the same shape as the font source everywhere else.

### Custody: bookmarks, not copies

`Library::import` does `std::fs::read` on the whole file to SHA-1 it, and so
does `fingerprint_of_file`. For a large CBZ or PDF that is a hundreds-of-
megabyte spike against a mobile memory limit, for a hash that should stream.
Small, concrete, and worth fixing on every platform.

The deeper mismatch is that a private directory owning copies of books is a
desktop idea. iOS users expect books to live in Files or iCloud Drive,
reached through the document browser and security-scoped bookmarks — visible
to them, not sealed inside a container, and not duplicated.

The schema already anticipates this: `books` carries `file_path` *and*
`source_path`, and `import` already writes an empty `file_path` for a book it
does not copy. So "we hold a record but not the file" is expressible today.
What is missing is somewhere to persist the bookmark, so a cold launch can
re-resolve access *before* the session is constructed — see FFI.md on why
that ordering is not optional.

### Storage stays SQLite, and is already sync-shaped

Bundled SQLite compiles and links for `aarch64-apple-ios`. Replacing it with
Core Data or SwiftData would make the storage layer unshareable with Linux
and Android, which is the whole point of having one. Keep it.

Three adjustments, all shell-visible rather than schema-visible: the database
belongs in `Library/Application Support` rather than the `$HOME` fallback it
lands in by accident, it needs an explicit backup-exclusion decision, and it
must not hold file locks across suspension once a share extension or widget
puts it in a shared container.

Worth recording as a thing that went right: `positions` carries `updated_at`,
and `books`, `annotations` and `opds_sources` all carry `deleted` soft-delete
flags. That is the local half of §5's sync story already in place — an iOS
shell can mirror those tables into CloudKit with no schema change, and sync
stays a shell concern rather than an engine one.

### Accessibility constrains the FFI's first version

§4 lists accessibility as a feature shells cannot add from outside. iOS
sharpens it into a scheduling constraint. A rasterized page is opaque to
VoiceOver: a reader that is a *picture* of text is unusable with a screen
reader, and a web view would have given that away for free.

Chapbook can do it — `frame()` already returns a display list whose text
fragments carry locators, which is exactly the material a
`UIAccessibilityElement` tree needs. But FFI.md's boundary deliberately keeps
the display list out of the first C ABI. On Android that was defensible. On
iOS it makes accessibility unimplementable in v1, and adding it after a
Contract-tier header exists is the expensive order. The selection loupe and
the edit menu want the same fragment geometry.

**Text identity, two smaller ones.** Nothing reads `UIContentSizeCategory`,
so Dynamic Type — the accessibility setting Apple users actually change —
does not reach `adjust_font`. And if the sandbox turns out to block
`/System/Library/Fonts`, note that the bundled-faces fallback cannot include
San Francisco: it is licensed for use *on* Apple platforms through the
system, not for redistribution in an app bundle. The reader would ship an OFL
face and would not look like an Apple app. That is a product decision, not a
bug.

## Priorities

1. **FFI boundary (§3)** — gate on the largest device markets; forces the
   session API into SDK shape, which is also what §2's remaining piece
   (typed sources and injectable I/O instead of `open(&str)`) needs.
2. **The edges (§7)** — the injected transport, credential store and file
   custody that the iOS assessment turned up. Sequenced here because it is
   the same shape work as §2's remaining piece and lands in the same pass,
   and because the accessibility finding constrains what the first C ABI
   may leave out.
3. **Sync clients (§5)** — belongs to the platform, not to each app, and
   annotation interchange rides along.
4. **Hygiene (§6)** — continuous, never urgent, decides whether any of this
   is usable by anyone else.

The render seam (§1) came first and is closed, including the damage
increment that was its last remainder; §2 followed and is closed apart
from session lifecycle and font-family selection.

§6 is now closed too, which was the cheapest of the three to underrate:
feature flags, the stability policy, the shell-author docs, the reference
minimal shell, and the conformance harness. The harness is the one worth
singling out — it exists because a shell defect got all the way to real
hardware past a green suite, and it is the only test here that watches
the seam from the outside.

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
