# Archived: the original kalam-engine docs

**These documents are superseded. Do not follow them.**

They describe the first idea for `kalam-engine`: a text engine written from
scratch on `lol_html` + `cosmic-text` + `tiny-skia`, with publisher CSS
stripped entirely and a ban on the `stylo` CSS engine. About 100 lines of
placeholder code were written against that design before it was abandoned
on 2026-09-08 in favour of forking Chapbook.

Kept for the record, because parts of the *intent* still hold and are
carried forward in [`../PLAN.md`](../PLAN.md):

- no WebKit, no browser engine, no JavaScript — still true;
- Kalam's theme is authoritative over the publisher's — still the goal,
  now achieved through Chapbook's user-origin theme sheets rather than by
  stripping CSS;
- reading positions anchored to text, not to layout — still true, now via
  Chapbook's `LayeredLocator`, which is a better version of the same idea;
- no database, no networking, no PDF/comics inside the engine — still true.

What changed: the ban on `stylo` is lifted (its stated reason, a C++
toolchain requirement, was incorrect; it is heavy to compile but light to
run), and "build it" became "fork it and strip it".

| File | Was |
|---|---|
| `README.md` | Original front page |
| `ARCHITECTURE.md` | The six-stage pipeline that was never built |
| `RESTRICTIONS.md` | Guardrails; see above for which still apply |
| `ROADMAP.md` | Six phases, none completed |
