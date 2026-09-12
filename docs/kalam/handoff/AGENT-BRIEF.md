# Engine handoff — read this first

You are an AI agent working in **calibre-alt** ("Kalam"), a GTK4/relm4
desktop ebook reader. This directory was generated from a second
repository, **kalam-engine**, by a second agent that cannot see this
repo. The owner relays messages between the two of you by copy-paste.
The job: replace Kalam's WebKitGTK reader with the `kalam-reader`
widget from kalam-engine.

## Files here

| File | What |
|---|---|
| `INTEGRATION.md` | **The recipe.** Numbered steps, each a compile point. Follow it in order. |
| `patch/engine.rs` | Complete new file → copy to `src/pages/reader/engine.rs`. |
| `kalam-reader-API.md` | Every public item of the widget crate with its doc comments, generated from source. Authoritative for signatures. |
| `kalam-reader-README.md` | The widget crate's README: what it does, what it needs, the numbers behind its defaults. |
| `ENGINE-VERSION.txt` | The exact engine commit this bundle describes. Quote it in every report. |

You do **not** have the engine's source. Do not guess at APIs beyond
`kalam-reader-API.md`; if you need something the widget does not
expose, say so in a report (format below) instead of working around it.

## Ground rules (from the owner, via the engine side)

1. **Do the steps in INTEGRATION.md's order, one commit per step.**
   Step 0 (the gtk4 0.9 → 0.11 / relm4 0.11 / libadwaita 0.9 bump) is
   its own commit and must build before any reader code is touched.
2. **The recipe was written from a snapshot that lacked four files**
   (`src/pages/reader/{session,lists,panels}.rs`, `src/db/annotations.rs`).
   Where it says "described, not written" for those, you write it — you
   can see them; the engine side cannot.
3. **The tree wins over the recipe.** If a line number, field name or
   signature in INTEGRATION.md does not match this repo, follow the
   repo and note the difference in your report.
4. **Do not modify the engine's contract to fit Kalam.** If a widget
   method is missing or wrong, report it; the engine side changes the
   widget and regenerates this bundle. Never vendor or fork
   `kalam-reader` into this repo.
5. **Keep every WebKit-free feature working:** sidebars, TOC, bookmarks,
   highlights list, saved words, dictionary, reading sessions, UI prefs.
   The recipe touches only the reading surface.
6. **Build with `cargo build --release -j 2`** (4 GB machine). Never
   `cargo clean`.
7. The owner is not a software engineer. Reports go through them
   verbatim; keep the *report block* machine-precise and put any
   plain-language summary outside it.

## Report format — the only thing that crosses to the other side

The owner copies your report to the engine agent and pastes its reply
back. Neither agent sees anything else. So every report must be
**self-contained**: the step, the file, the exact compiler text, and
what you already tried. Use this block exactly, so it can be parsed
by eye in one pass:

```
=== KALAM REPORT ===
engine: <commit from ENGINE-VERSION.txt>
step: <INTEGRATION.md step, e.g. 3d>
status: OK | BLOCKED | QUESTION | DONE
files: <files touched in this step>

<For BLOCKED: the first compiler error, verbatim, including the
 `-->` file:line:col line and the code excerpt cargo printed. One
 error per report — the first one; the rest usually follow from it.>

<For QUESTION: the exact API/behaviour you need and where in
 INTEGRATION.md it is unclear.>

<For OK/DONE: one line per checklist item passed/failed (step 4),
 with numbers where the checklist asks for them.>

tried: <what you already attempted, one line each>
=== END ===
```

Reports the engine side will send back look like:

```
=== ENGINE REPLY ===
re: <step>
action: <what to change, as a unified diff or an exact edit
         instruction with the file and the old/new text>
or
action: engine updated to <commit>; regenerate bundle
        (owner runs make-bundle.sh and re-copies this directory)
=== END ===
```

Apply what the reply says, rebuild, send the next report. If a reply
does not fix the error, say so with the new error — do not silently
try something else for more than one attempt.

## What "done" means

INTEGRATION.md step 4's checklist passes on the owner's machine:
first page well under a second, instant page turns, position/highlights
survive font change and reopen, theme switch instant, continuous-scroll
switch works, dictionary popover works, external links open the
browser, and memory while reading is under ~100 MB above the library
page. Report those as the final `status: DONE` block, with the numbers.
