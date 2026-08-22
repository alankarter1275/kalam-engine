# Locator stability

Positions and annotations are keyed on `Locator { spine_index, char_offset }`.
The char_map makes offsets survive **relayout** — but relayout is not the only
thing they must survive. Two more axes, and both are cheap to handle now and
miserable to retrofit:

1. **chapbook releases.** `char_offset` indexes into text produced by our own
   extraction: which nodes contribute, whitespace policy, entity handling,
   counting unit. Any change to any of those silently shifts every stored
   position. History's warning: readers that stored engine-internal locators
   (KOReader's crengine XPaths, Kobo's injected span ids) made positions
   unreadable outside one engine version. An unversioned char_offset is the
   same bug, one layer up.
2. **Replaced files.** Re-download a book as a different edition and exact
   offsets die. Systems that keyed state to file bytes (kosync's MD5-of-file
   identity) orphan everything on re-download.

No single locator format survives both. Every design that held up in the wild
(W3C Web Annotation selector stacks, Readium locators, URL text fragments)
layers several locators and degrades gracefully. Do the same.

## Requirements

### 1. Specify and version the extraction function

A short normative section in chapbook-core (plus a golden test: fixture EPUB →
expected offset map, so changing the definition is a conscious act):

- **Unit:** Unicode scalar values (`char` count of the text), not bytes, not
  UTF-16 units.
- **What counts:** text nodes of the post-parse tree, document order. Excluded:
  generated `::before`/`::after` content, `alt` text, `display:none` subtrees.
  The offset anchors to *source* text; a page may begin mid-visible-text where
  generated content renders. That's accepted.
- **Raw, not collapsed:** offsets index un-collapsed text-node content.
  Whitespace collapsing is layout's concern; keeping offsets pre-collapse makes
  them independent of `white-space` and of collapsing-rule changes.
- Store a **`locator_version`** integer with every position/annotation; bump it
  on any change to the above.

### 2. Store layered locators

Per position — and per annotation *endpoint* (annotations are a pair of these,
and a drifted highlight silently marks the wrong text, which is worse than a
lost bookmark, so the quote layer is mandatory there):

```
spine_href, spine_index        identity within the book
char_offset, locator_version   exact primary
quote { prefix, exact, suffix }  ~32 chars context each side
spine_fraction                 within-spine-item proportion
book_progression               whole-book proportion
```

All of it falls out of the char_map and the text around the offset at write
time.

### 3. Resolve via fallback chain

1. Same file + same `locator_version` → `char_offset` directly.
2. Same file, older version → re-find `quote` nearest `spine_fraction`; on
   success rewrite the offset at the current version (self-healing migration).
3. Different file/edition → quote search across the spine biased by
   `book_progression`; else `spine_href` + `spine_fraction`.
4. Last resort → `book_progression`. Degrade to "right page-ish", never "gone".

### 4. Define the whole-book denominator

`book_progression = (extracted chars in prior spine items + char_offset) /
total extracted chars`. Character-count weighting is stable across font size,
margins, and relayout; synthetic page counts and file sizes are not. Write it
down in chapbook-core so every future producer computes the same float.

The denominator counts the format's *progression unit*: extracted chars for
reflowable text; pages for image-per-page formats (each spine item is one
page, `char_offset` always 0, quote layer empty — position identity lives in
`spine_index`/`book_progression` alone). Under the page-start convention the
last page of an n-page book reads (n−1)/n; 1.0 needs an explicit "finished"
state, which belongs to the sync/library layer.

### 5. Book identity ≠ file identity

chapbook-library keys books by a library id; the file hash is stored as an
*edition fingerprint*. A changed fingerprint triggers the re-anchor chain
(step 3), never orphans state.

## Why this shape (future-proofing)

The layered record maps field-for-field onto the open sync/interchange formats
this design should stay compatible with, so sync later is serialization, not
redesign:

- **OPDS Progression 1.0** (draft): `progression` = book_progression;
  `references` = `spine_href#:~:text=prefix-,exact,-suffix`.
- **Readium locators / Readium Annotations** (W3C Web Annotation profile;
  Thorium imports/exports it): `href`/`locations.progression`/
  `locations.totalProgression`/`text.before|highlight|after` map directly.

Schema hygiene for the same reason: stable ids, updated-at timestamps, and
soft deletes on positions/annotations, even while everything is single-device.
