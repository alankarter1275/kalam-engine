#!/usr/bin/env python3
"""Rebuild fixtures/cbz/minimal.cbz: three solid-color pages named to
exercise natural sort, plus junk entries readers must skip. Pure stdlib,
deterministic (fixed timestamps, stored entries)."""
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

out = os.path.join(os.path.dirname(__file__), "minimal.cbz")
pages = [
    ("page1.png", png(120, 180, (196, 64, 48))),    # red
    ("page2.png", png(120, 180, (52, 120, 82))),    # green
    ("page10.png", png(120, 180, (58, 78, 160))),   # blue - sorts LAST
]
junk = [
    ("ComicInfo.xml", b"<ComicInfo><Title>Minimal</Title></ComicInfo>"),
    ("__MACOSX/page1.png", b"junk"),
    (".hidden.png", b"junk"),
]
with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as z:
    # Write out of order so reading order must come from sorting.
    for name, data in [pages[2], pages[0], junk[0], pages[1], junk[1], junk[2]]:
        info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
        z.writestr(info, data)
print(f"built {out}")
