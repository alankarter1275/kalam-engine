# Windows

Two different things live here, and it is worth being clear which is
which before reading further.

| | |
|---|---|
| `Chapbook/` | A .NET binding over the C ABI — the analogue of `ios/Chapbook`, for a host that is not Rust |
| `Chapbook.Tests/` | The gate: the binding driven against the real engine, headless |
| `build-native.ps1` | `chapbook-ffi` → `chapbook_ffi.dll` |

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

The binding covers the **session**, completely — including the events the
ABI grew alongside the sync work, which are the other half of a wake:
`PollLoaded` answers "should I repaint", `DrainEvents` answers "is there
anything to tell the reader", and a host needs both.

What it does not cover, in the order the ABI added it:

- **The shelf.** `cb_library_*` — query, collections, mark-as-read — which
  the Swift package already has and which a Windows app opening onto
  something other than a book will need first.
- **Sync.** `cb_sync_*`, plus the `cb_library_*_sync_*` calls that record
  which services a book answers to. It is the larger piece, because the
  sync client is opened over a host transport rather than a bundled one:
  binding it means binding `cb_http_get_fn`, the new `cb_http_send_fn`,
  and `finalize`, and getting the ownership and threading of those
  callbacks right. `HttpClient` is what a .NET host would put behind it,
  which is exactly the substitution the seam exists for.

And no CI job compiles this directory. That is the state `ios/` was in
before the `apple` job existed, when the Swift package landed broken and a
Mac was the only thing that could have said so; the same is true here of a
Windows runner with the .NET SDK on it, and it is worth fixing before this
grows further.

`crates/chapbook-app` is the other thing to read before building on this.
It is the shared application model the GTK app sits on, and a Windows
application belongs over that rather than over a second viewer — the
binding here is what a non-Rust front end would use, and the two are
different answers to different questions.
