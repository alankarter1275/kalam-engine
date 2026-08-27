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
- `ring` (TLS, reached through `chapbook-opds` → `opds-client`'s default
  `ureq` feature)

Both want `aarch64-linux-android-clang`, which is the NDK. Turning off the
`opds` feature drops `ring`; SQLite is on the path of every build, because
the library is where positions and annotations live. So the NDK is a
prerequisite, not a fallback, and it is the *only* prerequisite.

Since the OPDS split there is a better answer than dropping catalogs,
though it is not reachable from `chapbook-reader` yet: `ring` arrives
through `chapbook-opds`'s default `ureq` feature rather than through OPDS
itself, so a build that turns that off and hands `opds-client` an
`HttpClient` backed by the platform's own stack keeps catalogs and sheds
the NDK dependency. What is missing is a way to pass that transport through
`Session::open` — see PLATFORM §7.

**Android loads no fonts.** fontdb 0.23 gates its system-font discovery on
`cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))`, so
`FontSystem::new()` on Android returns an empty database and every page
lays out with no faces. Android keeps its fonts in `/system/fonts` and
expects an app to go and get them. This is not a papercut to be patched
later: a font source is a **mandatory** constructor argument on that
platform, and the API does not have one today. It is also, conveniently,
the font-enumeration story PLATFORM §2 says font-family selection needs
before it needs an API.

**There was no memory ceiling — now there is.** `Session::layouts` and
`Session::images` were cleared only wholesale, on a metrics or settings
change, and for an image book the pixels were deliberately *not* cleared
even then, because they are metrics-independent. Nothing evicted per unit
and nothing counted bytes.

Measured before fixing, on a synthesised 40-page 1600x2400 CBZ at iPhone
@3x: RSS went 29 MB at open, 92 MB after the first page, then **+15.4 MB
per page in a straight line** — exactly one decoded page, retained — to
**676 MB after forty**. This document's earlier estimate said thirty pages
was "most of half a gigabyte"; measured, page thirty is 538 MB, so the
estimate was right in shape and a little optimistic.

Worse than the growth was that nothing gave it back. A metrics change, a
theme change, and paging all the way back to page one each left RSS at
680 MB. There was no lever at all, which is what "`onTrimMemory` is a
callback with nothing to call" actually meant.

`SessionConfig::with_cache_budget` and `Session::set_cache_budget` now cap
both caches together, evicting least-recently-used units and never the one
being read. Re-measured on the same comic: **flat at 276 MB** under the
192 MiB default and **154 MB** under a 64 MiB budget, from page thirteen
and page four respectively onward. `Session::cache_bytes()` reports the
current total for a host that wants to decide.

Eviction is safe because nothing cached is authoritative — a comic page is
re-read from the archive and decoded again, a chapter is laid out again —
and a miss costs that re-decode, measured at 77-120 ms for a page this
size. That is the constraint on how small a budget can usefully be, and why
the current unit is pinned.

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

**Sources — done.** `open(source: &str)` sniffed a string for `http://` and
then for a file extension. Android hands an app a `content://` URI resolved
to a file descriptor, iOS hands it a security-scoped bookmark, and a WASM
build has no filesystem at all. None of the three has a path, and two of the
three have no extension either.

`chapbook_core::Source` is now what the session takes, through
`open_with(impl Into<Source>, SessionConfig)` — so a `&str` still means what
it always did and every existing caller compiles unchanged. Three notes from
building it, none of which the sketch below anticipated:

- **`ReadSeek` is `Send`, not `Send + Sync`,** even though rbook's
  `threadsafe` feature demands the latter. `Sync` is a strange thing to ask
  a *host* for — a reader shimming a foreign runtime manages `Send` easily
  and `Sync` only by adding a lock — so `chapbook-epub` adds that lock once,
  internally, and the public bound stays where a host can meet it.
- **`Format::Guess` beats the extension even when there is one.** The path
  branch sniffs the file head first and consults the name only when the
  bytes say nothing, which is what makes a misnamed book open correctly
  rather than fail to parse. The final fallback to EPUB is deliberate: rbook
  opens an *unzipped* EPUB directory, which has no leading bytes at all.
- **A stated format wins over the sniffer.** A host that resolved a
  `content://` URI already asked for a MIME type, and that answer may know
  things the first 64 bytes cannot.

Only path sources reach the library. Bytes and handles have no file to
record and no stable identity to key a position on, so they open at the
beginning every time — that is the custody question in PLATFORM §7, and a
source type cannot settle it alone.

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
`$HOME`), the font source, and OPDS credentials. A builder, not eleven
arguments. `SessionConfig` is that builder, and it now carries all of them:
the font source, `chapbook_core::CredentialStore`, the `HttpClient` and the
library directory, with the source itself typed as `Source`. Nothing in the
constructor reaches past the caller any more.

**Errors.** `ChapbookError` becomes a stable numbered enum, returned as a
negative `int32_t`, with a per-session "last error message" the host can
fetch for the human-readable half. Codes are the contract; strings are for
logs and are explicitly not stable.

**Panics are part of the error contract, and were missing from it.** A panic
unwinding out of an `extern "C"` function is undefined behaviour, and under
`panic = "abort"` it is a process kill the host cannot observe, report or
attribute. So every entry point wraps its body in `catch_unwind` and maps a
caught panic to a reserved code — one code, because a panic is a bug here and
not a condition worth enumerating. This belongs with the shape rather than
with the header: it constrains how each function is written, and retrofitting
it after a Contract-tier header exists means touching every function at once.

**Strings and buffers.** Caller allocates, callee fills, callee reports the
length it needed:

```c
int32_t cb_session_title(cb_session*, char* buf, size_t cap, size_t* needed);
```

Call it with a generous buffer, or call it twice. What this buys is the
absence of `cb_free_string` — and with it the absence of the single most
common defect in hand-written C ABIs, a host freeing Rust's allocation with
the wrong allocator. Nothing crosses the boundary owned.

**Pixels — done.** `Session::render()` allocates and returns a `Pixmap`.
Every host platform already owns the buffer it wants drawn into: Android's
`AndroidBitmap_lockPixels`, iOS's `CGBitmapContext`, the `Uint8ClampedArray`
behind a WASM `ImageData`. So the engine now has

```rust
pub fn render_size(&self) -> Option<(u32, u32)>;
pub fn render_into(&mut self, dst: &mut [u8], width: u32, height: u32, stride: usize) -> bool;
```

with `render()` kept as the wrapper that allocates. `render_size` came with
it, because a host cannot allocate a surface without being told how big one
is. This is a real change to the engine, not a shim in the binding: it
reaches down to `chapbook-render-tinyskia`, whose `render` now takes a
`PixmapMut` so its target can be memory this workspace did not allocate.

**What it is worth, measured rather than assumed.** At iPhone 393×852 @3x —
a 1179×2556, 12.1 MB surface, the case the table above calls 11.5 MB — over
60 frames of `fixtures/corpus/moby-dick.epub` on the Linux dev box:
`render()` then copying into a host buffer, which is what a shell pays
today, is **26.8 ms/frame**; `render_into` is **25.7 ms/frame**. About 4%.

That is honest and it is smaller than "removes the copy" sounds, because at
this surface size the copy is a small part of rasterizing a page. The effect
worth having is the other one: `render()` allocates and zeroes 12.1 MB per
frame, and `render_into` on the fast path allocates nothing at all. Android
is the platform that kills a process over exactly that kind of transient
pressure, and it is the same argument as the cache budget below.

**One caveat in the signature.** `stride` is honoured but only free when it
equals `width * 4`: tiny-skia cannot rasterize into a padded buffer and
rotation cannot be done in place, so a rotated page or a padded stride goes
through an intermediate and is copied row by row. Every named platform hits
the free path — `ARGB_8888` is tightly packed, a `CGBitmapContext` takes the
`bytesPerRow` the caller chooses, and `ImageData` is always `width * 4`.

*Not* the same as making the whole path zero-copy, and iOS is where the
difference bites — see **What Swift asks for that JNI did not** below.
`AndroidBitmap_lockPixels` hands back the Bitmap's own backing store, so
writing there is writing what gets composited. A `CGBitmapContext` is a
buffer the caller owns and CoreAnimation has never heard of; the path from
it to the screen is a second step, and the obvious form of that step
copies.

Shells that rasterize themselves still take `frame()` and the display list;
that path is unchanged and is deliberately *not* in the first C ABI. Three
`DisplayOp` variants are not hard to expose, but nothing needs them yet,
and STABILITY.md's Contract tier means whatever ships holds still. Exposing
them would also drag `cosmic_text::fontdb::ID` across a boundary meant to
name no third-party types — reason enough on its own, and the accessibility
argument that once pulled the other way has been withdrawn (PLATFORM §7).

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

Home: `chapbook_core::input`, Contract tier, no event loop, no I/O.
**Built** — all three, as sketched, with the decisions the sketch did not
make recorded below.

**Reading direction had to become a type before anything could consult it.**
The tap-zone bullet says the policy "depends on reading direction, which is
engine knowledge", and nothing in the workspace had a way to say which one a
book was: `page-progression-direction` was unparsed and `Ltr`/`Rtl` appeared
nowhere in core, epub or reader. So `ReadingDirection` was declared in
`input.rs`, where the only consumer was. RTL is one mirrored coordinate, not
a second set of comparisons, and the test asserts the two directions are
reflections of each other rather than checking each by hand.

**And then it needed an owner, which is a separate problem.** A type with no
producer left the fact with the shell — `TapZones::default()` is `Ltr`, so
every shell would have picked `Ltr`, on every platform, because nothing told
it otherwise. Two mobile shells were about to make that choice
independently and ship it, and the repair afterwards is a reading-direction
toggle in each app's settings: a user-visible API inherited by accident.

So the book owns it now. `Publication::reading_direction` defaults to `Ltr`;
`chapbook-epub` overrides it from rbook's `spine().page_direction()`, where
"no preference" and "ltr" are both `Ltr` because that is what the spec says;
and `Session::reading_direction` hands it to whoever builds the
`TapZones`. Formats with nowhere to declare it inherit the default, which
means manga in a CBZ still reads `Ltr` — there is nothing in the container
to consult and guessing from the images is not that layer's job.

The end-to-end test is the one worth having: the same tap, on the same page
box, resolves to `PrevPage` on `minimal.epub` and `NextPage` on a new
`rtl.epub` fixture, and the third assertion is that `TapZones::default()`
gets it wrong on the RTL book — the bug in the form it would actually have
shipped in.

**`action_at` takes panel coordinates, not page coordinates.** The sketch's
signature is unchanged, but the meaning of `x, y` is the numbers a touch
event actually carries, and the rotation is undone inside via
`PageMetrics::panel_to_page` — which already existed, with a doc comment
saying it is there "so input arrives in the space the page was laid out
in", and which nothing called. A shell drawing to a turned panel would
otherwise have had to know to call it, and the failure mode is a reader
whose page turns are ninety degrees out. That case is a test.

**Modifiers are deliberately absent from `Key`.** Chorded shortcuts are
where an app's own commands live, and an engine that claimed `Ctrl` would
collide with every one of them; the rule is that a shell consults the map
for unmodified presses only. Leaving them out is also the choice that keeps
`KeyMap::action`'s signature addable-to later, which the Contract tier
cares about more than the convenience does.

**`Action` and `Key` are `#[non_exhaustive]`.** Not a decision so much as
the admission that they close only if every reader intent is guessed up
front; bookmarks, search and a jump to the table of contents are plainly
coming. The attribute is what makes those additive later, and adding it
later would itself have been a break, so it goes in while there is one
caller outside the tests. It also matches what `Action::from_name`
returning `Option` already asks of the string path: a host ignores what it
does not recognise.

**Arrow keys are bound logically, not physically**, so `ArrowRight` is the
next page in an RTL book too. The two conventions cannot both be the
default and neither is obviously right; this one at least agrees with what
the tap zones resolve to. Written down because it is the kind of decision
that otherwise gets rediscovered as a bug report.

The vocabulary is short on purpose — every `Key` variant is bound by
`KeyMap::default`, and a test asserts it, because a name at the Contract
tier that means nothing anywhere is surface bought for nothing. Writing the
tests found one defect: a band configured to zero width still claimed the
last column of the page, because the far edge of a band is inclusive and
`>= 1.0 - 0.0` is true at `x == w`. Documented as disabled, behaving as
one-pixel-wide.

**The other half — `Session::apply(Action)` — is done too, and the return
value is where the design work was.** It began as the same
did-anything-move `bool` the navigation verbs give, which already forced
one decision: `adjust_font` clamps to 10–40, so the font actions could not
simply return `true` or a shell would repaint an identical page forever.
It compares the size across the call instead. The step size is the
engine's rather than each shell's, as `FONT_STEP_PX`, so a reader who
changes device finds the same ladder.

**But a bool was the wrong return type, and Android is what proves it.**
A shell needs two answers and can derive neither from the other: *should I
repaint*, and *did I consume this event*. `apply` returns
`ActionOutcome::{Changed, Unchanged, Unhandled}` instead. The forcing case
is concrete rather than theoretical — `KeyMap` binds the volume keys by
default, Android's `onKeyDown` must return `true` to keep an event, and a
shell that forwards a did-anything-move bool draws the system volume
slider over the book on the last page of every one. iOS's responder chain
is the same shape with quieter symptoms.

`Unhandled` is the state a bool could not express, and `Back` is why it
had to be the engine's call rather than each shell's. `ToggleMenu` is
`Unhandled` for the obvious reason — the engine has no chrome — which
deletes the footgun the first version had to document instead. `Back` with
an empty trail is `Unhandled` too, deliberately: whether there is anywhere
to return to is a fact about the back stack, and the bottom of it is
exactly where Android's and iOS's own Back should take over and leave the
reader. That lets a shell forward the gesture unconditionally instead of
shadowing the history to know when not to. `ActionOutcome` is
*exhaustive*, unlike the two enums above — consumed and repainting are two
bits and the fourth corner, declining an event while demanding a repaint,
describes nothing.

**`chapbook-viewer-gtk` is the first shell over it**, and converting it is
what showed the seam pays. Its keyboard handling went from a fourteen-arm
match on GDK keyval names to `engine_key` — a translation function with no
opinions in it — plus `KeyMap`, and it gained the arrow-key and Backspace
bindings it never had by doing nothing. It also gained tap zones, which it
had none of: a click in the left or right third turns a page now, resolved
in `drag_end` when the press neither followed a link nor grew into a
selection. Two places where the shell customizes rather than accepts are
worth noting as the demonstration that the map is a map: it unbinds `m`,
because it has no menu, and it gives `TapZones` an inert middle band for
the same reason.

Converting it also found a small standing bug. The old handler called
`queue_draw` after every bound key; it now redraws only when `apply` says
something moved, so the end of the book stops repainting the same page.
That is the return value the reader has documented since the fbdev shell
got it wrong, finally being used by the shell that was ignoring it.

And the second half of the outcome turned out to have a desktop use as
well, which was not the reason for it: GTK's `Propagation::Stop`/`Proceed`
is the same bit Android's `onKeyDown` returns, so the handler now proceeds
on `Unhandled` instead of swallowing every bound key unconditionally.

Android followed — `ReaderView.kt`'s hardcoded thirds are gone, and what
that swap found is in *What the touchscreen settled* below. What is left
is the winit viewer, which still hand-translates.

**Lifecycle — done.** `save_position()` was the right primitive but not the
right call: a platform needs one that means *you are about to be stopped*.
`suspend()` persists the position — settings and annotations are already
written when they change, so the position is the only lazily-written state —
closes the database, and releases the caches, because a stopped app should
not be holding decoded pages.

Closing rather than flushing is the iOS half: bundled SQLite takes POSIX
advisory locks, and an app still holding one on a file in a shared container
when it suspends is killed by the watchdog. See the iOS section.

The session stays usable afterwards. `onStop` is often followed by `onStart`
with the process still alive, so the next access reopens the library —
which is why `Session` keeps the directory it was given rather than only the
open handle. A session that quietly stopped saving after its first suspend
would lose every position from then on, so that path has a test.

What `suspend()` does *not* do is stop the loader thread: a page already
being decoded finishes.

**Power and memory.** Two calls, both driven by the finding above.
`set_cache_budget(bytes)` is **done** — caches evict least-recently-used
instead of growing, and lowering the budget evicts on the spot rather than
at the next page turn, which is what a memory warning needs.
`release_caches()` is **done** too — everything but the unit on screen, for
`onTrimMemory` and for an e-ink device that has been idle. It is distinct
from lowering the budget on purpose: a budget is a steady-state cap, this is
"right now, as much as you can". Neither is Android-specific — a Kobo has
256 MB — but Android is where it becomes an OOM kill rather than a slow
day.

## The Android spike

**Where the code is.** `crates/chapbook-jni` and `android/` are on main.
What follows is the findings, which are the part worth keeping either way —
and two of them (below) were bugs on main that the spike is how anyone
noticed. The spike is also no longer a spike: see *What became of
`chapbook-jni`*.


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
   for free at this stage. *(Since replaced by
   `SessionConfig::with_library_dir` — which is what the stopgap was
   arguing for.)*
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
   `Source::Reader` and `Format::Guess` to be real. *Done — all five rungs
   are. `openFd` takes a detached `ParcelFileDescriptor`, wraps it as a
   `File` and hands it over as `Source::reader`; the shim is nine lines,
   because `Source::Reader` and `Format::Guess` had already done the work.*

### What rungs 1 and 2 found

**Rung 1 needed no source changes at all.** `cargo ndk -t arm64-v8a -t x86_64
-P 24 build -p chapbook-reader` builds the whole thing, every feature on —
bundled SQLite and `ring` both compile against the NDK's clang without
persuasion. That is better than this document predicted: the two C gaps were
the only ones, and the toolchain closes both.

Then four things the build could not have told us.

**A font source is not merely missing, it is unreachable.** *(Closed — see
"Implemented" at the end of the fonts section.)* `Session::open`
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

**Everything the engine logs is invisible.** *(Closed twice — once for
Rust hosts, when the `log` seam landed and the rewrite verified it from the
device: `android_logger` in the binding, and records arriving under the
crate that emitted them, as in `I/chapbook chapbook_reader: resuming at unit
6 (Exact)`. And once for everyone else, when `cb_set_log_callback` was added
to the C ABI — because a C or Swift host cannot install a Rust backend, so
for them the callback this paragraph asked for is the only option after
all.)* `chapbook-reader`
makes eleven `eprintln!` calls and the workspace has no `log` or `tracing`
facade at all — a deliberate scope choice that works fine for a desktop
shell and stops working at the boundary. "library unavailable", "page N
failed to load", "resuming at unit N": on Android stderr goes nowhere an app
can read. The C ABI needs a log sink, and a callback is the cheap version
of it.

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
SQLite works on Android, the library found somewhere writable under
`filesDir`, and a locator round-tripped through the library. It is also the
check that caught both of the regressions below, which is the argument for
this rung being the cheap one. Watching
the app come back on the unit it was left on — and in the theme it was left
in, since settings persist the same way — is the same result arrived at from
outside.

**The font workaround holds.** 214 faces from `/system/fonts`, rising to 216
on a unit with embedded webfonts, so the webfont path works through the
binding too. *(The workaround is gone; `FontSource::android_system()` is the
same answer as a preset, and reports the same 214 faces with no unresolved
generics.)* Text pages render justified and hyphenated with the publisher's
own faces.

**Nothing the engine printed reached logcat**, as predicted. Not one of the
eleven `eprintln!` diagnostics appeared, which is the finding above confirmed
from the other side rather than a new one. *(Closed. The rewrite installs a
backend and the same diagnostics now arrive tagged and filterable — and the
first thing they were good for was diagnosing the two findings below.)*

What rung 4 did *not* settle: this is x86_64 under an emulator. Nothing here
has run on arm64 silicon, and the panel-side questions PLATFORM cares about —
refresh policy, e-ink waveforms — are as unproven as they were.

### What rung 5 found: the bytes win

`openFd` takes a `ParcelFileDescriptor` the app has `detachFd()`'d, wraps it
in a `File`, and passes `Source::reader`. That is the whole shim; the Rust
half was already built and tested, and this rung only had to confirm it
against a platform that actually withholds the path.

**The format sniffer earns its keep here, twice.** A file pushed with *no
extension at all* — which the system picker lists as an 832 kB "BIN file",
having no idea what it is — opens as the EPUB it is, title and all. So does
an EPUB renamed `timemachine.pdf`, which the picker displays with a red PDF
icon. Both are the `A book is its bytes, not its name` invariant arriving
somewhere it can be observed rather than asserted: on Android the name is
either absent or supplied by whoever wrote the file, and neither is evidence.

**A handle does not reach the library, and the session does not say so.**
That is by design — no path means nothing to fingerprint — and it is
documented on `Source`. What is not obvious from a shell is the *silence*: a
book opened this way restores no position, keeps no annotations, persists no
settings, and `suspend()` on it returns without a word. The demo shows it
plainly, because a book opened from the picker comes back in the default
theme rather than the one the reader left. Closing the gap is the custody
question (persist a bookmark, not a copy) in `docs/PLATFORM.md`; until then a
shell that offers both doors should say which one the reader came through.

### What the rewrite found, which the workarounds had been hiding

Rungs 1–4 were written against an API that was missing five things, and the
workarounds for those were the deliverable. Deleting all of them and
rewriting against the real constructor turned up two more — neither of which
the original spike could have seen, because in each case a workaround had
been standing where the failure would have shown.

**A feature flag lost persistence in silence.** `library` became optional
(the browser and stripped-e-ink profile), and the workspace pins
`chapbook-reader` with `default-features = false`. The spike's dependency
line named the formats it wanted — `["cbz", "pdf"]` — and so, from that
commit onward, built with no library at all. The app opened, laid out,
rendered, navigated, searched and *passed ten of eleven conformance checks*.
It simply remembered nothing, and nothing anywhere said so: the engine warns
when a library will not open, and has nothing to warn about when it was
compiled without one. On a desktop this is a missing bookshelf; on a phone,
where "come back where I left off" is most of what a reader notices, it is
the product. **Any artifact that ships a chosen feature set should be able to
report it at runtime** — one call naming the compiled-in capabilities, which
the C ABI wants anyway and which the demo's status line would have shown.

**A restored position followed the reader into the wrong chapter.** With the
library switched back on, rung 4 regressed: `the end of the book stands
still` failed, and only when the library already held a position. A restored
offset cannot become a page until its unit lays out, so it is held pending
and resolved in `frame()` — against whatever unit the reader had reached by
then, because it did not record which unit it came from. Nothing obliges a
shell to paint before it navigates, and the harness does not, so the offset
landed in a chapter it was never measured against. At the end of a book that
reads as the end moving: `next_page()` refuses, a `frame()` drops the reader
several pages back, and the same turn then succeeds.

Nothing about it is Android. It reproduces on a desktop by running
`examples/conform` twice against one library — the first run leaves the
position the second one restores — and it had been invisible because every
test in the workspace builds a fresh library directory, so no session in the
gate had ever restored anything it did not also write in the same process.
Fixed by pairing both pending landings with the unit they were captured in
and dropping them once the reader leaves it: navigating away supersedes a
restore. **The gate's blind spot is worth more than the bug** — "state
carried across two runs of the same binary" is a whole class this suite
cannot currently reach, and the cheapest fix is a test that runs the harness
twice against one directory.

### What became of `chapbook-jni`

`docs/STABILITY.md` predicted that once the C ABI existed this crate would
"become a thin JNI layer over it, or go away." It did neither, and the
reason generalizes to any future host binding.

**JNI is already a C ABI.** The JVM locates native methods by exported
symbol name with C linkage, and Rust emits those directly with
`#[no_mangle] extern "system"`. So a Rust JNI layer sitting on
`chapbook-ffi` is not one boundary, it is two, back to back, with Rust in
the middle converting in both directions:

- a `jstring` becomes a `String`, then a `CString` — an allocation whose
  only purpose is a NUL terminator for a boundary both sides are already
  past — then a `&str` again inside the ABI;
- a title comes back through the length-probe-then-fill idiom, which exists
  so that a *C* caller never frees Rust memory, and buys nothing at all
  between two Rust crates;
- a `Result<_, ChapbookError>` collapses to a status code plus a
  thread-local string and is then widened back out.

**Writing the glue in C instead removes the round trip**, and was seriously
considered: JNI's own API is C, `GetStringUTFChars` hands back exactly the
`const char *` that `cb_session_open_path` wants, and the AAR would then
consume the same header a third party does. What killed it is that the
benefit is almost entirely a *proof* — "the header works on Android" — and
the proof is four lines of `clang -fsyntax-only`, which `android/build-jni.sh`
now runs for both ABIs, `jni.h` included, so the composition is checked
rather than assumed. Buying that with a second build system, plus a third
hand-written layer to keep in sync across three languages that reference
each other at no point during compilation, is a bad trade — and this spike
already found that a drifted `external fun` name fails only at first call
on a device. It would also swap Rust's `catch_unwind`, real error types and
the `jni` crate's reference handling for hand-written C, where a missed
`ReleaseStringUTFChars` is the ordinary mistake.

So `chapbook-jni` stays a direct Rust binding over `chapbook-reader`. It
leaves the Spike tier, because it is not throwaway any more — it is the
native half of the Android artifact — and it is deliberately *not* a
consumer of `chapbook-ffi`.

**Which leaves the C ABI with one guaranteed consumer instead of two, and
that is the right number.** iOS cannot route around it: Swift speaks C and
nothing else, so the binding there has no alternative and will find every
gap by walking into it. Android had an alternative and a good one. A
boundary is kept honest by the host that has no choice, not by the host that
was talked into using it — and a consumer added for the sake of exercising
an interface tends to exercise the interface it was given rather than the
one it wanted.

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

Rung 1 done, rungs 2-5 not started. This section is the handoff: what a
session on a Mac can take as settled, what is different from Android, and what
only a device can answer.

The larger question this raised — whether the engine is the right shape to
build an Apple app on at all, against leaning on Apple's own frameworks — is
answered in [PLATFORM.md §7](PLATFORM.md). Short version: the pipeline earns
its place on measured numbers and on locators, and the *edges* — OPDS
transport, credentials, file custody — do not. (Credentials have since
been extracted; see PLATFORM §7 for the three decisions that shaped it, all
of them made for this document's boundary rather than for today's caller.)
A finding there used to reach back into this document — that accessibility
is built from the display list, so keeping the list out of the first C ABI
decided v1 could have no screen-reader path. That was wrong on both counts
and PLATFORM §7 now says why: the display list holds glyph indices and no
text at all. Accessibility is deferred and owes this boundary nothing.

**Most of the defect list is not Android's.** The font source being
unreachable, the missing generic-family mappings, the absent log seam, the
unbounded caches, and conformance needing an opener rather than a handle are
all properties of `chapbook-reader`, found on Android and true everywhere.
(All but the last are now fixed; the log seam is the `log` crate, which was
already in the graph, so the engine gained a voice for no new dependency.)
iOS should confirm them in passing, not rediscover them.

**The dependency graph is identical to Linux's.** `cargo tree --target
aarch64-apple-ios` resolves to the same 200 crates as x86_64 Linux, with
nothing added and nothing dropped, so there are no iOS-only dependencies and
no new licence surface to weigh. (Android is 198: it loses `fontconfig-parser`
and `roxmltree`, for the reason below.)

**fontdb has no iOS branch at all** — which reads worse than Android's
problem and is in fact much better; see *Fonts, on every platform* below.
There is not one `target_os = "ios"` in the crate. iOS is `unix` and is
neither `macos` nor `android`, so it falls into the branch marked *Linux* and
goes looking for `/usr/share/fonts/`, `/usr/local/share/fonts/`,
`$HOME/.fonts` and `$HOME/.local/share/fonts` — and, with the `fontconfig`
feature on as it is here, for `/etc/fonts/fonts.conf` first. None of that
exists on iOS. The result is the same empty database Android gets, reached by
a path that believes it is on a desktop, plus two crates of dead weight
compiled in to parse a config file that cannot be there.

What that first reading missed is that the faces iOS does have are reachable
in a single directory, recursively, and that fontdb already scans
recursively. The gap is a `cfg` branch and not a platform. Whether
`/System/Library/Fonts` can be read from inside the sandbox is the one
question left, and still the first thing to check on a device; if it cannot,
the answer is CoreText enumeration or bundled faces, and a font source
carries either.

**The library directory used to accidentally work, in the wrong place.**
`Library::default_dir()` fell back to `$HOME/.local/share/chapbook`, and iOS
*does* set `HOME`, to the app's container. So unlike Android it needed no
environment variable to function — it just put the database somewhere Apple
would not, outside `Library/Application Support`, with the backup and
purgeability implications that carry. Working by accident was worth noticing
precisely because it would not have raised an error.

Fixed from both ends. `default_dir()` now has a per-platform arm — XDG on
Linux, `Library/Application Support` on macOS, `%APPDATA%` on Windows — and
returns an *error* anywhere with no convention rather than a relative path,
so iOS and Android no longer resolve to somewhere plausible-looking by
accident. And `SessionConfig::with_library_dir` lets a host name its own
container, which is the only correct answer on a sandboxed platform: the
old code was reachable solely through an environment variable, which a
phone has no way to set.

**iOS is the more honest test of the C ABI.** Android went through JNI, which
is a layer of its own with its own conventions; Swift consumes a C header
directly, so there is no equivalent cushion. That makes this spike a better
proof of the boundary and a worse place to be sloppy about it.

The same discipline applies as on Android, and for the same reason: **do not
call the spike crate the C ABI crate.** It will contain `extern "C"`
functions because it has no choice, but a header generated and depended on
before the shape is fixed is exactly the freeze this document exists to
avoid. Name it a spike, keep cbindgen out of it, and throw it away.

### Checked back from Linux

The Mac session changed three things that belong to every platform, so they
were re-run on the Linux box that the Android rungs were done on. All three
hold from this side:

- **The two recalibrated session tests pass here.** That is the claim worth
  checking, because those assertions were originally calibrated against *this
  machine's* installed fonts — so a repair that merely re-pinned them to a Mac
  would have shown up as a failure here. Deriving the offsets from
  `search_unit` instead of writing them down makes them portable in both
  directions, not just the new one.
- **The GTK viewer is unaffected on Linux.** `cargo tree` resolves 280 crates
  for `x86_64-unknown-linux-gnu` and exactly one — itself — for
  `aarch64-apple-darwin`, and the crate still builds here.
- **The full gate is green on Linux:** 57 test binaries, `fmt` and
  `clippy -D warnings` clean, exit 0.

### What rung 1 found: it builds, and the workspace around it does not

Rung 1 is done, and it needed no persuasion. `cargo build -p chapbook-reader`
succeeds for both `aarch64-apple-ios` and `aarch64-apple-ios-sim` — no flags,
no patches, no feature changes — against Xcode 26.6 and the iOS 26.5 device
and simulator SDKs. `ring` builds and bundled SQLite builds, which were the
two Xcode-clang questions below; they are answered *yes at compile time*, and
still unanswered at run time inside a sandbox.

The dependency claim holds, and this machine can prove it more sharply than
Linux could. `aarch64-apple-ios` and `x86_64-unknown-linux-gnu` resolve
`chapbook-reader` to identical crate sets — not comparable, identical. On the
same Mac, `aarch64-apple-darwin` drops exactly `fontconfig-parser` and
`roxmltree`: the same two Android loses. macOS has a fontdb branch and iOS
does not, on Apple's own silicon, through Apple's own SDK.

Three things about the host, none of them iOS-specific, all of them waiting
for whoever opens this repo on a Mac.

**`cargo test --workspace` did not run on macOS at all** — fixed here.
`chapbook-viewer-gtk` reaches GTK4 through `gtk4-sys`, which probes for
`gtk4.pc` with `pkg-config`, and the build failed before a single test ran. So
the first thing a Mac session did was watch the gate CONTRIBUTING publishes
fail for a reason that had nothing to do with it. The crate is now Linux-only
by construction: every dependency sits under
`[target.'cfg(target_os = "linux")'.dependencies]`, the viewer proper moved to
`src/linux.rs`, and off Linux `main.rs` is a stub that explains itself. Linux
resolves all four dependencies as before; macOS resolves none. The published
gate now runs unmodified on a Mac.

**Two reader tests fail on macOS, and they were failing before this branch
existed.** `epub_session_renders_navigates_and_selects` and
`frames_report_what_changed` fail identically at `c73cbe5`, so nothing here
caused them. `Session::open` calls `chapbook_layout::system_font_system()` —
the host's installed fonts — while `chapbook-layout/src/fonts.rs` says in its
own doc comment that layout depends on the fonts available and so tests must
use `fixture_font_system` and "never the host's font collection." The CLI and
the layout tests obey it; the session does not. Different metrics wrap
differently, so a press at (100, 70) lands on a different glyph, and the page
count shifts far enough that `next_page()` becomes a no-op and `Selection`
outranks the `PageTurn` meant to beat it.

This is the unreachable-font-source defect wearing a third face, and the worst
of the three. Android fails loudly, with an empty database and blank pages.
iOS will fail the same loud way. macOS fails *quietly*, as two assertions about
selected text that read like selection bugs, on a machine where fonts are the
last thing anyone would suspect.

**And the expectation is pinned to one machine, not to this repository.**
Pointing `Session` at `fixture_font_system(fixtures/fonts, "Crimson Text")` —
the repository's own vendored faces, the ones the CLI and the layout tests
already use — does *not* turn the tests green. Three font sets give three
different answers: the Mac's host collection, Crimson Text, and an empty
database each select a slightly different run of text, and none is the run the
assertion wants. What `starts_with("e Illustrated Chapter")` records is the
metrics of whatever happened to be installed on the Linux box where it was
written. That is not a fixture, it is a fingerprint.

**Fixed by deleting the fingerprint rather than re-taking it.** Recalibrating
against one font set would only move the pin. So the two assertions were split
by what they actually prove. Hit-testing is a function of the fonts, and its
half now asserts only the shape of the result — the selection runs from the
heading into the paragraph below, ordered, with no line break. The exact-text
claim moved onto `select_range`, whose offsets come from `search_unit` rather
than being written down: search and selection share the unit's locator space,
and neither has any idea which fonts laid the page out. That makes the
strongest assertion in the test the portable one — a range spanning the source
line break between `underlined link` and `and some` comes back as exactly
`"underlined link and some"`, which is precisely the whitespace collapse the
old prose claimed and the old offsets only implied. The intent test walks
back to the first page before asking for a turn, because how many pages this
book has is a font question, and whether page 1 had a successor was the thing
that broke.

Both pass under the Mac's host collection and under Crimson Text, and the
whole workspace gate is now green on this machine: 57 test binaries, fmt and
`clippy -D warnings` clean.

**What that does not fix, measured.** Point the session at an empty database —
the Android and iOS condition — and **seven** of the thirty session tests
fail, these two among them. Not selection alone: the jump-offset test, the
search-and-navigate test, the link-and-TOC trail test and both settings tests
go with them, because a book with no faces has one page and nothing to
navigate. That is the number to beat, and no test change can beat
it. It is what the font source is for.

**The toolchain pin does not carry the targets.** `rust-toolchain.toml` pins
the channel and its components but declares no `targets`, so both iOS targets
have to be added by hand before rung 1 will start. The same unguessable
prerequisite as the NDK, and cheaper to fix.

One note for rung 4, from Android's rung 4: the harness conformed on a device
with an empty font database — ten passed, one skipped, none failed, against
Moby-Dick, with no faces loaded at all. So conformance does not assert on text
layout, and iOS conforming will not distinguish a working font path from an
absent one. Rung 4 green means less there than it looks like it means.

### What rung 2 found: it binds, and the header speaks Swift with an accent

Rung 2 is done, on an iPhone 15 Pro simulator against the iOS 26.5
runtime: every entry point the reader loop needs — open, error strings,
metrics, title, the whole input model, `render_into`, suspend — called
from Swift through the checked-in header and answering correctly, with
the device slice link-checked beside it. What lives in `ios/spike` is a
module map pointing at the real `chapbook.h`, one `main.swift`, and
`run.sh`; no copy of the header, no cbindgen, no generated binding. The
ladder's own description turned out stale in a good way: it asks for a
throwaway crate of `extern "C"` functions, and none was needed, because
the real ABI landed before this rung and already carries the
`catch_unwind` discipline. The throwaway part is the Swift.

**The one real defect: cbindgen's `cpp_compat` header imports every enum
twice.** The C branch declares a plain `enum` for the constants and a
fixed-width typedef for the signatures, and Swift's ClangImporter sees
two *different types with the same name* — the diagnostic is the
memorable `cannot convert value of type 'cb_log_level' to expected
argument type 'cb_log_level' (aka 'Swift.Int32')`. Every constant
reaches signature-land through `.rawValue` plus an integer-width
conversion, which the spike does once at the top of the file. Livable
for a spike; the real Swift package will want a one-time constants shim,
or the header can emit C23 fixed-underlying-type enums and let the
typedef name the enum — a cbindgen question to settle before the Swift
package exists, not before.

**`Send`-not-`Sync` is now compiler-checked rather than argued.** The
`Session` wrapper is a `final class` that is deliberately not
`Sendable` — marking it `@unchecked Sendable` would be claiming the
`Sync` the Rust side does not have. Compiled under `-swift-version 6`,
the thread-move test only builds because `Task.detached` takes a
`sending` closure and region isolation can prove the handle leaves its
region for good; adding a use of the session after the send is a compile
error. The boundary section predicted the isolation model would take
this shape "without persuasion", and it did — but only for a *local*
handle. A top-level `let` is a global in Swift's eyes, and a global can
never be proven sent, which is a real constraint on how a shell scopes
its session and worth knowing before one is designed around a singleton.

**Two toolchain traps, both cheap once written down.** `swiftc -sdk`
compiles Swift against the right SDK but leaves clang's *linker* on the
macOS sysroot — a warning, and the produced binary is correct (verified
`platform 7` in its load commands), but the fix is spelling it
`xcrun -sdk iphonesimulator swiftc`. And a Mac that has never completed
Xcode's first launch is a trap with no error message: `simctl` *hangs*
rather than failing, and a headless `xcodebuild -runFirstLaunch` blocks
forever inside `AuthorizationCopyRights` waiting for a password dialog
that cannot appear — it needs an admin at a GUI, once. The same species
of unguessable prerequisite as the NDK and the missing toolchain
targets, and this machine hit it live: the sudden cure for a wedged
`simctl` was completing first launch, not restarting CoreSimulator.

**What the simulator run answered beyond binding.** The log seam works
from inside the runtime, and the first record it delivered was the
right one: the deliberate no-library open produced `reading without a
library: no default library location on this platform` — the exact
behavior the library-directory fix was built for, arriving through a C
callback on an Apple runtime. Bundled SQLite opened, wrote and
suspended a real library in a container directory under the simulator.
The input model — LTR direction off the book, thirds resolving, the
page-zero `prev` returning *unchanged-but-consumed* — replayed from
Swift exactly as it ran on the Android device. And `render_into` filled
a 1200x1600 buffer at `dpi_scale` 2; what the pixels look like, and the
channel-order check, stay rung 3's questions.

### What Swift asks for that JNI did not

The boundary above was designed with Android in hand. Most of it transfers,
and three parts of it turn out to be worth *more* here than the reasoning
that produced them — see the end of this section. These are the places where
Swift and the platform ask for something JNI never did.

**Device and simulator cannot be one library. This is settled, not a
choice.** The section below used to offer `staticlib` plus a module map as
the simple path and an XCFramework as the packaged one. They are not
alternatives. Both slices are `arm64`, and they differ only in a Mach-O load
command — the device object carries `LC_VERSION_MIN_IPHONEOS`, the simulator
object carries `LC_BUILD_VERSION` with `platform 7`. `lipo` refuses to put
them in one file, in as many words:

```
have the same architectures (arm64) and can't be in the same fat output file
```

So an artifact that builds against a simulator *and* a device is an
XCFramework by necessity, with a slice each — three, if Intel Macs are still
a target, adding `x86_64-apple-ios`. Android's `cdylib` shape does not
decide this; it has no analogue of it.

**The pixel path costs one copy more than Android's, and the naive form
costs two.** `AndroidBitmap_lockPixels` returns the Bitmap's own backing
store: `render_into` there writes the pixels that get composited. iOS has no
equivalent handle on a layer's storage. A `CGBitmapContext` built over a
caller-supplied buffer takes `render_into` perfectly well, but it is a
private buffer, and `CGBitmapContextCreateImage` copies on the way to
anything that can be displayed.

The zero-copy form exists and is worth naming before a spike writes the easy
one: build a `CGImage` directly over the engine's buffer with a
`CGDataProvider` and a release callback, and hand that to `CALayer.contents`.
Then the engine's pixels are the layer's pixels, with the buffer's lifetime
tied to the provider. Rung 3 should do it that way and say which it measured,
because "the same as Android" is the one answer that is not available.

**A source can be legal, named, and not yet readable.** No `content://` URI
teaches this. A file picked from Files or iCloud Drive may be a placeholder
that has not been downloaded, and opening it fails or blocks until it is.
Security-scoped access is revocable and does not survive relaunch: the
bookmark has to be re-resolved *before* the session is constructed. Since the
library keys positions by book, a shell that stores the position but not the
bookmark reopens to a book it can no longer find. `Source::Reader` is the
right mechanism and none of this changes it — what changes is that on iOS,
acquiring the reader is a step that can fail for reasons the engine must not
try to interpret.

**`suspend()` has to release locks, not just persist.** The lifecycle section
justifies `suspend()` with Android's `onStop`. iOS adds a requirement of a
different kind. Bundled SQLite uses POSIX advisory locking — `F_SETLK` and
`flock` across 25 sites, with `fstat` at 21 — and an app holding a lock on a
file in a *shared* container when it suspends is killed by the watchdog with
`0xdead10cc`. Inside the app's own private container this does not arise. It
arises the moment there is a share extension or a widget, because those mean
an app group, and an app group means a shared container. That makes it a
constraint on where the database lives, decided now, rather than a bug found
later. The engine side of that is now in place: `with_library_dir` takes
whatever container the app group resolves to, and there is no `$HOME`
fallback left to land in by accident.

**CoreText enumeration is a narrower fallback than it sounds.** If
`/System/Library/Fonts` proves unreadable from the sandbox, "CoreText
enumeration or bundled faces" reads as two options. Enumeration yields names
and descriptors. Getting *bytes* that fontdb can load means either
`kCTFontURLAttribute` — a URL back into the directory that could not be read
— or reassembling an sfnt table by table through `CTFontCopyTable`. If the
directory is closed, bundled faces is effectively the only answer, and the
font source has to carry them.

**The privacy manifest has no Android counterpart.** An XCFramework
distributed for others to embed wants a `PrivacyInfo.xcprivacy` inside it,
and bundled SQLite's `fstat` and `statfs` calls fall under Apple's
required-reason categories for file timestamps and disk space. This should be
checked against Apple's current list at submission rather than taken from
here, but it is a shipping requirement with no Android twin, and it is much
cheaper to know before review than during it.

**What Android taught that should not be carried over.** The
`#[link(name = "jnigraphics")]` trap — a green build, a produced `.so`, and
three `UND` symbols that fail only on a device — has no iOS twin. Static
linking turns a missing framework into a link error at app build time. Do not
port the link-time check; there is nothing here for it to catch.

**Three decisions that are better here than the argument that made them.**
The boundary chose `Send`-not-`Sync` handles from a Rust finding, and it maps
exactly onto Swift 6's isolation model: "movable, not shareable" is what an
`actor` or a `@MainActor`-confined wrapper expresses, so the handle's Rust
property and its Swift type agree without persuasion. Caller-allocated
strings were chosen to avoid cross-allocator frees; Swift has no natural way
to free Rust's memory at all, so the absence of `cb_free_string` removes a
defect class here rather than merely a common defect. And the waker's
`void* user` is load-bearing rather than decorative, because a Swift C
function pointer cannot capture context: the callback must be a global
function that recovers its object through `Unmanaged.fromOpaque(user)` and
hops to the main actor. Which is precisely why the header must say, as the
boundary section insists, that **it fires on the loader thread**.

### What only a Mac can answer

- Whether bundled SQLite *runs* under the iOS sandbox. It compiles, as does
  `ring` — see rung 1 above — but a database opening in a container is a
  different question from a database linking. *Half-answered at rung 2: it
  opened, wrote and suspended inside the simulator runtime. `simctl spawn`
  is not App Sandbox confinement, so the device half stands.*
- The pixel path's *channel order*, which is still a device question even
  though its shape is settled above. A `CGBitmapContext` with
  `kCGImageAlphaPremultipliedLast` and `kCGBitmapByteOrder32Big` should be
  premultiplied RGBA and therefore a straight copy, as
  `AndroidBitmap_lockPixels` turned out to be — but run the same test that
  settled it there, which is the sepia theme and not black text, because
  black on white cannot tell RGBA from BGRA.
- Whether `/System/Library/Fonts` is readable from a sandboxed app. *The
  simulator runtime has the directory, populated and listable — see rung 2
  — but with the same caveat: a spawned process is not a sandboxed app.*
- Simulator versus device. Android's rungs ran on an x86_64 emulator and
  therefore proved nothing about arm64 silicon; a Mac can close that gap for
  Apple and, with a physical Android device, for Android too.

### The ladder, again

1. It builds: `cargo build --target aarch64-apple-ios-sim` and `-ios`.
   **Done** — see rung 1 above.
2. It binds: a spike crate of `extern "C"` functions, hand-declared in Swift,
   each wrapping its body in `catch_unwind`. An isolation decision for the
   handle wrapper belongs here too, and it is `Send`-not-`Sync` spelled in
   Swift. **Done, on a simulator** — and no spike crate was needed, because
   the real ABI landed first and already carries the `catch_unwind`
   discipline; what was hand-written is Swift over the checked-in header.
   See *What rung 2 found* above.
3. It draws: `render_into`, tap zones, sepia for the channel check. Do it
   both ways and record the difference — `CGBitmapContext` then
   `CGBitmapContextCreateImage`, against a `CGImage` built over the engine's
   buffer with a `CGDataProvider` and handed to `CALayer.contents`. The
   second is the one that matches what Android got for free.
4. It conforms: the harness, on a simulator and then on a device. Remember
   that Android conformed with an empty font database, so a green run here
   proves less than it looks like it proves.
5. It takes a security-scoped bookmark — the iOS twin of Android's content
   URI, and the other half of the argument for typed sources. Resolve it
   from cold at launch, not only from the picker, because that is the path
   that fails.

### What the touchscreen settled

The argument for putting the input model on Android before the C ABI was
that a touchscreen is where tap zones can be judged and a desktop shell is
not. That turned out to be true in a narrow, checkable way, and the check
is worth recording because it is the one a spike exists to produce.

`ReaderView` had `val third = width / 3f` and called `prevPage`/`nextPage`
by hand. It now asks the engine, and three things came out of the swap.

**The coordinates are not the ones the event carries.** `MotionEvent.x` is
in view pixels; `setMetrics` was given the page box in logical units. A
shell that forwards the raw event puts every tap in the last band on a 3x
screen, and nothing errors — the tap simply always means "next page". The
division by density is one line and is the sort of line a second shell
would have got wrong independently, so it is in `SHELLS.md` and in the
JNI doc comment rather than only here.

**Actions cross the boundary as their names.** `Action` is
`#[non_exhaustive]` and Kotlin has no way to notice it being reordered, so
`tapAction` returns `"next-page"` and `applyAction` takes it back. The
allocation is charged per keypress, at human rates. Kotlin never has to
*interpret* an action — it hands back what it was given and reads the
outcome — so the only enum mirrored across the boundary is
`ActionOutcome`, which is three closed values. `Action::name`/`from_name`
existed for exactly this and until now had no consumer.

Android keycodes go the other way and are translated in Rust, which looks
backwards until you ask which half is stable: Android can never renumber
`KEYCODE_VOLUME_UP` without breaking every app on the platform, and an
ordinal invented in this workspace can move in any commit. The mapping
that is safe to hardcode is the one hardcoded.

**And `ActionOutcome` earned itself on the device.** The volume keys are
bound to page turns by `KeyMap::default`, and Android's `onKeyDown` must
return `true` to keep an event. On a fresh install, at unit 0 page 0,
volume-up dispatches `prev-page`, the position does not move, and the
reader keeps window focus with no `com.android.systemui` window in the
dump. That is `Unchanged` — nothing to repaint, event still consumed —
and it is exactly the case where the `bool` this used to return would
have said "false", the shell would have forwarded it, and Android would
have drawn its volume slider over the book. It is also why `onKeyUp` has
to claim the key without acting on it: consuming only the down still lets
the system act, and acting on both turns two pages per press.

## Fonts, on every platform

The font defect is reported three times above — Android's empty database, the
generic families that resolve to desktop names, and the two macOS session
tests — and they are one defect seen from three angles. It is worth setting
out whole, because once it is whole the fix is smaller than three reports
make it sound, and two of the three platforms turn out to be nearly free.

**`FontSystem::new()` does three independent things, and this codebase has
only ever noticed one.** The first is *which faces exist*: `load_system_fonts()`
and its per-target directory scans. The second is *what the five CSS generics
mean*: `serif`, `sans-serif`, `monospace`, `cursive`, `fantasy`. The third is
*which family is tried when the chosen face has no glyph for a character* —
cosmic-text's script fallback. A database can succeed completely at any one
of the three and fail completely at the others, and nothing in the type
system distinguishes them.

An earlier revision of this section counted two. The third was found while
checking the other two against cosmic-text 0.19's source, and it is the one
that decides whether a Greek epigraph renders or comes out as boxes.

**The generics are worse than "Windows defaults", and the earlier reading of
them was wrong.** `Database::new()` does set the five to `Times New Roman`,
`Arial`, `Courier New`, `Comic Sans MS` and `Impact` (`Papyrus` on macOS).
But `FontSystem::new()` does not stop there. It calls `new_with_fonts`, which
runs `load_system_fonts()` — on Linux that is `load_fontconfig()`, reading
fontconfig's `<alias>` rules — and then, unconditionally and on every target,
overwrites three of them:

```rust
db.set_monospace_family("Noto Sans Mono");
db.set_sans_serif_family("Open Sans");
db.set_serif_family("DejaVu Serif");
```

So fontconfig's answers for serif, sans-serif and monospace are computed and
then thrown away, and the two it is allowed to keep are the two nobody
checks. Measured on this Linux dev box, all three stages:

| generic | `Database::new()` | after fontconfig | after `FontSystem::new()` |
|---|---|---|---|
| serif | Times New Roman | **FreeSerif** (present) | DejaVu Serif |
| sans-serif | Arial | **FreeSans** (present) | Open Sans |
| monospace | Courier New | **FreeMono** (present) | Noto Sans Mono |
| cursive | Comic Sans MS | IranNastaliq — **absent from the database** | IranNastaliq |
| fantasy | Impact | Homa — **absent from the database** | Homa |

Two things fall out of that, and neither was in the previous reading. The
three names chapbook actually uses are cosmic-text's hardcoded Linux-flavoured
guesses on *every* platform, macOS and Windows included, rather than anything
the host said. And on this machine `font-family: cursive` resolves to a
Persian nastaliq family that is not installed — fontconfig's alias chain
names it, fontdb records the name, and nothing checks that a face exists. A
publisher styling a pull-quote `cursive` gets no match and silently falls
through to fallback.

The lesson is not "fontconfig is bad". It is that **a generic family name is
a string nobody validates**, on any platform, and that the one target this
project assumed was handled is handled the least carefully of all.

| target | faces | generics | script fallback |
|---|---|---|---|
| Windows | `%SYSTEMROOT%\Fonts` | native, correct | `windows.rs` |
| Linux, `fontconfig` on | via fontconfig | 3 clobbered, 2 name absent families | `unix.rs` — matches |
| Linux, `fontconfig` off | four known dirs | 3 clobbered, 2 MS names | `unix.rs` — matches |
| macOS | four dirs, 370 faces | 3 clobbered to families macOS lacks | `macos.rs` |
| iOS | **none — no branch** | would be 4 of 5, by luck | `unix.rs` — **wrong names** |
| Android | **none — no branch** | 0 of 5 | `other.rs` — **empty** |
| wasm32 | none | 0 of 5 | `other.rs` — **empty** |

Linux is the only target where all three axes are handled, and two of them
are handled by a dependency that exists on no other target. That is the shape
behind every font surprise in this document: chapbook was written on the one
platform where fonts are somebody else's problem, and `fontconfig` was quietly
doing two thirds of the work that a font source will have to do explicitly.

### The third axis, and why Android's workaround is not enough

`cosmic-text/src/font/fallback/mod.rs:13-27` selects a fallback module by
`cfg`, and the selector for the empty one is
`not(any(all(unix, not(target_os = "android")), target_os = "windows"))`.
Android is unix, so `all(unix, not(android))` is false and Android lands in
`other.rs`, where `common_fallback()`, `forbidden_fallback()` and
`script_fallback()` each return `&[]`. iOS is unix and is neither macOS nor
Android, so it lands in `unix.rs` alongside Linux, whose list opens with
`Noto Sans`, `DejaVu Sans` and `FreeSans` — three families no iOS device has.

The consequence for Android is sharper than "0 of 5 generics", and it is not
fixed by the thing rung 3 did. The workaround loads 214 faces from
`/system/fonts`, which is essentially complete Noto coverage for every script
a book might use — and cosmic-text will never reach for any of it, because
loading a face and *falling back to* a face are different mechanisms and only
the first was addressed. A book with a Cyrillic name, a Greek quotation, a
CJK title or an emoji renders tofu on a device that has the glyph sitting in
its own font directory.

**This one needs no upstream patch.** Unlike fontdb's missing iOS `cfg`
branch, cosmic-text already takes the list as a parameter:
`FontSystem::new_with_locale_and_db_and_fallback(locale, db, impl Fallback)`
is public in 0.19. The fix is ours to make in-tree, today.

### The third axis is also a hole in the test gate

*(Closed, with the rest.)* `chapbook-layout/src/fonts.rs` opened by saying
layout results depend on the fonts available, so tests and goldens use a fixed
directory of vendored fonts and "never the host's font collection".
`fixture_font_system` then built its pinned `Database` and handed it to
`new_with_locale_and_db` — which supplies `PlatformFallback`. So the
deterministic font system was deterministic on two axes and host-branched on
the third.

It is latent rather than live: the only non-ASCII in the xhtml fixtures is
`—`, `“`, `”`, `é` and `ï`, all of which Crimson Text covers, so no fixture
currently reaches fallback at all. It goes off the first time somebody adds a
CJK or Greek fixture, and it will go off as a golden diff on one platform
only, which is the worst way to find out. Rule 5 below does not close it,
because rule 5 was written against the two-axis model.

### iOS is the cheap one, and this document said the opposite

The handoff above called fontdb's missing iOS branch "worse than Android's
problem." That was wrong, and an iOS 18.5 simulator runtime says why. Three
things, from a real iOS filesystem:

`/System/Library/Fonts` on iOS holds **no files at its top level.** It is all
subdirectories — `Core/`, `CoreAddition/`, `AppFonts/`, `WebFonts/`,
`LanguageSupport/`, `UnicodeSupport/`. macOS is flat there; iOS is not, which
is exactly the shape that would defeat a naive scan.

It does not defeat fontdb's. `load_fonts_dir_impl` recurses into every
`is_dir()` entry it meets, so one path picks up all **265** face files.

And the Microsoft names are present: `Core/TimesNewRoman.ttf`,
`WebFonts/Arial.ttf`, `Core/CourierNew.ttf` and `WebFonts/Impact.ttf`, four
faces each for the first three. Only `Comic Sans MS` is missing, so `cursive`
is the single generic that would not resolve.

So iOS's *face* problem is **one missing `cfg` branch**, not a platform
limitation. Add `target_os = "ios"` loading `/System/Library/Fonts`, and 265
faces appear and four of five generics resolve by the same accident that has
been keeping macOS working all along. It is a six-line patch worth sending
upstream whether or not we choose to depend on it landing.

Two of three axes, then. The third still fails: iOS takes `unix.rs`'s
fallback list and will chase `Noto Sans`, `DejaVu Sans` and `FreeSans`
through a database that contains none of them. iOS's real fallbacks are
`Helvetica`, `PingFang SC`, `Apple Color Emoji` and friends, and no `cfg`
branch upstream supplies them. This is the one place iOS is not cheaper than
Android — but since the list is a constructor parameter rather than a
`cfg`, both are the same fix.

One question survives, and it is now the only iOS font question: whether a
sandboxed app can read `/System/Library/Fonts` at all. A simulator runs
without the container restrictions, so this file cannot answer it and a
device must. If the answer is no, the fallbacks are CoreText enumeration or
bundled faces — and a font source carries either.

### macOS is already fine, and its two red tests are not a font-source bug

370 faces load, and all five generics resolve, because macOS ships the whole
Microsoft core set alongside its own. Nothing about the font *source* is
broken there. What is broken is that the session takes the host's fonts at
all, so the two session assertions are pinned to one machine's collection —
and, as recorded above, the repository's own Crimson Text does not reproduce
them either. macOS needs no font work. It needs those two tests moved off
the host, and then recalibrated once against whatever they are moved onto.

### Android is the real one

Android is the only target where all three axes genuinely fail and none fails
by accident. The faces are in `/system/fonts`, which nothing scans; the
families there are `Noto Serif` and `Roboto`, which no Microsoft default
names; and the fallback lists are empty, so even a correctly loaded database
is unreachable for any character outside the selected face. It is also the
only target where the platform will not meet us halfway, and so it is the one
that sets the shape of the API.

### The cost, reproduced on a second platform

The seven-of-thirty measurement was taken on a Mac. It reproduced exactly on
Linux: patching the old `system_font_system` to hand back an empty
`fontdb::Database` — Android's condition — took `chapbook-reader`'s session
suite to 23 passed, 7 failed, the same seven. So it is a property of the
engine and not of anyone's host, which is what makes it a number worth
quoting.

That experiment cannot be repeated as written, because the function it
patched no longer exists: a source that resolves to zero faces is now an
error at construction. The seven failures are what the error message
describes.

The names say more than the count:

```
a_jump_remembers_the_offset_it_left
a_settings_change_relayouts_and_keeps_the_place
external_links_are_not_a_reading_position
frames_report_what_changed
links_and_the_toc_both_navigate_and_the_trail_comes_back
search_finds_hits_that_navigate_and_select
settings_survive_a_restart_and_can_be_overridden_per_book
```

Not one of them is about selection, and not one is about how the page looks.
They are navigation, links, the table of contents, search, and settings
persistence. A book with no faces paginates to a single page, and everything
that depends on there being somewhere to go fails behind it. That is the
argument for treating the font source as load-bearing rather than as a
rendering nicety: on a platform with no fonts, chapbook is not an ereader
that looks wrong, it is an ereader that cannot move.

### The shape

Four things are collapsed into one call today: where faces come from, what
the generics mean, what happens when a glyph is missing, and whether layout
may depend on the host at all. Separate them.

```rust
pub struct FontSource {
    faces: Vec<Faces>,
    generics: Generics,    // no Default — see rule 2
    fallback: Fallbacks,   // no Default — see rule 3
    locale: String,
}

pub enum Faces {
    Dir(PathBuf),        // Android /system/fonts, iOS /System/Library/Fonts
    Bytes(Arc<[u8]>),    // bundled, or handed over by CoreText
    Host,                // explicit opt-in to fontdb's per-target guess
}

pub struct Generics { serif, sans_serif, monospace, cursive, fantasy: String }

pub struct Fallbacks {
    common: Vec<String>,                  // tried for any missing glyph
    per_script: Vec<(ScriptTag, Vec<String>)>,
    forbidden: Vec<String>,               // never fall back to these
}
```

**It belongs in `chapbook-core`, at Contract tier.** It appears in the
session constructor's signature, so every shell has to name it — which is the
Contract test, the same one `Panel` passes. It costs core no new dependency:
`PathBuf`, `Arc<[u8]>`, `String`. Core describes the source;
`chapbook-layout` realizes it into a `FontSystem`. That is also the split
that lets it cross the C ABI, where Android hands over a directory path and
iOS hands over bytes.

**`Fallbacks` is data, not a trait.** cosmic-text's `Fallback` returns
`&'static [&'static str]`, which is fine for a list compiled in and wrong for
one a Java or Swift caller assembles at runtime. Core holds owned `Vec<String>`
and layout wraps it in an adapter implementing `Fallback`. Small, but it is
the difference between a design that survives the FFI and one that has to be
redone at it.

Six rules, each one bought with a failure recorded above.

1. **A mandatory constructor argument, not a setter.** Today the only door is
   `paint_resources()`, which reaches through a *paint* accessor to configure
   fonts before anything is painted. It has to be mandatory rather than
   defaulted because the failure is silent: a fontless session lays out,
   renders, paints and *conforms*.
2. **`Generics` gets no `Default`, and all five fields are required.** This
   is the rule that turns rung 3's second finding into a compile error. No
   caller can accidentally inherit `Times New Roman`.
3. **`Fallbacks` gets no `Default` either.** Same argument, newer finding:
   the platform default is empty on Android and wrong on iOS, and an empty
   fallback list fails as tofu rather than as an error. A caller that
   genuinely wants none writes `Fallbacks::none()` and can be found by grep.
4. **Zero faces is an error, not a state.** The constructor returns `Err`
   when a source resolves empty, so Android's blank pages and iOS's blank
   pages both become one message at construction.
5. **Nothing calls `load_system_fonts()` implicitly.** Only `Faces::Host`
   does, and the name tells the caller what they bought — host-dependent
   layout, including host-dependent tests.
6. **Tests, goldens and conformance take an embedded source, never `Host`,
   and pin all three axes.** Pinning faces alone was what
   `fixture_font_system` did, and it left the gate meaning different things
   on Linux, on a Mac and on a device.

Rule 6 was expected to cost a one-time recalibration, because pointing the
session at Crimson Text had been tried and gave a third answer rather than
the expected one. It did not, and the reason is worth recording: the two
pinned assertions had already been fixed the better way — by deleting the
fingerprint rather than re-taking it, taking their offsets from `search_unit`
instead of writing them down. So when the suite was moved onto an embedded
source, all thirty tests passed unchanged. Fixing the assertion and fixing
the font source were two halves of the same defect, and doing the harder
half first made the second free.

### The direction nobody has designed: reading it back

`FontSource` as written is input-only, and PLATFORM §2 records font-family
selection as the one settings feature still missing, "which needs a
font-enumeration story before it needs an API". That story is this object
read backwards:

```rust
impl Session { pub fn families(&self) -> Vec<FamilyName>; }
```

It is nearly free once the source is explicit — the database is already
built and already enumerable — and it is the only thing standing between
`ReadingSettings` and a font-family field. Worth landing in the same pass,
because a shell that can supply fonts and cannot list them is an odd
half-API to ship.

Not part of this: Dynamic Type on iOS and `fontScale` on Android are
`base_font_px`, which `set_settings` already reaches. They are a shell
responsibility and they are easy to file under "fonts" by mistake.

### Implemented

`FontSource` landed as described, with one addition the design did not
anticipate.

`chapbook_core::font` holds the description — `FontSource`, `Faces`,
`Generics`, `Fallbacks`, `ScriptTag` — and costs core no new dependency:
paths, bytes and strings, nothing from cosmic-text or fontdb. Contract tier,
because it is in the session constructor's signature.
`chapbook_layout::build_font_system` realizes it, and is now the only thing
in the workspace that touches `fontdb::Database`. `system_font_system` and
`fixture_font_system` are gone; `Session::open(source, fonts)` takes the
source as a required second argument.

**The addition is `FontReport`.** Every failure in this section is silent,
and rule 4 only catches the loudest one — zero faces. A generic that names
a family nothing carries fails quieter still: no error, no blank page, just
a `font-family: cursive` that matches nothing. So realizing a source also
returns what it produced, and `Session::font_report()` keeps it. On the
Linux box this was written on it says exactly what the measurement above
predicted:

```
2382 faces; cursive -> "IranNastaliq" not installed; fantasy -> "Homa" not installed
```

Printing that once at startup is the difference between diagnosing a device
and guessing at it.

`Session::font_families()` came with it, per *the direction nobody has
designed* above: sorted, deduplicated, and inclusive of families a book
registered through its own `@font-face` rules. That is PLATFORM §2's
font-enumeration story, so font-family selection is now blocked on nothing
but a field in `ReadingSettings`.

What the tests prove, rather than what the code claims: a source resolving
to zero faces is refused at construction with a message naming the source;
a generic naming an absent family is reported and not fatal; script fallback
answers by ISO 15924 tag; family names intern rather than leaking per
session; and the whole reader suite — thirty-two tests now — runs on the
vendored Crimson Text faces with all three axes pinned and no platform
fallback underneath.

Still open, and both are one constructor argument away: `Faces::Host` is
what every desktop shell passes, so nothing yet *forces* a device build to
choose; and the iOS preset is not written, because whether a sandboxed app
can read `/System/Library/Fonts` is still the question only a device can
answer.

### Turn `fontconfig` off everywhere it is a lie

`fontconfig` is a cosmic-text default and compiles into every target. On iOS
that is `fontconfig-parser` and `roxmltree` linked in to parse
`/etc/fonts/fonts.conf`, a file that cannot exist; on Android the same two
crates are dropped only because Android takes a different branch by accident.
Declare it per target instead:

```toml
[target.'cfg(all(unix, not(any(target_os = "macos",
                               target_os = "ios",
                               target_os = "android"))))'.dependencies]
```

and off everywhere else. Linux keeps the alias rewriting, which under this
design is the one place fontdb's help is still wanted. iOS and Android drop
two crates and stop looking for a file that is not there. PLATFORM's feature
audit left this knob alone as "wanting a device to justify it"; the device
justified it.

## Building the C ABI

`chapbook-ffi` exists, at `crates/chapbook-ffi`, with `include/chapbook.h`
beside it. Forty-nine entry points over one opaque `cb_session*`: open
from a path, from bytes or from a file descriptor; metrics, navigation,
position; title and book kind; settings; `render_size` and `render_into`;
`suspend`, `release_caches` and the cache numbers; the font report; the
waker with `poll_loaded`; the log sink; and the input model — tap zones,
the default key table, reading direction and `cb_session_apply`. The six boundary questions
above were answered by the spike, so most of this was transcription. Eight
things were not.

**The last-error string cannot hang off the session, and the reason is the
interesting part.** The proposal above says "a per-session last error
message", which is the obvious shape and is wrong: the failures a host most
needs explained are a book that would not open, a font source that resolved
to nothing, and a config it assembled incorrectly — and in every one of
those there is no session to ask, because failing to make one is the whole
event. It is thread-local instead, and thread-local rather than global
because a `cb_session` is `Send`: two threads may each be driving one, and
neither should be able to overwrite the other's diagnosis.

**Every pointer-taking entry point is declared `unsafe fn`.** Not a
concession to a lint — though clippy's `not_unsafe_ptr_arg_deref` is what
raised it — but the truth: each one has preconditions C cannot check and
Rust cannot verify. The C declaration cbindgen emits is identical either
way, so this costs a host nothing and makes the obligation visible on the
Rust side, where the next person adding a function will see it. The
follow-on is that clippy then wants a `# Safety` section on all thirty-eight.
They get one section, at crate level, because cbindgen copies doc comments
into the header and a header repeating the same paragraph thirty-eight times
is a worse artifact rather than a safer one.

**The conventions had to become tests, and one of them was already broken.**
"Every entry point wraps its body in `catch_unwind`" is exactly the kind of
property that holds on the day it is written and decays silently afterwards:
the function that forgets is the one added six months later, and nothing
fails until a panic unwinds into a host and takes the process with it.
`tests/discipline.rs` reads the source back and asserts it — crudely, by
string search, because the shape being looked for is one line long and never
legitimately absent. It found two on its first run, `cb_abi_version` and
`cb_capabilities`, both of which had been left bare on the reasoning that
arithmetic cannot panic. That reasoning is how the rule erodes, so they are
wrapped and the rule has no exceptions. The same file asserts that every
`extern "C"` function is `#[no_mangle]` — the Rust-side half of the check
`android/build-jni.sh` performs against a built `.so` — and that no entry
point returns an owned pointer, which is what keeps `cb_free_string` absent.

**A generated header is not a compiling header.** cbindgen will happily emit
text that does not parse; a doc comment containing a comment terminator is
the classic, and this crate's comments are long. Nothing else in the
workspace compiles C, so nothing else would notice, and the host would find
out at their integration. So the header is compiled — twice-included, at
`-std=c99`, `c11` and `c17`, with `-Wall -Wextra -Werror` — and skipped
rather than failed where no compiler is on `PATH`, so the gate still runs on
a machine with only a Rust toolchain.

**`cb_capabilities()` is here because of the spike's silent missing
library.** A header cannot tell a host which `.so` it actually loaded, and
every profile this document argues for is a build that quietly cannot do
something a caller may assume. It returns a bitmask; the ABI's own test
suite uses it to skip the persistence test rather than `cfg!`, because
asking the ABI is what a host would do and the test should be shaped like a
host.

**Writing an actual C host is what found the last defect.** The Rust-side
tests all passed, the header compiled, and a short C program linked against
`libchapbook_ffi.so` then opened Moby-Dick, rendered it and walked all 675
pages — and reported a deliberately-wrong surface width as
`CB_ERR_UNAVAILABLE`, "the page is not available", when what had happened
was that the caller got its arithmetic wrong. `render_into` was discovering
the mismatch late, after the stride and length checks, and conflating "your
surface is the wrong shape" with "there is nothing to draw". It checks
against `render_size` first now, and the two are separate codes with a
message that names both sizes. Nothing was *broken* before; it just sent a
caller to look in the wrong place, which is the failure mode a C ABI can
least afford, because the caller cannot read the source to find out
otherwise. Worth the twenty minutes, and worth repeating from Swift when
iOS gets there.

**The log sink was promised above and was missing.** *What rung 3 found*
says in as many words that "the C ABI needs a log sink, and a callback is
the cheap version of it," and the first cut of the ABI did not have one —
which nothing noticed, because the only thing consuming it was a Rust test
that could reach `log` for itself. `cb_set_log_callback(fn, user,
max_level)` is that sink: records arrive tagged with the crate that emitted
them, so `chapbook_reader` and `chapbook_library` can be routed apart, and
**it fires on any thread**, the loader thread included, which the header
says because it is the mistake a host is otherwise going to make.
`cb_log` is beside it so a host's own messages land in the same stream in
the same order, which is the entire reason interleaved logs are worth
having. Verified the way the rest of it was: a C program with no Rust in it
watching `chapbook_reader: library unavailable: Not a directory` arrive from
inside the engine.

**`cb_rotation` shipped in the header before anyone had turned it.** Every
case in `tests/abi.rs` set `CB_ROTATION_NONE`, so a knob the header
advertises had never been exercised from C in any language — and
`render_into` takes a *different* path for a turned page, through a
temporary copied row by row. Rotation was covered below the boundary
(`chapbook-paint`'s unit tests, the session's own turned-panel test, and
the conformance harness's `RotationIsNotAReflow`, which ran on a device at
rung 4) and not across it. It is now: a quarter turn, `render_size`
swapping 600x800 to 800x600, the page count unmoved, the rotated copy path
writing real pixels, and the unrotated buffer refused rather than
half-filled.

The header also never said the thing a host most needs to know, which is
that `cb_metrics.width`/`height` are the page in **reading** orientation
rather than the panel. A 600x800 view wanting a turned page passes
800x600. Getting that backwards is not an error and is not reported as
one — the page just paginates to the wrong aspect, and since the host
allocates from `render_size` there is no mismatch left for anything to
catch. That is the same "sent to look in the wrong place" failure as the
`render_into` status above, and it is now a paragraph in the struct's
comment. Comment-only, so no ABI surface moved, but the header is a golden
and the diff is one every downstream host reads.

**What is deliberately not in it.** The display list, for the reasons
already given. Search, the table of contents, links and annotations, for a
different one: Contract tier means what ships holds still, so the first
header carries what five rungs on a real device actually demonstrated a
reader needs. Every one of those is additive later and none is blocked by
anything here.

**And the input model, which was excluded until a touchscreen had driven
it, and crossed once one had.** This paragraph used to record the
exclusion and the argument for it: the first cut carries what real rungs
demonstrated, and the input model had been demonstrated on zero of them.
The Android run closed that argument — taps resolved through the engine's
bands on a device, the volume keys turned pages, and `ActionOutcome`
earned its third value in front of a systemui window dump — so the model
crosses now, before iOS consumes the header, which is the consumer the
exclusion was deferring to. Six entry points:
`cb_session_reading_direction`, `cb_session_set_tap_zones`,
`cb_session_tap_action`, `cb_key_default_action`,
`cb_char_default_action`, and `cb_session_apply` returning a
`cb_action_outcome`.

Where the device evidence overruled this document's own sketch, the
evidence won. The sketch wanted a free `cb_tap_action_at` over a
`cb_tap_zones` struct; the zones live on the session instead, because the
struct's direction field is the one thing a host must *not* fill in, and
JNI landing the same call found the shape — configure bands, and the
direction is re-read from the book so a tap cannot be resolved against
the wrong edge. Actions cross as `cb_action` enum values rather than the
names JNI uses, and both are right for their boundary: Kotlin mirrors a
Rust enum whose ordinals can move in any commit, while this header is a
frozen artifact whose values are permanent — and on iOS it ships inside
the same XCFramework as the library it describes, so the two cannot skew.
`CB_ACTION_NONE` is the value zero so "this tap meant nothing" is
falsy in C, and keys that are characters take their own entry point,
because a Unicode scalar is not a member of a closed set.
`cb_key_default_action` is free of any session, as sketched: the value is
the default table — it already knows a Kobo's bezel buttons — not the
mutability, and a host that rebinds keeps its overrides on its own side.
`cb_session_back`/`can_go_back` were sketched and are *not* there:
`CB_ACTION_BACK` through `cb_session_apply` already answers both, and
`CB_OUTCOME_UNHANDLED` at the bottom of the trail is precisely the
answer `can_go_back` would have existed to precompute.

The file-descriptor door is `#[cfg(unix)]` — a descriptor is
what Android and iOS hand out, and Windows has no analogue worth guessing at
from this side of the boundary.

## Sequencing

PLATFORM's priority list puts the FFI first because it forces the §2
cleanup. That is right about the dependency and wrong about the order:

1. **Spike (throwaway).** Rungs 1–5 above. Output is a list of API defects,
   not code worth keeping. Done, twice over: the defects were found, fixed
   in step 2, and then the spike was rewritten against the result — which
   is how the last two findings below turned up, since a binding with no
   workarounds left has nothing to hide a regression behind.
2. **Fix the shape, in safe Rust.** Typed sources and a builder, a font
   source (done), a credential store (done), an HTTP transport (done — and
   `chapbook-reader`'s new `ureq` feature is what makes dropping rustls and
   ring reachable from the session rather than only from `opds-client`,
   which is this document's second NDK prerequisite), typed sources (done),
   the library directory (done — `SessionConfig::with_library_dir`),
   `render_into` (done), a cache budget and `release_caches` (done),
   `suspend()` (done), the log seam (done — the `log` crate, plus
   `chapbook_core::log_to_stderr` for shells that want a terminal), and the
   optional `library` feature (done). Step 2 is complete: every capability
   arrives through `SessionConfig` or `Source`, and every profile the
   device targets need is a feature away. All of it tested in the workspace,
   none of it FFI. This is PLATFORM §2's session-lifecycle item, arrived at
   by evidence instead of by guessing. The wasm32 CI check lands here, once
   there is something for it to prove.
3. **`chapbook-ffi` for real.** cbindgen header, stable error codes, Contract
   tier, and an FFI conformance test that drives the C ABI from a Rust test
   so CI covers the boundary without an emulator. **Done** — see *Building
   the C ABI* below. All four parts landed as described; the header is
   checked in and golden-tested rather than generated at build time, which
   the sketch did not specify and which is what lets a host consume the ABI
   with no Rust toolchain at all.
4. **`chapbook-android`, and the input model.** The AAR, the demo app, and
   `chapbook_core::input` — which lands here because a touchscreen is where
   tap zones can actually be judged. *Started. `chapbook-jni` and the two
   Gradle modules already exist and stay as they are — see* What became of
   `chapbook-jni` *— and `chapbook_core::input` now exists: `Action`,
   `ReadingDirection`, `TapZones::action_at`, and a `KeyMap` that already
   knows about Kobo and PocketBook page-turn buttons — with
   `Session::apply(Action)` under it and `chapbook-viewer-gtk` converted to
   both. Android drives all of it now: `ReaderView`'s hardcoded thirds are
   gone, the volume keys turn pages, and it ran on an emulator — see* What
   the touchscreen settled *below. What is left is the winit viewer's
   hand-translation and the AAR as a published artifact.*
5. **The browser demo.** Last, because Android has a user and a demo has an
   audience, and because by this point step 2 has already done all of its
   work for it.

## Still to decide

- ~~**Whether `frame()` crosses the boundary at all**~~ — **decided: it does
  not.** Pixels only. This was held open because accessibility was thought
  to be built from the display list, which would have made leaving it out a
  real cost. It is not: the list carries glyph indices and no text (PLATFORM
  §7). A screen-reader path wants a small text-runs-and-rects accessor
  instead, which is additive and can arrive whenever accessibility does.
  Shells that rasterize for themselves still take `frame()` in Rust; that
  path is unchanged and simply does not cross the C ABI.
- ~~**Whether a built artifact can report its own feature set**~~ —
  **decided: a bitmask.** `cb_capabilities()` returns one, and
  `cb_capability` names the bits. A struct would have had to stay
  ABI-stable for a list that grows every time a feature is added, and a
  string would have made every host parse. Raised by the spike building
  without a library and saying nothing; the ABI's own tests now use it to
  skip rather than `cfg!`, because asking is what a host does.
- **Whether an embedded font source ships in every shell binary** or only in
  tests and conformance. Four Crimson Text faces is not nothing. The
  recommendation is that it is always available to tests, and that shells are
  required to name a source — so nobody ships a reader that silently falls
  back to one serif.
- **Who owns the per-target generic tables.** The recommendation is
  `chapbook-layout`, shipping known-good mappings for Android, iOS, macOS and
  Windows with a shell override, rather than each shell rediscovering that
  `sans-serif` means `Roboto` on Android.
- **Whether to upstream the fontdb iOS branch.** Six lines, and it makes
  `Faces::Host` correct on iOS instead of empty. Worth sending either way;
  the question is only whether we wait on it.

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

**Done, and measured.** `--no-default-features` is the EPUB-only profile:
250 dependency edges against 259 with the library and 313 for the default
build, with `rusqlite` and `libsqlite3-sys` — a C build, not just a crate —
gone entirely. Without it a session reads but remembers nothing: every book
opens at the beginning, marks cannot be stored, and settings last as long as
the session does. Those are the honest consequences of having nowhere to
write, not degradations to apologise for.

`opds` requires `library`, which is worth stating because it is not obvious:
a page stream caches to disk, and the directory it caches into is the
library's.

The argument against was never the port, it was that a browser build becomes a
fourth target to keep green forever. The answer to that is proportion: **check
it in CI, do not ship it from CI.** A `cargo check --target
wasm32-unknown-unknown` over the EPUB-only profile costs no linker, no
wasm-bindgen and no Node, and it catches exactly the regressions that would
quietly close the door — a new ambient `std::fs` call, an `Instant::now`, a
thread. Building and deploying the page itself stays manual and occasional.

That job landed with the optional-library work, and its comment says what it
does and does not prove — because the concern below was right and does not go
away just because the feature exists. It would pass while proving nothing about
SQLite, because `chapbook-library` compiles for
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
at all. It does not. chapbook's cascade driver has always passed `None` for
`traverse_dom`'s rayon pool, and `STYLE_THREAD_POOL` is a `LazyLock` that
nothing on our path dereferences — the only two readers in stylo are its own
shutdown path and a Gecko-facing `get_thread_handles`. No pool is ever built
and no thread is ever spawned.

That is not a portability convenience, it is load-bearing: the arena DOM's
`unsafe impl Send`/`Sync for Node` justifies the
`Cell`/`UnsafeCell` state a traversal mutates by there being exactly one
traversal writing it. A pool passed to `traverse_dom` would race that state
in a build that still compiles and mostly still works. Until now the
invariant was held up by a code comment; it is now
`chapbook-layout/tests/sequential.rs` and an entry in CONTRIBUTING's list.

What remains is dead weight rather than live threads: `rayon-core` is an
unconditional dependency of stylo, so it links, and its panic strings are
visible in the 6 MB module above. Deleting that is a size question, not a
correctness one.
