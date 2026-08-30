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
  `references` = `spine_href#:~:text=prefix-,exact,-suffix`. The client
  for it is `opds_client::progression`, behind a feature while the draft
  is unreleased; the mapping above is still the mapping, unwritten.
- **Readium locators / Readium Annotations** (W3C Web Annotation profile;
  Thorium imports/exports it): `href`/`locations.progression`/
  `locations.totalProgression`/`text.before|highlight|after` map directly.

Schema hygiene for the same reason: stable ids, updated-at timestamps, and
soft deletes on positions/annotations, even while everything is single-device.
