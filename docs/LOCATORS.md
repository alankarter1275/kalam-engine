# Locator rationale

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
layers several locators and degrades gracefully. Chapbook does the same:
every persisted position and annotation *endpoint* is a layered record —
exact offset, quote context, spine fraction, whole-book progression — with a
resolve chain that degrades to "right page-ish", never "gone". (Endpoints,
not just positions, because a drifted highlight silently marks the wrong
text, which is worse than a lost bookmark.)

## Where the spec lives

The normative definition — the versioned locator-text extraction function
(`LOCATOR_VERSION`), the `LayeredLocator` record, the resolve chain, and the
per-format progression denominators — is the `chapbook_core::locator` module
documentation, kept honest by the golden offset-map test in chapbook-layout
(changing the extraction is a conscious act, not an accident). This document
is why that shape was chosen, not what it is.

## Book identity ≠ file identity

chapbook-library keys books by a library id; the file hash is stored as an
*edition fingerprint*. A changed fingerprint triggers the re-anchor chain,
never orphans state.

## Why this shape (future-proofing)

The layered record maps field-for-field onto the open sync/interchange formats
this design should stay compatible with, so sync later is serialization, not
redesign:

- **OPDS Progression 1.0** (draft): `progression` = book_progression;
  `references` = `spine_href#:~:text=prefix-,exact,-suffix`. The wire half
  is `opds_client::progression` and the mapping is
  `chapbook_opds::progression`, both behind a feature while the draft is
  unreleased. A *point* has no `exact`, and a text fragment cannot match
  emptily, so it is written as the run starting at it —
  `text=prefix-,suffix` — and the match start is the point. `char_offset`
  deliberately does not cross: it means nothing off this device.
- **W3C Web Annotation** (the profile Readium and Thorium speak):
  `chapbook_annotations`. One endpoint becomes a selector stack —
  `TextQuoteSelector` for the quote layer, `TextPositionSelector` for the
  offset, `ProgressSelector` for book_progression — with the spine item on
  the target. The offset selector travels stamped with
  `chapbook:locatorVersion` and is only read back when the stamp matches,
  because an offset from an extraction we did not do would mark the wrong
  words with full confidence. Everything a peer sent and this crate does
  not model survives the round trip: a shared container means a lossy
  parse is somebody else's data loss.

Schema hygiene for the same reason: stable ids, updated-at timestamps, and
soft deletes on positions/annotations, even while everything is single-device.

## Version history

- 1 → 2: the MathML altimg/alttext fallback rewrite runs before extraction.
- 2 → 3 (kalam): content documents are parsed as XML first, HTML as the
  fallback (`chapbook-layout/src/dom/parse.rs`). The XML tree keeps the
  whitespace text node between `<html>` and `<head>` that the HTML parser
  discards (typically one `\n` at the front), and nests self-closed empty
  elements (`<a id="x"/>`) correctly where the HTML algorithm left them
  open, so offsets shift for well-formed chapters. See
  `docs/kalam/RESEARCH.md` R15 and R17.
