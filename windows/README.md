# Windows

Two different things live here, and it is worth being clear which is
which before reading further.

| | |
|---|---|
| `Chapbook/` | A .NET binding over the C ABI — the analogue of `ios/Chapbook`, for a host that is not Rust |
| `Chapbook.Tests/` | The gate: the binding driven against the real engine, headless |
| `build-native.ps1` | `chapbook-ffi` → `chapbook_ffi.dll` |

It covers the session, the shelf, the text surface, session events, the
host transport and sync — everything the header carries except the
`download` callback, which .NET has no background-transfer facility worth
the engine deferring to.

The *other* Windows integration is `crates/chapbook-viewer-win32`, which
is a Rust shell over `chapbook-reader` directly and shares nothing with
this directory. A desktop Rust binary has no boundary to cross, so it does
not go through the header. This directory is for everything that is not
Rust: WinUI, WPF, WinForms, MAUI, a console tool.

## Building

```powershell
./windows/build-native.ps1                       # cargo → target/release/chapbook_ffi.dll
dotnet test windows/Chapbook.Tests/Chapbook.Tests.csproj
```

While iterating on the Rust, `-Profile debug` and
`-p:ChapbookProfile=debug` are minutes faster and the tests say the same
thing.

The DLL is a build product and is not checked in. `Chapbook.csproj` copies
it beside the assembly from `target/<profile>/` and *warns* rather than
fails when it is absent, because the managed half compiles perfectly well
without it and the honest failure is the `DllNotFoundException` on the
first call.

## The binding

`Chapbook` is P/Invoke over `crates/chapbook-ffi/include/chapbook.h` and
nothing else. Two decisions shape it.

**It does not depend on WinUI, and its target framework is plain
`net9.0`.** Nothing in it calls a Windows API — it is the ABI's calls and
the types they name — so a WPF, WinForms, MAUI or console host can consume
it without pulling in a UI framework it is not using. Bundling one into
the thing a host embeds is exactly the trade the engine refuses one layer
down, where fonts, HTTP, credentials and storage all arrive by injection.

**Runtime marshalling is disabled for the assembly.** With it on, the CLR
silently converts at the boundary: the ABI's one-byte `bool` becomes a
four-byte Win32 `BOOL`, and a struct that has drifted from the header
still compiles while misreading every field after the first mismatch.
Turning it off makes that a build error in `Native.cs` instead, which is
why the native structs there use `byte` where the header says `bool` and
convert once, in the public types. Strings are UTF-8 explicitly for the
same class of reason: the default is ANSI, which mangles any path holding
a character outside the active code page and reports nothing.

## The transport, and why sync needs the write half

`HttpTransport` is two blocking methods — `Get`, and `Send` for POST, PUT
and DELETE — and a host implements them over whatever it already has.
`HttpClientTransport` is that over `HttpClient`, which is the reason to
bother: a client the app configured carries its proxy, its trust
decisions, its authentication handler and its timeouts, and a stack inside
the engine would carry none of them. No credential crosses this boundary
by design; a service behind auth wants a client whose handler attaches
its own.

Three parts of that contract are unforgiving, and all three are the kind
that fail quietly. A callback **may fire on any thread** and must *block*
until the transfer settles — it is not a place to hand back a `Task`. It
must not call back into this library except through the
`HttpResponseBuilder` it was given. And it must not **retry**:
authentication retry is the engine's own flow one level up, so a
transport that retries turns one 401 into several.

The write half has a duty of its own: **report the response headers**.
A Web Annotation container carries its whole concurrency story in `ETag`
and says where it put a new mark in `Location`, so a transport that
discards them makes safe concurrent editing impossible — and the failure
looks like sync quietly forgetting marks rather than like a missing
header.

`HttpTransport.OnReleased` is the other end of the ABI's finalizer,
called exactly once when the engine lets go — the last session closing, a
worker closing, or a configuration disposed without ever being opened.
A host that built something for the engine's sake releases it there,
which is the only moment nothing is still inside a callback.

## Testing sync without a server

`Chapbook.Tests` fakes the transport rather than opening a socket, and
that is a design point rather than a shortcut: the transport *is* the
seam, so a fake one exercises the whole path — worker thread, callbacks,
response builders, the report queue — with nothing to flake. What the
tests then assert is the behaviour that matters to a host: a shelf where
nothing syncs finishes with zero books rather than spinning, an
unreachable service is one book's problem and not the batch's, a book
that has left the shelf is *reported* rather than silently skipped, and
the waker fires on the worker's thread and not the caller's.

## Five things the ABI does not say out loud

Each of these was found by a test in `Chapbook.Tests` failing, and each is
a bug a host would otherwise ship.

- **`SetTapZones` takes two band *widths*, not two boundaries.** Passing
  0.3 and 0.7 does not leave a middle band: it makes the next-page band
  seven tenths of the page, overlapping the other, and overlaps resolve to
  the previous-page side. The binding names them `previousWidth` and
  `nextWidth` so the reading cannot go wrong; the ABI's own parameters are
  `prev_fraction` and `next_fraction`, and only the *middle* band's action
  is configurable.
- **Setting the metrics is not laying out.** Layout is lazy, so
  `PageTextRuns` and `SpeakablePage` answer `null` on a session that has
  only been told how big a page is. Rendering lays it out; so does asking
  for `PageCount`, which is the cheap way when a host wants the text
  before the pixels.
- **A restored position lands on the first *render*.** Reopening restores
  the unit as soon as the loads settle, but the page inside it stays at
  zero until a frame is taken — laying the unit out is not enough. A host
  that reads `Position` before it draws sees the top of the chapter, which
  is indistinguishable from a bookmark that was never saved. Draw, then
  ask.
- **There is no `save_position` across this ABI.** The Rust API has one
  that persists and does nothing else; the header does not carry it, so
  `Suspend()` — which also closes the database and drops the caches — is
  the only way to leave a bookmark. Disposing a session does not save.
- **Pixels are premultiplied RGBA, not BGRA.** Windows imaging is mostly
  BGRA, including a `WriteableBitmap`'s back buffer, so a host either
  swizzles or asks its surface for RGBA. Getting it wrong renders a page
  that reads cold blue where the sepia theme should be warm paper — a
  colour bug people see and nothing reports, which is why
  `TheSepiaThemeProvesTheChannelOrderIsNotSwapped` exists.

## Packaging

In this repository the consumer is a project reference and the DLL is
copied to the output directory. A NuGet package puts the same file at
`runtimes/win-x64/native/chapbook_ffi.dll`, which `DllImport` resolves
with no help from the host; an Arm64 build goes beside it under
`runtimes/win-arm64/native/`. `Engine.AbiVersion` is what a host checks
before anything else when it ships its own copy, and
`Engine.Capabilities` is what it asks to find out which formats that copy
was actually built with — a header cannot answer either.

## What is not here yet

No UI. `Chapbook.WinUI` — a `SessionView` control and the automation peer
that puts its page in front of Narrator — and a demo app over it are the
next thing, and the reason the binding was built first: everything above
it is now a matter of drawing, not of boundary-crossing.

The `download` callback is deliberately unbound. It exists for a host with
a background transfer facility worth deferring to — an iOS background
`URLSession`, Android's `WorkManager` — and .NET has none of that shape,
so the engine streams through `Get` and writes the file itself.

`crates/chapbook-app` is the thing to read before building that UI. It is
the shared application model the GTK app sits on, and a Windows
application belongs over an equivalent rather than over a second viewer —
though a .NET front end reaches the engine through *this* binding, not
through that crate, and the two are different answers to different
questions.
