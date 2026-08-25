# API stability

Seventeen workspace members is a lot of surface to promise nothing about
and far too much to promise everything about. This is the split: which
crates carry semver discipline, which are implementations you may depend
on at your own pace, and which are internals that will move under you.

Nothing here is a promise about **1.0**. The workspace is at `0.1.0` and
under 1.0 a minor bump may break anything — that is what `0.x` means, and
pinning `=0.1` is the honest way to depend on it today. What the tiers
decide is where breakage will be *treated as a cost* once versions start
meaning something, and where it will not be treated as a cost at all.

## The tiers

| Tier | Crates | What it means |
|---|---|---|
| **Contract** | `chapbook-core`, `chapbook-paint` | Types that appear in signatures a downstream must name. Breaking one breaks every shell *and* every backend at once. Changed most reluctantly. |
| **API** | `chapbook-reader`, `chapbook-library`, `chapbook-opds` | What a downstream calls. Semver discipline: breaking changes are deliberate, announced in the changelog, and worth the migration. |
| **Producer** | `chapbook-epub`, `chapbook-cbz`, `chapbook-pdf` | Format readers behind `Publication`. Depend on one only to open that format directly; through `chapbook-reader` they are an implementation detail. |
| **Backend** | `chapbook-render-tinyskia`, `chapbook-render-vello`, `chapbook-panel-fbdev` | Implementations of a Contract-tier trait. The *trait* is stable; the crate implementing it is free to change, because substituting it is the point. |
| **Internal** | `chapbook-dom`, `chapbook-style`, `chapbook-layout` | No stability of any kind. They exist to make the engine work, they follow stylo's shape rather than a design of their own, and a stylo upgrade rewrites them. |
| **Not a library** | `chapbook-viewer`, `chapbook-viewer-gtk`, `tools/chapbook-cli` | Binaries. Their surface is their command line, not their Rust API; the reference shells exist to be read and copied, not linked. |

## Why the lines fall there

**Contract before API.** The natural first cut is "what does an app call",
which puts `chapbook-reader` at the top. But `reader` can add a method
without disturbing anyone, while a field added to `chapbook_core::Rect`
or a fourth variant of `chapbook_paint::DisplayOp` breaks every
implementor of `Panel` and every rasterizer at once — including ones
outside this repository, which is the whole premise of `PLATFORM.md`.
Blast radius, not call frequency, is what earns the strictest tier.

That is also why `chapbook-paint` is Contract rather than Internal, even
though nothing outside the workspace has ever imported it: a render
backend cannot exist without naming `DisplayList`, and render backends
are the seam.

**Backends are stable in the direction that matters.** A device port
implements `chapbook_core::Panel`; it does not link `chapbook-panel-fbdev`.
So the promise belongs to the trait, and the framebuffer crate stays free
to change — it is a worked example of the port, not a dependency of one.
The same holds for the two rasterizers against `DisplayList`.

**`chapbook-library` is API, not Internal**, because it is not hidden:
`chapbook-reader` re-exports it and its `AnnotationKind` and record types
appear in the session's own signatures. A crate whose types leak through
a stable API is a stable API, whatever its intent was.

**`chapbook-opds` is API rather than Producer** because it is the one
crate here with a life of its own. It is a working OPDS 1.2/2.0 client
with no ereader attached, and it is plausible for something to depend on
it and nothing else.

**Internal means internal.** `dom`, `style` and `layout` are shaped by
stylo's trait requirements, not by a design anyone chose, and the pinned
stylo set upgrades all-at-once as a deliberate task that rewrites them.
Promising anything about their surface would be promising something about
Servo's.

## What this asks of a change

- **Contract or API tier:** a breaking change needs a reason in the commit
  message, not just a diff. Prefer additive change; prefer a new method to
  a changed signature; prefer a new variant on a `#[non_exhaustive]` enum
  to a changed one.
- **Producer or Backend tier:** break freely, fix the callers in the same
  commit. The workspace builds together, so the compiler finds them.
- **Internal tier:** no ceremony. These follow the engine.
- **Any tier:** if a change moves a type *between* crates, it moves
  between tiers too — `quantize` and `rotate` moving from
  `chapbook-render-tinyskia` into `chapbook-paint` promoted them from
  Backend to Contract, which is the correct outcome and worth noticing at
  the time rather than later.

## Depending on chapbook

A shell depends on **`chapbook-reader` alone**. It re-exports
`chapbook_core`, `chapbook_paint`, `chapbook_library`,
`chapbook_render_tinyskia`, `tiny_skia` and `cosmic_text`, so a shell
cannot skew versions with the engine it drives. `docs/SHELLS.md` §9 says
the same thing from the shell's side.

Reaching past that re-export — naming `chapbook-core` as a direct
dependency, say — is supported and sometimes necessary (a `Panel`
implementation must), but it is a version you now have to keep in step
yourself.

Nothing is published to crates.io yet. When it is, the Internal and
"Not a library" tiers get `publish = false` unless there is a reason not
to; until then, this document is the only thing that distinguishes them.
