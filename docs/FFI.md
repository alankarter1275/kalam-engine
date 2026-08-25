# The portability boundary

Status: **design, nothing built.** This is PLATFORM §3's missing document —
the boundary that gates Android/Kotlin, iOS/Swift and WASM, plus the input
and lifecycle model that rides with it. It records what a spelunking pass
actually verified, what it proposes, and what is still a decision.

Read `docs/SHELLS.md` first. That describes the Rust-native contract a shell
already programs against; this describes what has to survive being called
from another language, and every gap below is a gap between those two.

## What the recon established

Five things, each checked rather than assumed.

**`Session` is `Send` and not `Sync`.** Confirmed by compiling the two
assertions: the `Send` bound passes, `Sync` fails on the loader's
`mpsc::Receiver` and on rusqlite's `RefCell<InnerConnection>`. That is the
ideal shape for a handle-based FFI, and it is worth stating in the header:
the host may move a session between threads, and must not touch one from
two threads at once. It also means the binding needs no lock of its own —
which is a design freedom, not an accident, so `crates/chapbook-reader/tests/
session.rs` now pins it.

**The engine already cross-compiles to Android except for two C
dependencies.** `cargo check --target aarch64-linux-android` passes clean,
with no NDK present at all, for `chapbook-core`, `chapbook-paint`,
`chapbook-layout`, `chapbook-render-tinyskia`, `chapbook-epub`,
`chapbook-cbz` and `chapbook-pdf` — stylo, cosmic-text, tiny-skia,
hyphenation and hayro included. Exactly two crates fail, both with the same
error and neither ours:

- `libsqlite3-sys` (bundled SQLite, reached through `chapbook-library`)
- `ring` (TLS, reached through `chapbook-opds`)

Both want `aarch64-linux-android-clang`, which is the NDK. Turning off the
`opds` feature drops `ring`; SQLite is on the path of every build, because
the library is where positions and annotations live. So the NDK is a
prerequisite, not a fallback, and it is the *only* prerequisite.

**Android loads no fonts.** fontdb 0.23 gates its system-font discovery on
`cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))`, so
`FontSystem::new()` on Android returns an empty database and every page
lays out with no faces. Android keeps its fonts in `/system/fonts` and
expects an app to go and get them. This is not a papercut to be patched
later: a font source is a **mandatory** constructor argument on that
platform, and the API does not have one today. It is also, conveniently,
the font-enumeration story PLATFORM §2 says font-family selection needs
before it needs an API.

**There is no memory ceiling.** `Session::layouts` and `Session::images`
are only ever cleared wholesale, on a metrics or settings change, and for
an image book the pixels are deliberately *not* cleared even then, because
they are metrics-independent. Nothing evicts per unit and nothing counts
bytes. Reading a comic forward accumulates every decoded page: a
1600x2400 RGBA page is 15 MB, so thirty pages is most of half a gigabyte.
Desktop never noticed. Android is the first platform that kills the
process for it, and `onTrimMemory` is a callback with nothing to call.

**Pixels should line up.** tiny-skia's `Pixmap` is premultiplied RGBA8888,
and an Android `Bitmap.Config.ARGB_8888` is premultiplied RGBA in native
memory, so the frame ought to `memcpy` straight into locked bitmap pixels
with no conversion. Ought to — this one is reasoned from both formats'
documentation and is the first thing the spike should check against a real
pixel, because a silently swapped red and blue channel is exactly the class
of bug that reads as "it works."

## The boundary

Six questions. The answers below are proposals.

**Handles.** One opaque pointer per session, created and destroyed by
explicit calls, no global state, no implicit singletons. Backed by the
`Send`-not-`Sync` finding: movable, not shareable.

**Sources — this is PLATFORM §2's last structural piece.** `open(source:
&str)` sniffs a string for `http://` and then for a file extension. Android
hands an app a `content://` URI resolved to a file descriptor, iOS hands it
a security-scoped bookmark, and a WASM build has no filesystem at all. None
of the three has a path, and two of the three have no extension either.

```rust
pub enum Format { Guess, Epub, Cbz, Pdf }

pub enum Source {
    Path(PathBuf),
    Bytes  { format: Format, bytes: Vec<u8> },
    Reader { format: Format, reader: Box<dyn ReadSeek + Send> },
    Url(String),                       // OPDS-PSE; feature-gated
}
```

`Reader` is what a file descriptor becomes, and `Read + Seek` is what the
zip and PDF readers already need. `Format::Guess` on bytes sniffs the magic
— the EPUB `mimetype` entry, `%PDF` — which is strictly better than the
extension test it replaces, since a hostile or merely misnamed `.epub` is
currently a parse error rather than a CBZ.

Everything else the constructor implicitly reaches for should come in the
same way, because each is an environment assumption that only holds on a
desktop: the library directory (today `CHAPBOOK_LIBRARY_DIR`, or XDG, or
`$HOME`), the font source, and OPDS credentials (today two environment
variables). A builder, not eleven arguments.

**Errors.** `ChapbookError` becomes a stable numbered enum, returned as a
negative `int32_t`, with a per-session "last error message" the host can
fetch for the human-readable half. Codes are the contract; strings are for
logs and are explicitly not stable.

**Strings and buffers.** Caller allocates, callee fills, callee reports the
length it needed:

```c
int32_t cb_session_title(cb_session*, char* buf, size_t cap, size_t* needed);
```

Call it with a generous buffer, or call it twice. What this buys is the
absence of `cb_free_string` — and with it the absence of the single most
common defect in hand-written C ABIs, a host freeing Rust's allocation with
the wrong allocator. Nothing crosses the boundary owned.

**Pixels.** `Session::render()` allocates and returns a `Pixmap`. Every
host platform already owns the buffer it wants drawn into: Android's
`AndroidBitmap_lockPixels`, iOS's `CGContext`, the `Uint8ClampedArray`
behind a WASM `ImageData`. So the engine wants

```rust
pub fn render_into(&mut self, dst: &mut [u8], width: u32, height: u32, stride: usize) -> bool;
```

with today's `render()` kept as the wrapper that allocates. This is a real
change to the engine, not a shim in the binding, and it is what makes the
boundary zero-copy on all three targets.

Shells that rasterize themselves still take `frame()` and the display list;
that path is unchanged and is deliberately *not* in the first C ABI. Three
`DisplayOp` variants are not hard to expose, but nothing needs them yet,
and STABILITY.md's Contract tier means whatever ships holds still.

**Events.** `set_waker` already takes a callback; the C form is
`void (*wake)(void* user)` plus a `void*`. The header has to say the thing
the Rust doc comment says: **it fires on the loader thread.** On Android
that means a thread the JVM has never seen, so the JNI layer attaches it,
or — better for a spike — writes a byte to a pipe the main looper watches.
Everything else the shell polls, and PLATFORM §2's wanted observer model
(layout invalidated, position changed, book finished) is the same callback
shape with a discriminant, once there is a second caller to justify it.

## Which FFI technology

**Decided: a hand-written C ABI in a new `chapbook-ffi` crate, with cbindgen
generating the header.**

The reason is not that it is the least work — it isn't — but that it is the
only artifact all three named hosts consume. Kotlin reaches it through JNI
or Panama, Swift through C interop directly, WASM through a `wasm-bindgen`
wrapper over the same core. A C header is also exactly the kind of object
STABILITY.md's Contract tier was written for, and the conformance harness
already establishes the culture of proving a boundary from outside it.
Licence hygiene points the same way: the repo gates its dependency terms in
`deny.toml`, and a C ABI adds nothing to the graph.

**The counter, considered and rejected, was UniFFI.** It would generate idiomatic Kotlin and
Swift — records, sealed classes, exceptions from `Result`, callback
interfaces — and delete essentially all of the glue described above. The
costs are that it is MPL-2.0 and therefore a `deny.toml` decision rather
than a technical one, that its WASM story is not the one we would want, and
that it dictates the API's shape from the outside. It was a genuine option, and
it is rejected on purpose rather than by default.

**What makes the choice cheap to defer:** the irreversible work here is the
*shape* — typed sources, `render_into`, a font source, a cache budget, a
C-callable waker — and all of that is safe Rust in `chapbook-reader`, sitting
under any exporter. Doing the shape first means the technology bet is made
with the API already known, and UniFFI, cbindgen and `wasm-bindgen` all stay
open until then.

## Input, lifecycle and power

The other half of PLATFORM §3, and the half where the obvious move is wrong.

**Do not put a gesture recognizer in the engine.** Android, iOS and GTK all
ship one, each is better than anything written here, and each has platform
conventions — fling velocity, edge slop, the long-press timeout a user set
in accessibility settings — that an app is judged on. A shell that fights
its platform's recognizer produces something that feels foreign, which is
the exact failure a portability layer is meant to prevent.

What *is* shared, and is genuinely engine knowledge:

- **An `Action` enum** as the seam. Shells translate native events into
  actions; the engine applies them. That removes hand-translation without
  taking the platform's gesture engine away: `NextPage`, `PrevPage`,
  `NextUnit`, `PrevUnit`, `Back`, `FontUp`, `FontDown`, `CycleTheme`,
  `ToggleMenu`.
- **Tap zones.** "Left third goes back, the rest goes forward, the middle
  band opens the menu" is policy every ereader gets slightly differently,
  and it depends on reading direction, which is engine knowledge. A
  `TapZones::action_at(x, y, &metrics)` returning `Option<Action>` is the
  whole feature, and it is pure and testable.
- **A key map.** The forcing case is hardware: Kobo and PocketBook have
  physical page-turn buttons, and no desktop shell has ever exercised one.
  A default binding table that already knows about them is the difference
  between porting and reverse-engineering.

Proposed home: `chapbook_core::input`, Contract tier, no event loop, no I/O.

**Lifecycle.** `save_position()` exists and is the right primitive, but a
platform needs one call that means *you are about to be killed*: position,
settings, and any dirty annotation, persisted now. Android's `onStop` is
the only guaranteed callback there is, and it has a time budget. Call it
`suspend()`; the reopen path already restores.

**Power and memory.** Two calls, both driven by the finding above:
`set_cache_budget(bytes)` so caches evict by least-recently-used instead of
growing, and `release_caches()` for `onTrimMemory` and for an e-ink device
that has been idle. Neither is Android-specific — a Kobo has 256 MB — but
Android is where it becomes an OOM kill rather than a slow day.

## The Android spike

Purpose: **discover the API's defects against a real platform before the C
header freezes them in.** A header designed without having run on the device
will be wrong, and the Contract tier means it will be wrong for a long time.
So the first rung is deliberately throwaway.

### What Android does and does not lend you

A recurring assumption worth killing early: Android ships SQLite and BoringSSL,
so surely native code should use them rather than carrying its own. It cannot.
The NDK exposes a fixed list of system libraries, and the dynamic linker refuses
everything outside it — this is the sysroot for NDK 30 at API 37, in full:

```
libaaudio  libamidi  libandroid  libbinder_ndk  libcamera2ndk  libc++  libc
libdl  libEGL  libGLESv1_CM  libGLESv2  libGLESv3  libicu  libjnigraphics
liblog  libmediandk  libm  libnativehelper  libnativewindow  libneuralnetworks
libOpenMAXAL  libOpenSLES  libstdc++  libsync  libvulkan  libz
```

No `libsqlite`, no `libssl`, no `libcrypto`. Android's SQLite is reachable only
through the Java `android.database.sqlite` API, and its BoringSSL only through
Java's `javax.net.ssl`. Both exist on the device as private libraries, and
linking them from native code fails at load time.

So:

- **SQLite: bundle it, which is what we already do.** `rusqlite`'s `bundled`
  feature is not a workaround here, it is the only supported arrangement for
  native code. Nothing to change; the alternative would be routing the whole
  library layer back through JNI to Java, which is a far worse trade than
  ~1 MB per ABI.
- **Crypto: bundle it too**, which `ring` already does. The part that is *not*
  fine to bundle is the **trust store**. `webpki-roots` carries Mozilla's CA
  list and therefore ignores the device's actual trust configuration —
  enterprise roots, user-installed CAs, an app's network security config, and
  OS root updates. The Android-correct answer is `rustls-platform-verifier`,
  which calls Android's `X509TrustManager` through JNI and honours all of it.
- **And none of this blocks the spike**, because TLS arrives only with the
  `opds` feature, and the spike should not build it. The trust-store question
  becomes real the day OPDS runs on Android, and not before.

Two entries on that list are ones we actively want: `libjnigraphics`, which is
`AndroidBitmap_lockPixels` and therefore the `render_into` destination, and
`libnativewindow` for a `SurfaceView` path later.

### Prerequisites

Present on the dev machine: Android Studio, SDK platform 37.0, build-tools
36 and 37, cmake 4.1.2, platform-tools, emulator, JDK 21. Missing:

```sh
sdkmanager --install "ndk;28.2.13676358"
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
```

NDK 28.2 rather than the newest: it is the current stable line, and it
defaults to 16 KB page alignment, which Android 15 requires of anything
targeting API 35 or later. Two targets because the device is arm64 and the
emulator is x86_64. `cargo-ndk` exists to set `CC`/`AR` per target, which is
precisely what the two failing C dependencies need — no other fix is
required.

### The ladder

Each rung is worth having on its own, and each one's failure says something
different.

1. **It builds.** `cargo ndk -t arm64-v8a -t x86_64 build -p chapbook-reader`.
   Proves the NDK closes both C gaps and nothing else was hiding behind them.
2. **It runs.** A `chapbook-jni` cdylib over `chapbook-reader` directly —
   not over `chapbook-ffi`, which does not exist yet and should not until
   rung 5. Eight entry points: open, set metrics, next, prev, render, title,
   position, close. `CHAPBOOK_LIBRARY_DIR` set to `context.filesDir` via
   `setenv`, which needs no API change and buys the whole storage question
   for free at this stage.
3. **It draws.** Kotlin `View`, an `ARGB_8888` `Bitmap`, `render_into` over
   locked pixels, tap to turn. This is where the premultiplied-RGBA
   assumption gets checked against a real pixel, and where the missing font
   source stops being theoretical — expect a blank page until
   `/system/fonts` is loaded explicitly.
4. **It conforms.** Expose the conformance harness as one call returning its
   report, and show it in the demo app. The harness is the outside-in check
   that already exists; running it through the binding tests the *binding*
   rather than the build, and it is the cheapest high-value rung on this
   list.
5. **It takes a content URI.** Open through the storage access framework,
   which has no path and no extension, and is therefore the rung that forces
   `Source::Reader` and `Format::Guess` to be real.

### Shape on disk

Two Gradle modules from the start, because a library others build on is the
thing PLATFORM is about and the split costs nothing on day one:

```
android/
  settings.gradle.kts
  chapbook/     library module -> AAR; jniLibs/{arm64-v8a,x86_64}/libchapbook.so
  demo/         app module; one View, one book, the conformance report
```

`minSdk 24`, `compileSdk 37`. Binary size is a live question — the
all-formats fbdev `show` binary is 18.8 MB on aarch64 Linux, and the `.so`
will not be smaller — but it is unmeasured here, and the answers when it
matters are the existing feature flags and per-ABI bundle splits.

## Sequencing

PLATFORM's priority list puts the FFI first because it forces the §2
cleanup. That is right about the dependency and wrong about the order:

1. **Spike (throwaway).** Rungs 1–4 above. Output is a list of API defects,
   not code worth keeping.
2. **Fix the shape, in safe Rust.** Typed sources and a builder, a font
   source, `render_into`, a cache budget and `release_caches`, `suspend()`.
   All of it tested in the workspace, none of it FFI. This is PLATFORM §2's
   session-lifecycle item, arrived at by evidence instead of by guessing.
3. **`chapbook-ffi` for real.** cbindgen header, stable error codes, Contract
   tier, and an FFI conformance test that drives the C ABI from a Rust test
   so CI covers the boundary without an emulator.
4. **`chapbook-android`, and the input model.** The AAR, the demo app, and
   `chapbook_core::input` — which lands here because a touchscreen is where
   tap zones can actually be judged.

## Still to decide

- **UniFFI or a hand-written C ABI.** Recommended above, but it is a
  licence decision as much as a technical one.
- **Whether `frame()` crosses the boundary at all**, or whether hosts get
  pixels only. Pixels only, for now, is the recommendation.
- **Whether the WASM target is real.** See below; the decision is open, but it
  does not gate the C ABI.

## WASM: measured, not decided

`wasm-bindgen` wraps Rust, not C, so a WASM build would be a sibling exporter
over the same shape and never a consumer of the header. The C ABI is therefore
safe either way, and what follows constrains the *shape* only.

**It compiles, and it is smaller than expected.** Every engine crate passes
`cargo check --target wasm32-unknown-unknown`, including `chapbook-library`;
only the `opds` feature fails, on `getrandom` wanting its `js` backend. A real
`cdylib` linking the whole EPUB reading path — open, paginate, walk, render,
search, TOC — built at `opt-level = "z"` with LTO, `panic = "abort"` and
symbols stripped, is **6.0 MB raw and 2.08 MB gzipped**. Brotli would land
lower still. That is an ordinary web app's payload, not a disqualifying one.

**But `check` is not `run`, and the gap is entirely ambient dependencies.**
Nothing in the engine's own code is hostile to WASM — there are no
platform-specific paths and no OS assumptions. What breaks at runtime is
everything the session reaches for implicitly: `std::fs` (stubs that always
fail), SQLite's need for a filesystem VFS, `std::env::var`, `std::thread`, and
`Instant::now`, which panics outright and which `conformance::settle` uses for
its budget.

Every one of those is already on the fix-the-shape list for Android reasons,
with exactly two exceptions. WASM is the only caller that forces:

- **The library must be optional.** No SQLite means no positions, no
  annotations, no import — a session that reads and reports its locator but
  persists nothing, with the host storing it. Defensible on non-WASM grounds
  too: a stripped e-ink build may not want SQLite either.
- **The loader thread must be optional.** EPUB never uses it — chapters lay
  out on the session thread — so an EPUB-only build should not require one.
  CBZ and PDF in a browser would need `SharedArrayBuffer` and
  cross-origin isolation, which is a separate decision from this one.

Both are cheap now and structural later, which is the argument for doing them
whether or not a browser build ever ships.

**stylo's own threading is already settled, and not by luck.** stylo is a
parallel style engine, so the obvious worry is that a browser build would need
`wasm-bindgen-rayon` and cross-origin isolation before it could style anything
at all. It does not. `chapbook-style` has always passed `None` for
`traverse_dom`'s rayon pool, and `STYLE_THREAD_POOL` is a `LazyLock` that
nothing on our path dereferences — the only two readers in stylo are its own
shutdown path and a Gecko-facing `get_thread_handles`. No pool is ever built
and no thread is ever spawned.

That is not a portability convenience, it is load-bearing:
`chapbook-dom`'s `unsafe impl Send`/`Sync for Node` justifies the
`Cell`/`UnsafeCell` state a traversal mutates by there being exactly one
traversal writing it. A pool passed to `traverse_dom` would race that state
in a build that still compiles and mostly still works. Until now the
invariant was held up by a code comment; it is now
`chapbook-style/tests/sequential.rs` and an entry in CONTRIBUTING's list.

What remains is dead weight rather than live threads: `rayon-core` is an
unconditional dependency of stylo, so it links, and its panic strings are
visible in the 6 MB module above. Deleting that is a size question, not a
correctness one.
