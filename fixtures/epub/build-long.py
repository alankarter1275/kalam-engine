#!/usr/bin/env python3
"""Rebuild fixtures/epub/long.epub: a reflowable book long enough to
paginate. Pure stdlib, deterministic (fixed timestamps, seeded prose).

The other EPUB fixtures are built by build.sh from checked-in source trees,
because they are small enough to read and every byte in them is deliberate.
This one is not: its whole point is *length* — enough chapters, each enough
pages, that a reader can walk out of the unit it started in and keep going
to the end of the book. Sixty kilobytes of generated filler is not worth
reading in a diff, so the generator is the source and the .epub is the
build output.

What needs it: the restored-position tests in chapbook-reader and
chapbook-ffi. A restore is resolved against the unit it was captured in,
and proving that takes a book with units to be wrong about. Those tests
used to reach into fixtures/corpus/, which is a download, so a clean
checkout failed.

The prose is filler, but not uniform filler: sentence and paragraph lengths
vary per chapter from a seeded generator, so lines break at different
places and pages do not all hold exactly the same amount. Uniform text
paginates too tidily to be a fair test.
"""
import os
import zipfile

CHAPTERS = 8
# Tuned against the tests' 600x800 page with 40pt margins and the vendored
# Crimson Text: enough words that a chapter is two to four pages, so eight
# of them make a book of roughly twenty. See `long_enough_to_walk_out_of`
# in chapbook-reader's session tests, which asserts the shape this aims at
# rather than trusting this comment.
WORDS_PER_CHAPTER = 1100

WORDS = (
    "acorn amber anvil ash beacon bellows birch bramble brine candle cedar "
    "chalk cinder clay clover copper cotton crane dawn dusk ember fathom "
    "fern flint frost gable garnet gorse granite harbour hazel heather "
    "hollow ink iron kestrel lantern larch ledger lichen linen loam marsh "
    "meadow mercury millstone moss nettle oakum ochre orchard osier paper "
    "peat pewter pitch quarry quill reed rowan rush saffron sage salt "
    "sandstone sedge shale sorrel spindle stone tallow tansy thatch thistle "
    "thorn tide timber tin vellum verge vetch wharf willow wicker yarrow"
).split()

VERBS = (
    "gathers settles turns weathers holds carries answers keeps marks "
    "opens rests shifts stands waits weighs counts"
).split()

JOINERS = (
    "and so", "which is why", "though only", "except that", "and after",
    "as if", "until", "because", "wherever", "long after",
)


class Lcg:
    """A 32-bit LCG, so the book is the same on every Python and every host.
    `random` would do, but its stability is a promise about a library and
    this is a promise about a fixture."""

    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFF

    def next(self, bound):
        self.state = (1103515245 * self.state + 12345) & 0x7FFFFFFF
        return self.state % bound

    def pick(self, seq):
        return seq[self.next(len(seq))]


def sentence(rng):
    """Six to twenty words, so lines break unevenly."""
    subject = rng.pick(WORDS)
    parts = [f"The {subject} {rng.pick(VERBS)}"]
    for _ in range(rng.next(3)):
        parts.append(f"{rng.pick(JOINERS)} the {rng.pick(WORDS)} "
                     f"{rng.pick(VERBS)} the {rng.pick(WORDS)}")
    return " ".join(parts).rstrip() + "."


def chapter_xhtml(number, rng):
    """One chapter: a heading, then paragraphs until the word budget runs
    out. The first paragraph carries `class="first"` like the other
    fixtures, so the same stylesheet means the same thing here."""
    paragraphs, words = [], 0
    while words < WORDS_PER_CHAPTER:
        sentences = [sentence(rng) for _ in range(3 + rng.next(4))]
        text = " ".join(sentences)
        words += len(text.split())
        paragraphs.append(text)

    body = [f'  <section class="chapter" id="ch{number}">',
            f"    <h1>Chapter {number}</h1>"]
    for i, text in enumerate(paragraphs):
        cls = ' class="first"' if i == 0 else ""
        body.append(f"    <p{cls}>{text}</p>")
    body.append("  </section>")
    joined = "\n".join(body)
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en" lang="en">
<head>
  <title>Chapter {number}</title>
  <link rel="stylesheet" type="text/css" href="style.css"/>
</head>
<body>
{joined}
</body>
</html>
"""


def nav_xhtml():
    items = "\n".join(
        f'      <li><a href="chapter{n}.xhtml">Chapter {n}</a></li>'
        for n in range(1, CHAPTERS + 1)
    )
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="en" lang="en">
<head>
  <title>Contents</title>
</head>
<body>
  <nav epub:type="toc" id="toc">
    <h1>Contents</h1>
    <ol>
{items}
    </ol>
  </nav>
</body>
</html>
"""


def package_opf():
    manifest = "\n".join(
        f'    <item id="ch{n}" href="chapter{n}.xhtml" '
        f'media-type="application/xhtml+xml"/>'
        for n in range(1, CHAPTERS + 1)
    )
    spine = "\n".join(f'    <itemref idref="ch{n}"/>'
                      for n in range(1, CHAPTERS + 1))
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id" xml:lang="en">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="pub-id">urn:uuid:3c5b7a02-1f10-4e3c-9e40-chapbook0005</dc:identifier>
    <dc:title>The Long Book</dc:title>
    <dc:creator id="creator">Ada Fixture</dc:creator>
    <dc:language>en</dc:language>
    <meta property="dcterms:modified">2026-08-27T00:00:00Z</meta>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
{manifest}
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine>
{spine}
  </spine>
</package>
"""


CONTAINER = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"""

# Deliberately the same rules as fixtures/epub/src/minimal/OEBPS/style.css,
# minus the selectors no chapter here uses. A book that paginates should
# paginate under the stylesheet the other fixtures are read with.
STYLE = """.chapter {
  font-family: serif;
}

h1 {
  text-align: center;
  page-break-after: avoid;
}

p {
  margin: 0;
  text-indent: 1.2em;
}

p.first {
  text-indent: 0;
}
"""

STAMP = (2026, 8, 27, 0, 0, 0)


def add(zf, name, data, compress):
    info = zipfile.ZipInfo(name, date_time=STAMP)
    info.compress_type = compress
    info.external_attr = 0o644 << 16
    zf.writestr(info, data)


def main():
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "long.epub")
    with zipfile.ZipFile(out, "w") as zf:
        # EPUB requires `mimetype` first and stored, which is the whole
        # reason Format::sniff can read one from its first bytes.
        add(zf, "mimetype", b"application/epub+zip", zipfile.ZIP_STORED)
        add(zf, "META-INF/container.xml", CONTAINER, zipfile.ZIP_DEFLATED)
        add(zf, "OEBPS/package.opf", package_opf(), zipfile.ZIP_DEFLATED)
        add(zf, "OEBPS/nav.xhtml", nav_xhtml(), zipfile.ZIP_DEFLATED)
        add(zf, "OEBPS/style.css", STYLE, zipfile.ZIP_DEFLATED)
        for n in range(1, CHAPTERS + 1):
            # Seeded per chapter, so editing one chapter's budget does not
            # reshuffle every chapter after it.
            rng = Lcg(0x00C4B00C + n * 7919)
            add(zf, f"OEBPS/chapter{n}.xhtml", chapter_xhtml(n, rng),
                zipfile.ZIP_DEFLATED)
    print(f"built long.epub ({os.path.getsize(out)} bytes, {CHAPTERS} chapters)")


if __name__ == "__main__":
    main()
