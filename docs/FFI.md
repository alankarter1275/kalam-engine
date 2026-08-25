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

**Pixels line up — measured, not reasoned.** tiny-skia's `Pixmap` is
premultiplied RGBA8888 and an Android `Bitmap.Config.ARGB_8888` is
premultiplied RGBA in native memory, so the frame `memcpy`s straight into
locked bitmap pixels with no conversion. Confirmed on a device rather than
inferred from two documents: black text on white paper cannot tell RGBA from
BGRA, so the check was run against the sepia theme, whose paper sampled
**(246, 240, 226)** on screen — warm, `R > G > B`. A channel swap would have
read (226, 240, 246), and looked cold blue.

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
# Gradle needs a compiler, and a JRE is not one — see below.
sudo apt install openjdk-21-jdk
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
   `Source::Reader` and `Format::Guess` to be real. *Not yet done — rungs 1
   through 4 are.*

### What rungs 1 and 2 found

**Rung 1 needed no source changes at all.** `cargo ndk -t arm64-v8a -t x86_64
-P 24 build -p chapbook-reader` builds the whole thing, every feature on —
bundled SQLite and `ring` both compile against the NDK's clang without
persuasion. That is better than this document predicted: the two C gaps were
the only ones, and the toolchain closes both.

Then four things the build could not have told us.

**A font source is not merely missing, it is unreachable.** `Session::open`
calls `chapbook_layout::system_font_system()` directly, and nothing takes its
place, so on Android the session begins life with an empty font database and
no way to fill it. The only door is `paint_resources()`, which hands back
`&mut FontSystem` — so the spike reaches through a *paint* accessor to
configure fonts before anything is painted, which works solely because the two
happen to share a `FontSystem`. It is the right shape for exactly nobody, and
it is the strongest argument in this document for the constructor argument.

**Loading the faces is only half of it.** Even with `/system/fonts` in the
database, the five CSS generics still resolve to fontdb's built-in defaults,
which are desktop family names no Android device has. They have to be pointed
at `Noto Serif`, `Roboto` and friends explicitly. On Linux fontconfig does this
invisibly, which is why it has never come up. A `FontSource` therefore has to
carry generic-family mappings, not just a list of faces or directories.

**`#[link(name = "jnigraphics")]` is load-bearing, and its absence is
silent.** Without it the crate compiles, the `.so` is produced, and
`llvm-readelf` shows all three `AndroidBitmap_*` symbols `UND` with no
`DT_NEEDED` entry naming the library. Nothing fails until `System.loadLibrary`
on a device. Any C ABI shipped as an Android artifact wants a link-time check
that the needed list is what it should be, because a green build proves
nothing here.

**The conformance harness takes an opener, not a session.** `Harness::new`
wants `FnMut() -> Session`, because "position survives a restart" cannot be
asked of a session that never stopped. So the C ABI cannot expose conformance
as a method on a handle; it needs the source, and it needs to construct
sessions itself. Worth knowing before the header says otherwise.

Size, for the record: the release `.so` for arm64 with CBZ and PDF built in is
**16.4 MB**, x86_64 **18.5 MB**, and the debug APK carrying both is 35.7 MB.
In the same country as the 18.8 MB fbdev binary, and the same answer applies —
feature flags, and per-ABI bundle splits so no device downloads two.

### What rung 3 found

**Everything the engine logs is invisible.** `chapbook-reader` makes eleven
`eprintln!` calls and the workspace has no `log` or `tracing` facade at all —
a deliberate scope choice that works fine for a desktop shell and stops
working at the boundary. "library unavailable", "page N failed to load",
"resuming at unit N": on Android stderr goes nowhere an app can read. The C
ABI needs a log sink, and a callback is the cheap version of it.

**Two failure classes that a green build cannot see.** A missing
`#[link(name = ...)]` and a drifted `external fun` name both produce a
complete, packaged, installable app that dies on the device — the first at
`System.loadLibrary`, the second at the first call, because Kotlin and Rust
never reference each other at compile time. `android/build-jni.sh` now checks
both after every build. Any real binding wants the same two checks, and a C
ABI wants a third: that the header and the exported symbols agree.

**The 16 KB page worry comes off the list.** Both `.so` files come out with
`LOAD` alignment `0x4000` without being asked, so NDK 30 satisfies Android
15's requirement by default.

**The environment is most of the work.** None of it is chapbook's problem
and all of it is in the way, so: **Gradle needs a JDK, not a JRE.** With only
a JRE installed, Gradle's toolchain auto-detection selects an installation
that has no compiler in it and fails before compiling anything, and the error
names the toolchain rather than the missing package. Android Studio's bundled
JBR does have `javac`, but it is Java 25, which Gradle 8.14 refuses — so it
is not the escape hatch it looks like. Install a JDK (`openjdk-21-jdk` on
Debian) and nothing else needs configuring: no `JAVA_HOME`, no toolchain
settings, no entry in `gradle.properties`. Separately, cargo-ndk 4 changed
`-p` from platform to package, so the API level is `-P` now.

### What rung 4 found: it conforms

The harness ran on an Android 36 emulator, through the JNI binding, against
Moby-Dick:

```
  ok      a crossed unit still moves
  ok      page turns walk the whole book
  ok      turns are reversible
  ok      the end of the book stands still
  ok      resize keeps the place
  ok      rotation is not a reflow
  ok      frame consumes the change record
  skip    damage stays inside the page — no frame stated a damage region
  ok      pending loads converge
  ok      a selection lives and dies with the page
  ok      position survives a restart
```

Ten passed, one skipped, none failed. The skip is the harness being honest:
an EPUB with no selection and no arriving images never states a damage
region, so there is nothing to check rather than nothing wrong.

**"Position survives a restart" is the load-bearing one.** It means bundled
SQLite works on Android, `CHAPBOOK_LIBRARY_DIR` found somewhere writable
under `filesDir`, and a locator round-tripped through the library. Watching
the app come back on the unit it was left on — and in the theme it was left
in, since settings persist the same way — is the same result arrived at from
outside.

**The font workaround holds.** 214 faces from `/system/fonts`, rising to 216
on a unit with embedded webfonts, so the webfont path works through the
binding too. Text pages render justified and hyphenated with the publisher's
own faces.

**Nothing the engine printed reached logcat**, as predicted. Not one of the
eleven `eprintln!` diagnostics appeared, which is the finding above confirmed
from the other side rather than a new one.

What rung 4 did *not* settle: this is x86_64 under an emulator. Nothing here
has run on arm64 silicon, and the panel-side questions PLATFORM cares about —
refresh policy, e-ink waveforms — are as unproven as they were.

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

## The iOS spike

Not started. This section is the handoff: what a session on a Mac can take as
settled, what is different from Android, and what only a device can answer.

**Most of the defect list is not Android's.** The font source being
unreachable, the missing generic-family mappings, the absent log seam, the
unbounded caches, and conformance needing an opener rather than a handle are
all properties of `chapbook-reader`, found on Android and true everywhere.
iOS should confirm them in passing, not rediscover them.

**The dependency graph is identical to Linux's.** `cargo tree --target
aarch64-apple-ios` resolves to the same 200 crates as x86_64 Linux, with
nothing added and nothing dropped, so there are no iOS-only dependencies and
no new licence surface to weigh. (Android is 198: it loses `fontconfig-parser`
and `roxmltree`, for the reason below.)

**fontdb has no iOS branch at all, and that is worse than Android's problem.**
There is not one `target_os = "ios"` in the crate. iOS is `unix` and is
neither `macos` nor `android`, so it falls into the branch marked *Linux* and
goes looking for `/usr/share/fonts/`, `/usr/local/share/fonts/`,
`$HOME/.fonts` and `$HOME/.local/share/fonts` — and, with the `fontconfig`
feature on as it is here, for `/etc/fonts/fonts.conf` first. None of that
exists on iOS. The result is the same empty database Android gets, reached by
a path that believes it is on a desktop, plus two crates of dead weight
compiled in to parse a config file that cannot be there. Whether
`/System/Library/Fonts` can be read from inside the sandbox is the first thing
to check on a device; if it cannot, the answer is CoreText enumeration or
bundled faces, and either way `FontSource` earns its place again.

**The library directory accidentally works, in the wrong place.**
`Library::default_dir()` falls back to `$HOME/.local/share/chapbook`, and iOS
*does* set `HOME`, to the app's container. So unlike Android it needs no
environment variable to function — it just puts the database somewhere Apple
would not, outside `Library/Application Support`, with the backup and
purgeability implications that carry. Working by accident is worth noticing
precisely because it will not raise an error.

**iOS is the more honest test of the C ABI.** Android went through JNI, which
is a layer of its own with its own conventions; Swift consumes a C header
directly, so there is no equivalent cushion. That makes this spike a better
proof of the boundary and a worse place to be sloppy about it.

The same discipline applies as on Android, and for the same reason: **do not
call the spike crate the C ABI crate.** It will contain `extern "C"`
functions because it has no choice, but a header generated and depended on
before the shape is fixed is exactly the freeze this document exists to
avoid. Name it a spike, keep cbindgen out of it, and throw it away.

### What only a Mac can answer

- Does bundled SQLite compile and run under the iOS sandbox, and does
  `ring` build for `aarch64-apple-ios`? Both are Xcode-clang questions that
  Android answered in its own terms and neither answer transfers.
- Static or dynamic: a `staticlib` plus a module map is the simple path, an
  XCFramework the packaged one. Android's `cdylib` shape does not decide it.
- The pixel path. A `CGBitmapContext` with `kCGImageAlphaPremultipliedLast`
  and `kCGBitmapByteOrder32Big` should be premultiplied RGBA and therefore a
  straight `memcpy`, exactly as `AndroidBitmap_lockPixels` turned out to be —
  but run the same test that settled it there, which is the sepia theme and
  not black text, because black on white cannot tell RGBA from BGRA.
- Whether `/System/Library/Fonts` is readable from a sandboxed app.
- Simulator versus device. Android's rungs ran on an x86_64 emulator and
  therefore proved nothing about arm64 silicon; a Mac can close that gap for
  Apple and, with a physical Android device, for Android too.

### The ladder, again

1. It builds: `cargo build --target aarch64-apple-ios-sim` and `-ios`.
2. It binds: a spike crate of `extern "C"` functions, hand-declared in Swift.
3. It draws: `render_into` a `CGBitmapContext`, tap zones, sepia for the
   channel check.
4. It conforms: the harness, on a simulator and then on a device.
5. It takes a security-scoped bookmark — the iOS twin of Android's content
   URI, and the other half of the argument for typed sources.

## Sequencing

PLATFORM's priority list puts the FFI first because it forces the §2
cleanup. That is right about the dependency and wrong about the order:

1. **Spike (throwaway).** Rungs 1–4 above. Output is a list of API defects,
   not code worth keeping.
2. **Fix the shape, in safe Rust.** Typed sources and a builder, a font
   source, `render_into`, a cache budget and `release_caches`, `suspend()`,
   and the optional `library` feature. All of it tested in the workspace,
   none of it FFI. This is PLATFORM §2's session-lifecycle item, arrived at
   by evidence instead of by guessing. The wasm32 CI check lands here, once
   there is something for it to prove.
3. **`chapbook-ffi` for real.** cbindgen header, stable error codes, Contract
   tier, and an FFI conformance test that drives the C ABI from a Rust test
   so CI covers the boundary without an emulator.
4. **`chapbook-android`, and the input model.** The AAR, the demo app, and
   `chapbook_core::input` — which lands here because a touchscreen is where
   tap zones can actually be judged.
5. **The browser demo.** Last, because Android has a user and a demo has an
   audience, and because by this point step 2 has already done all of its
   work for it.

## Still to decide

- **Whether `frame()` crosses the boundary at all**, or whether hosts get
  pixels only. Pixels only, for now, is the recommendation.

Settled: the binding is a hand-written C ABI, and WASM is a demo surface
rather than a product — see below for what that costs and what it keeps open.

## WASM: a demo, deliberately

`wasm-bindgen` wraps Rust, not C, so a WASM build would be a sibling exporter
over the same shape and never a consumer of the header. The C ABI is therefore
safe either way, and what follows constrains the *shape* only.

**Decided: build the demo, do not commit to a web reader.** A page that takes
someone's own EPUB and paginates it in front of them is the pitch for a
platform others are meant to build on, and at the size measured below it is
affordable. A web reader stays a live option and is not being planned.

What makes that cheap is that the demo is not a detour from the reader —
it is the same build profile, so it pays for the door rather than deferring
it. **EPUB only, no library, no loader thread, fonts embedded, opened from
bytes**: every one of those is a thing a browser forces and a thing a web
reader would have needed anyway. The same profile is what a stripped e-ink
build wants, which is the second reason to define it whether or not a browser
ever runs it. Concretely it implies a `library` feature on `chapbook-reader`,
default on, that a device or browser build turns off.

The argument against was never the port, it was that a browser build becomes a
fourth target to keep green forever. The answer to that is proportion: **check
it in CI, do not ship it from CI.** A `cargo check --target
wasm32-unknown-unknown` over the EPUB-only profile costs no linker, no
wasm-bindgen and no Node, and it catches exactly the regressions that would
quietly close the door — a new ambient `std::fs` call, an `Instant::now`, a
thread. Building and deploying the page itself stays manual and occasional.

That job should land *with* the optional-library work and not before. Today it
would pass while proving nothing, because `chapbook-library` compiles for
wasm32 perfectly well and merely fails to work — and this repo has already
been bitten once by a CI job that ran happily and checked nothing.

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
