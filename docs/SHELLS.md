# Writing a shell

Chapbook is an engine, not an app. A **shell** is the part you write: it
owns a window or a panel, it owns input, and it owns the process. Between
those, it drives one object — `chapbook_reader::Session` — which owns
everything else: the open publication, fonts, layout, the reading
position, settings, selection, annotations, and the library.

This document is the contract from the shell's side. `ARCHITECTURE.md`
explains how the engine computes a page; nothing here is about that.
`PLATFORM.md` explains which seams are substitutable; this is how to sit
on top of them.

There are four shells in the workspace to read alongside it:

| | crate | what it shows |
|---|---|---|
| smallest | `chapbook-viewer/examples/minimal.rs` | the whole contract, nothing else |
| desktop | `chapbook-viewer` | selection, links, clipboard, touch, GPU |
| toolkit | `chapbook-viewer-gtk` (Linux only) | the same session under someone else's main loop |
| device | `chapbook-panel-fbdev/examples/show.rs` | rasterizing yourself, panel policy, damage |

Start from `minimal.rs`. It exists to be copied.

## The shape of a shell

```rust
let mut session = Session::open(&source)?;         // 1. open
session.set_metrics(metrics);                      // 2. say how big a page is
loop {
    // 3. turn input into Session calls
    match input() {
        Key::Right => { session.next_page(); }
        Key::Left  => { session.prev_page(); }
        _ => {}
    }
    // 4. ask what to draw, and draw it
    if let Some(pixmap) = session.render() {
        blit(&pixmap);
    }
}
session.save_position();                           // 5. leave a bookmark
```

Everything below is a detail of one of those five steps. The order
matters: `Session` produces nothing at all until it has metrics, because
it does not know what a page is until you tell it.

## 1. Opening

`Session::open` takes a string and dispatches on it: a path to an `.epub`,
a `.cbz`, or a `.pdf`, or an OPDS URL for a page-streamed comic. Which of
those compile in is a feature choice — `cbz`, `pdf` and `opds` are all on
by default and a device build turns off what its hardware will never open.
A format that was compiled out is refused at open time, with
`ChapbookError::FormatNotBuilt`, rather than at compile time — so the same
shell source builds against every configuration.

Opening also imports or matches the book in the library, which is how
step 5 has somewhere to put a position. The library lives under
`CHAPBOOK_LIBRARY_DIR` when that is set and in the XDG data directory
otherwise; if it cannot be opened at all, the session says so on stderr
and reads on without one. A shell that wants no library — a preview pane,
a test — should point that variable at a scratch directory rather than
look for a switch, because there isn't one.

## 2. Metrics

```rust
session.set_metrics(PageMetrics {
    size: Size::new(600.0, 800.0),   // the whole page, margins included
    margins: EdgeSizes::uniform(32.0),
    dpi_scale: 1.0,
    rotation: Rotation::None,
});
```

`size` is in CSS pixels and in **reading orientation** — the orientation
text flows in, not the orientation your panel is bolted to the case in.
`dpi_scale` is passed to rasterizers and does not affect layout: a hidpi
window sets `size` to the logical size and `dpi_scale` to the ratio, so
the same book paginates identically at every scale factor.

`rotation` is likewise not a layout input. It is applied on the way to the
panel, which is why turning the panel does not repaginate the book — see
§7.

Call `set_metrics` again on every resize and scale-factor change. It is
cheap when nothing changed (it compares first), it relayouts and keeps the
reader's place when the page box changed, and it skips the relayout
entirely when only the rotation did.

## 3. Input

Navigation, in rough order of how often a shell wires it up:

- `next_page` / `prev_page` — the ordinary turn. Crosses into the
  neighbouring spine unit at the ends.
- `next_unit` / `prev_unit` — chapter skip.
- `goto(locator)`, `goto_anchor(spine, fragment)`, `goto_toc(entry)` —
  jumps. `toc()` gives you the table of contents to build a menu from.
- `link_at(x, y)` then `follow_link(href)` — a tap on a link. `link_at`
  returns the href under a point, `follow_link` navigates it and returns
  false for anything that is not a reading position (an external URL is
  the shell's problem, and the shell's opportunity: open a browser).
- `back()` / `can_go_back()` — a 64-deep stack. Jumps push onto it; page
  turns deliberately do not, or "back" would just be "previous page".

**All four navigation calls return `bool`: whether the position moved.**
Use it. Do not derive it, and in particular do not compare `page()`
across a turn — a turn off the end of a unit crosses into the next one by
resetting the page to 0, so a successful move looks like a failed one.
Most books open on a single-page cover, so a loop built on that
comparison stops on the very first turn. That is not a hypothetical: it is
what the fbdev shell did on its first run against real hardware, and it is
why `Session::position()` returns the `(spine, page)` pair as one value.

Selection, links and text:

- `selection_begin(x, y)` on press (returns false if there is no text
  there — that is your cue to treat it as something else), `selection_drag`
  on move, `selection_clear` to drop it.
- `select_range(start, end)` places a selection directly, which is how you
  show a search hit or an annotation.
- `selected_text()` for the clipboard, `selected_range()` to know whether
  there is one.
- `search_unit(spine, query)` for one unit — the piece you can drive from
  a worker — and `search(query, limit)` for the whole spine, which blocks.

Settings and annotations round it out: `set_settings` with a
`SettingsScope` of `Global` or `ThisBook`, the `adjust_font` / `cycle_theme`
conveniences over it, and `add_highlight` / `add_note` / `add_bookmark` /
`annotations` / `goto_annotation` / `remove_annotation`.

Hit-testing takes page coordinates. If your panel is rotated, put the
event through `PageMetrics::panel_to_page` first — the engine does not see
your rotation on the way in, only on the way out.

## 4. Drawing

Two ways, and the difference is who owns the rasterizer.

**`session.render() -> Option<Pixmap>`** is the whole pipeline plus the
bundled CPU backend. A shell that just wants pixels calls this and blits.
It also applies the panel policy on the way out — the pixel format set by
`set_pixel_format`, then the rotation from the metrics — so the pixmap is
already in panel orientation and the panel's colour depth.

**`session.frame() -> Option<Frame>`** is the seam. `Frame` carries:

- `list` — a `DisplayList` of paint-neutral ops. There are exactly three:
  `FillRect`, `GlyphRun`, `Image`. Everything else lowers to those.
- `intent` — a `FrameIntent` saying what changed.
- `damage` — `Option<Rect>`, the region the change disturbs, or `None`
  meaning "assume the whole page", which is always correct and sometimes
  wasteful. Selections, highlights and a landed page image state a region;
  a turn, a unit change and a reflow replace the page and say so.

Glyph runs name faces in the session's font database and `Image` ops carry
keys, not pixels, so a shell rasterizing for itself also needs
`session.paint_resources()`, which hands back the `FontSystem` and the
`ImageStore` as disjoint borrows.

Both return `None` before metrics are set, and while the current unit has
no page — an image book still decoding, or a unit that failed to load.
`None` means "nothing to draw yet", not "error".

**Taking a frame consumes the change record.** The next `frame()` reports
`Repaint` until something else moves. So take one frame per paint, and do
not call `frame()` to poke the engine into doing something — if you want
the current unit laid out without consuming the record, ask for
`page_count()`.

`render()` takes a frame internally, so it consumes the record as well.
The two are alternatives, not layers: a shell that wants both the pixels
and the intent should call `frame()` and rasterize the list itself.

`FrameIntent` is ordered by how much of the page a change disturbs:

```
Repaint < Selection < Annotation < ContentArrived < PageTurn < UnitChange < Relayout
```

Several changes before a paint collapse to the strongest. A windowed shell
can ignore intent and repaint. A device shell cannot: `intent.update_class()`
maps it to the least disruptive panel update that still renders the change
faithfully, which is the difference between a page turn that feels instant
and one that flashes the whole screen black.

## 5. Position

`save_position()` captures the place as a layered locator and persists it.
Reopening the same book restores it. Call it when you exit — including the
paths that are easy to forget, like the window's close button — and after
any jump you would be sorry to lose.

`current_offset()` is the raw offset of the current page in the unit's
locator space, if you want to show or sync a position rather than store
one. `locator()` gives the full locator. `LOCATORS.md` explains what
survives a relayout and what does not, which matters the moment you sync
positions between two devices with different screens.

## 6. Background loads, and the one rule that is not negotiable

Comic pages and PDF rasterizations decode on the session's loader thread.
Text chapters do not — an EPUB chapter is a local zip read plus a cascade,
and it happens inline.

> **Never call `unit_bytes` or `resource` from your event loop.** They are
> "blocking fetch plus cache": seconds for a cold page over the network,
> and a failure mode of `ChapbookError::Network`. That is the loader
> thread's job, and the session already has one.

What a shell does instead:

```rust
// Once, at startup: let the loader wake you.
let proxy = event_loop.create_proxy();
session.set_waker(move || { let _ = proxy.send_event(()); });

// When woken:
if session.poll_loaded() {
    request_redraw();
}
```

`poll_loaded` drains finished loads into the caches and returns whether
**the page on screen changed**. A prefetched unit landing does not count:
nothing the reader can see moved, and treating it as a change costs a
full-page panel update. `has_pending_loads()` says whether any loads are
still in flight, which is what a placeholder page is telling the reader
about. A
shell with no thread-safe wakeup can poll `has_pending_loads` instead of
installing a waker; a shell that does neither shows placeholders forever.

The waker is called from the loader thread, so it must be `Send + Sync`
and must not touch the session. Post an event; do the work on your own
thread.

## 7. Panels

A framebuffer or e-ink panel needs three things a window does not.

**Pixel format.** `session.set_pixel_format(PixelFormat::Grey { levels, dither })`
makes `render()` quantize for a panel that cannot show full colour. Panel
policy belongs to the target, not to whichever rasterizer produced the
pixels — which is also why `chapbook_paint::rotate` lives there rather
than in a backend.

A shell rasterizing its own frames does the same reduction itself, and
should reach for `chapbook_paint::quantize_regions` rather than
`quantize`:

```rust
let dithered = frame.list.dither_regions(scale);
chapbook_paint::quantize_regions(&mut pixels, w, h, format, &dithered);
```

`dither` is one flag for a whole page, and a page is not one kind of
thing. Diffusing error through body text stipples the antialiased edge of
every glyph; *not* diffusing it through a photograph turns the photograph
into a silhouette. `dither_regions` asks the display list which pixels
came from images — the last point at which anything knows — so the
diffusion happens over those and nowhere else. At sixteen levels this is
a refinement; at two, which is what a 1bpp panel has, it is the
difference between a readable page and an unreadable one.

`quantize` is still there for a caller with no display list to hand, and
still dithers the whole page when asked.

**Rotation.** Set it in `PageMetrics` and the page is laid out unturned and
turned on the way out. `panel_size()` gives you the buffer size (axes
swapped on a quarter turn) and `panel_to_page()` untwists input
coordinates. Changing only the rotation does not rebuild the layout —
`same_layout()` is what decides that — so a shell may turn the panel as
often as it likes.

**Update classes.** `PanelDriver` wraps a `Panel` implementation and takes
`present(&rgba, damage, intent.update_class())`. `damage` is `None` for
"the whole panel", the same convention `Frame::damage` uses, so one passes
straight into the other — via `PanelRect::from_page(rect, page, scale,
rotation)`, which converts page coordinates to panel ones with the
rotation folded in. `RefreshPolicy` decides separately when to spend a
full flash the content did not ask for, to pay off ghosting.

Two calls a shell owes the panel and tends to forget:
`PanelDriver::settle(&rgba)` repaints whatever a fast update left
degraded — cheap when nothing is owed, so an idle tick is the right place
for it — and `flush()` before tearing the panel down.

## 8. Proving it

The failure this document keeps returning to — a shell that drove the
session wrong and stopped turning pages — was invisible to every test in
the workspace, because none of them drove a shell. So there is now a
harness for exactly that:

```rust
use chapbook_reader::conformance::Harness;

#[test]
fn my_shell_drives_the_session_correctly() {
    Harness::new(|| Session::open("fixture.epub").unwrap())
        .run()
        .assert_ok();
}
```

It opens a fresh session per check and asserts the rules this document
states: that a crossed unit still counts as a move, that turning forward
reaches the end of the book and stops, that turns are reversible, that a
resize keeps the reader's place and reports `Relayout`, that a rotation is
not a reflow, that taking a frame consumes the change record, that damage
stays inside the page, that background loads converge, that a selection
dies with its page, and that a position survives a restart.

Checks that cannot apply report `Skipped` rather than passing quietly — a
comic has no text to select, a one-chapter book has no unit to cross — so
read the report, not just the boolean.

From a terminal, against any book:

```sh
cargo run -p chapbook-reader --example conform -- mybook.epub
```

It writes to the library, because checking that a position survives a
restart means saving one. Point `CHAPBOOK_LIBRARY_DIR` somewhere scratch
if that matters.

## 9. What to depend on

Depend on `chapbook-reader` and nothing else. It re-exports everything a
shell consumes — `chapbook_core`, `chapbook_paint`, `chapbook_library`,
`chapbook_render_tinyskia`, `tiny_skia`, `cosmic_text` — specifically so a
shell cannot skew versions with the engine it is driving.

Model types come from `chapbook_core`. `chapbook_layout` — including its
`dom` and `cascade` modules — is internal; a shell that reaches into it has
found a gap in this document, and the gap is the bug.
