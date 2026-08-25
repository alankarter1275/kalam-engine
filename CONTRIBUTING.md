# Contributing

Design lives in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), reading
positions in [docs/LOCATORS.md](docs/LOCATORS.md), the OPDS wire contract in
[docs/OPDS-INTEROP.md](docs/OPDS-INTEROP.md), device porting in
[docs/PLATFORM.md](docs/PLATFORM.md), and driving the engine from a shell
in [docs/SHELLS.md](docs/SHELLS.md). Read the relevant one before changing
that subsystem — several of the constraints below are load-bearing in ways
the code alone does not explain.

## Verifying

Every change must pass the same gate CI runs, in this order:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets   # CI sets RUSTFLAGS=-D warnings
cargo test --workspace
cargo deny check licenses                # see NOTICE
```

Prerequisites are in the [README](README.md#getting-started).

The gate only means something on the pinned toolchain. `rust-toolchain.toml`
names an exact version and CI installs the same one, so `cargo` in this
directory picks it up with no action on your part — that is the point of
pinning rather than tracking `stable`. Running these commands under some
*other* compiler and reporting green is not running the gate: a clippy lint
that ships in a later release fails CI on code your run called clean. Bumping
the pin is a deliberate change, like the stylo pins, and the two places that
carry the version (`rust-toolchain.toml` and `RUST_PIN` in
`.github/workflows/ci.yml`) move together.

One consequence to know about: a pinned version is a *different toolchain*
from `stable`, with its own installed targets. If you cross-check the
device build, add them to the pin rather than to `stable`, or the check
fails with `can't find crate for core` — which reads like a broken build
and is not one:

```sh
rustup target add --toolchain "$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)" \
    armv7-unknown-linux-gnueabihf aarch64-unknown-linux-gnu
```

One more trap worth naming: clippy stops at the first crate that fails, so
the errors in a CI log are a prefix of the problem, not the whole of it. When
a new lint lands, sweep the workspace with `grep` for the pattern instead of
fixing what the log happened to reach.

Two kinds of golden test, with different update paths. Render goldens are
byte-exact PNGs; regenerate them deliberately with
`UPDATE_RENDER_GOLDENS=1 cargo test -p <crate>` and eyeball the diff before
committing — a golden that changed for a reason you cannot name is a bug you
are about to bless. Layout and style goldens use `insta`, so
`cargo insta review`.

The real-world corpus sweep is ignored by default because it needs a
download: `fixtures/fetch-corpus.sh`, then
`cargo test -p chapbook-cli --test corpus -- --ignored`.

## Invariants

These are not style preferences. Each one has cost somebody a day.

- **stylo lockstep pins.** `stylo`, `stylo_traits`, `stylo_atoms`,
  `stylo_static_prefs`, `stylo_dom`, `selectors` and `cssparser` are pinned
  with `=` and upgrade all at once, as a deliberate task — Blitz's diff is
  the migration guide. `html5ever`/`markup5ever`/`xml5ever` must match the
  markup5ever minor that stylo's selector types use. Never bump one alone.
- **MSRV 1.92, pinned stable toolchain.** No nightly features. The MSRV
  job builds under `RUSTUP_TOOLCHAIN`, not just an installed default,
  because `rust-toolchain.toml` outranks `rustup default` — without it
  the job rebuilds on the pinned stable and proves nothing.
- **No async runtime.** `opds-client`'s injected `HttpClient` is blocking by
  contract, and `UreqHttp` is only its default implementation; the page
  loader is a plain thread.
- **Core stays GPU-assumption-free**, so it can port to e-ink.
- **`DomNode` must stay pointer-sized** — stylo's style sharing cache
  statically asserts it.
- **stylo runs sequentially, and that is a soundness precondition.**
  `traverse_dom` always gets `None` for its rayon pool, because
  `chapbook-layout`'s `dom::tree` `unsafe impl Send`/`Sync for Node` justify
  the `Cell`/`UnsafeCell` node state by there being exactly one traversal
  writing it. Handing it a pool would invalidate that argument in a build
  that still compiles. Guarded by `chapbook-layout/tests/sequential.rs`,
  which also keeps `STYLE_THREAD_POOL` unnamed — it is a `LazyLock`, so it
  costs nothing until something dereferences it, and threads are spawned by
  the act of looking.
- **Never compact the spine.** A dangling idref keeps its slot with an empty
  href, because indices are locator identity and persisted positions are
  keyed by them.
- **Viewers never call `unit_bytes`/`resource` on the UI thread.** Both may
  block for seconds — a remote comic page, a cold PDF rasterization. That is
  what `chapbook-reader/src/loader.rs` is for.
- **Model types come from `chapbook_core` only**, never via an EPUB
  re-export, so `grep chapbook_epub` stays an honest coupling map. Fragments
  reference their source by opaque `u64` tag, never by DOM type.
- **Fragmentation properties live in chapbook-layout's sidecar cascade.**
  servo-mode stylo does not carry `break-*`, `page-break-*`, `widows`,
  `orphans` or `hyphens`, so adding one means adding it there.

## Out of scope

Fixed-layout EPUB, JavaScript, MathML, vertical writing modes, absolute
positioning, media overlays, DRM.

## Gotchas

Things that fail quietly rather than loudly.

- **Scripted edits against rustfmt-reflowed code silently no-op.** A `sed`
  or Python string replacement matches nothing when rustfmt has rewrapped
  the lines since you copied them, and reports success either way. Grep to
  confirm the edit landed.
- **quick-xml 0.42 reports `&amp;` and friends as `Event::GeneralRef`,**
  splitting the surrounding text into separate events. A `_ => {}` arm
  deletes every entity, and `trim_text(true)` then eats the spaces on either
  side — `Science &amp; Nature` becomes `ScienceNature`. Handle
  `GeneralRef`, leave `trim_text` off, and trim assembled values instead.
- **cosmic-text 0.19:** `Buffer` methods do not take a `FontSystem` except
  `new` and `shape_until_scroll`. Clamp `line_height` above zero — real
  books ship `line-height: 0`.
- **Library tests set process-global environment,** so they serialize behind
  `ENV_LOCK` and use per-test directories. Follow the existing
  `open_isolated` / `reopen_isolated` helpers in
  `chapbook-reader/tests/session.rs`.

## Licensing

Contributions are dual licensed MIT OR Apache-2.0, matching the project. New
dependencies must resolve to a license already in `deny.toml`, or the CI
license job fails and the addition becomes a decision somebody makes on
purpose. [NOTICE](NOTICE) explains what a binary already carries and why.
