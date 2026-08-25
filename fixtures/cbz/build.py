#!/usr/bin/env python3
"""Rebuild the CBZ fixtures. Pure stdlib, deterministic (fixed timestamps,
stored entries).

minimal.cbz  three solid-color pages named to exercise natural sort, junk
             entries readers must skip, and a ComicInfo.xml carrying the
             metadata and page bookmarks a tagged archive has.
bare.cbz     the same pages with no sidecar at all — the majority of comics
             in the wild, and the fallback path.
unnamed.cbz  pages with no extension to classify them by, plus a non-image
             member, so page detection has to reach for the bytes."""
import struct, zlib, zipfile, os

def png(width, height, rgb):
    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c))
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    row = b"\x00" + bytes(rgb) * width
    idat = zlib.compress(row * height, 9)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
            + chunk(b"IDAT", idat) + chunk(b"IEND", b""))

here = os.path.dirname(__file__)
pages = [
    ("page1.png", png(120, 180, (196, 64, 48))),    # red
    ("page2.png", png(120, 180, (52, 120, 82))),    # green
    ("page10.png", png(120, 180, (58, 78, 160))),   # blue - sorts LAST
]

# An ampersand and a two-role credit, because both are places a naive
# reader loses data. Bookmarks index the *sorted* pages, so Image="2" is
# page10.png.
comic_info = b"""<?xml version="1.0" encoding="utf-8"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <Series>Cogs &amp; Levers</Series>
  <Number>3</Number>
  <Summary>Three plates in primary colors.</Summary>
  <Writer>Ada Lovelace</Writer>
  <Penciller>Ada Lovelace, Grace Hopper</Penciller>
  <LanguageISO>en</LanguageISO>
  <PageCount>3</PageCount>
  <Pages>
    <Page Image="0" Type="FrontCover" />
    <Page Image="1" Bookmark="The Escapement" />
    <Page Image="2" Bookmark="Afterword" />
  </Pages>
</ComicInfo>"""

sidecar = ("ComicInfo.xml", comic_info)
junk = [
    ("__MACOSX/page1.png", b"junk"),
    (".hidden.png", b"junk"),
]

def build(name, members):
    out = os.path.join(here, name)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as z:
        for member, data in members:
            info = zipfile.ZipInfo(member, date_time=(2026, 1, 1, 0, 0, 0))
            z.writestr(info, data)
    print(f"built {out}")

# Written out of order so reading order must come from sorting.
build("minimal.cbz", [pages[2], pages[0], sidecar, pages[1], junk[0], junk[1]])
build("bare.cbz", [pages[2], pages[0], pages[1]])
build("unnamed.cbz", [
    ("003", pages[2][1]),
    ("001", pages[0][1]),
    ("002", pages[1][1]),
    ("notes", b"just some text, not a page"),
])
