# OPDS fixture corpus

Wire-format fixtures for opds-client. The Atom/JSON pairs were
generated from a reference OPDS server implementation and deliberately
exercise every feature and quirk `crates/opds-client/INTEROP.md` requires
the client to handle; `crates/opds-client/tests/fixtures.rs` parses every
one of them.

| File | What it exercises |
|---|---|
| `navigation.atom.xml` / `navigation.opds2.json` | Navigation feed: nav entries with/without rel, a group with publications (2.0 `groups[]`; in 1.2 the group flattens to entries carrying `rel="collection"` links), search link. |
| `acquisition.atom.xml` / `acquisition.opds2.json` | Acquisition feed, page 2 of a paginated set: `previous`+`next` (no `first`/`last`; rel is `previous`, not `prev`), OpenSearch counters (1.2) vs `numberOfItems`/`currentPage` (2.0), three facets across two groups with `opds:activeFacet` (1.2 only — 2.0 drops the active flag), two publications incl. the PSE comic. |
| `entry.atom.xml` / `publication.opds2.json` | Standalone complete entry: multi-author (object + bare-string forms in 2.0), summary (text) vs content (HTML), series (`belongsTo`, 2.0 only), cover + thumbnail (2.0 images carry no rel — first is cover), buy w/ price + nested indirect acquisition (LCP→EPUB), borrow w/ lending extension (`opds:availability`/`holds`/`copies` — schema-invalid by design in 1.2), open-access. |
| `entry-pse.atom.xml` | Comic entry with OPDS-PSE 1.2 stream link: `pse:count`, `pse:lastRead`, `pse:lastReadDate`, literal `{pageNumber}`/`{maxWidth}` tokens in the href (not a valid URI — do not strict-parse), CBZ + EPUB open-access pair. PSE never appears in 2.0 output. |
| `pse-feed-golden.atom.xml` | Byte-exact golden PSE acquisition feed (cover, thumbnail, open-access CBZ, stream links with and without lastRead). Treat as immutable. |
| `opensearch.xml` | OpenSearch description document (1.x search discovery). |
| `authentication.opds-auth.json` | OPDS Authentication Document 1.0 (`application/opds-authentication+json`) with the Basic flow — what a server returns alongside 401. Hand-written from the spec. |
| `schema/` | Official OPDS 2.0 JSON Schemas + vendored Readium Web Publication Manifest schema tree (offline `$id`s; from drafts.opds.io / readium). Use to validate the 2.0 parser corpus in tests — but never as a gate on *received* feeds: PSE and lending output is schema-invalid by design. |

All dates in generated fixtures are the fixed timestamp `2026-01-02T03:04:05Z`
so files are reproducible. Media types use no space after `;`
(`application/atom+xml;profile=opds-catalog;kind=acquisition`) — compare
parsed type+parameters, never raw strings.
