# API stability

Which crates carry a stable interface and which are internals that will
move under you. In this fork "downstream" means one thing — Kalam — but
the split still matters: it says where a change must be careful and where
it can be casual. Upstream's original, written for a workspace of 21
crates and many consumers, is at `git show ab14cb7:docs/STABILITY.md`.

Nothing here is a promise about **1.0**. The workspace is at `0.1.0` and
under 1.0 a minor bump may break anything. What the tiers decide is
where breakage is *treated as a cost*, and where it is not.

## The tiers

| Tier | Crates | What it means |
|---|---|---|
| **Contract** | `chapbook-core`, `chapbook-paint` | Types that appear in signatures other crates must name (`LayeredLocator`, `ReadingSettings`, `Palette`, `DisplayList`). Changing one ripples through every crate below. Changed most reluctantly, and additively when possible. |
| **API** | `kalam-reader`, `chapbook-reader`, `chapbook-library` | What a host calls. `kalam-reader` is the crate Kalam actually links; `chapbook-reader` is the engine session underneath it; `chapbook-library` is the engine's own bookshelf, kept only until Kalam's database is wired in its place. Breaking changes are deliberate and named in the commit message. |
| **Producer** | `chapbook-epub` | The format reader behind `Publication`. Through `chapbook-reader` it is an implementation detail. |
| **Backend** | `chapbook-render-tinyskia` | Turns a `DisplayList` into pixels. The *trait* is stable; the crate implementing it is free to change. |
| **Internal** | `chapbook-layout` | No stability of any kind. Its DOM binding and cascade driver follow stylo's shape, and a stylo upgrade rewrites them. |
| **Not a library** | `chapbook-viewer-gtk`, `tools/chapbook-cli`, `tools/kalam-reader-demo` | Their surface is a command line or a window, not a Rust API. The viewer and the demo exist to be run and read, not linked. |

## Why the lines fall there

**Contract before API.** The natural first cut is "what does Kalam
call", which puts `kalam-reader` at the top. But a method added to a
session disturbs nobody, while a field added to `chapbook_core::Rect` or
a new variant of `chapbook_paint::DisplayOp` touches layout, paint and
rendering at once. Blast radius, not call frequency, earns the strictest
tier.

**`kalam-reader` is API, not Contract,** because it is the *end* of the
chain: nothing in the workspace depends on it, only Kalam does, and Kalam
and this repository are edited by the same person. A breaking change
there costs one commit on each side.

**Internal means internal.** `chapbook-layout` — its `dom` binding, its
`cascade` driver, and layout itself — is shaped by stylo's trait
requirements, not by a design anyone chose. Promising anything about
that surface would be promising something about Servo's.

## What this asks of a change

- **Contract or API tier:** a breaking change needs a reason in the
  commit message, not just a diff. Prefer additive change; prefer a new
  method to a changed signature. Edits to inherited crates also get a
  `kalam:` prefix and a row in `docs/kalam/UPSTREAM.md`, so the next
  upstream comparison knows what moved and why.
- **Producer or Backend tier:** break freely, fix the callers in the
  same commit. The workspace builds together, so the compiler finds them.
- **Internal tier:** no ceremony. These follow the engine.

## Depending on the engine

Kalam depends on **`kalam-reader` alone**. It re-exports the types a host
needs (`LayeredLocator`, `TocEntry`, `Highlight`, the prefs and theme
types) so Kalam never names an engine crate directly and cannot skew
versions with the engine it drives.

Nothing is published to crates.io. Kalam takes the engine as a git
dependency pinned to a commit on this repository; this document is the
only thing that distinguishes the tiers.

The `stability` test in the CLI crate keeps the table above in step with
the workspace: every member must be named here, and nothing named here
may have left.
