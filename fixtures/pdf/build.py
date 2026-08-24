#!/usr/bin/env python3
"""Rebuild fixtures/pdf/minimal.pdf: three 300x400pt pages (two solid colored
rectangles and a text page), an Info dict, and a document outline. Pure
stdlib, deterministic.

The outline is deliberately awkward, because real ones are: a direct /Dest
array, a /GoTo action instead of a /Dest, a nested child reached by a *named*
destination through the /Names name tree, a UTF-16BE title, and a /URI action
that points out of the document and so must resolve to no page at all.
"""
import os

def obj(n, body):
    return f"{n} 0 obj\n{body}\nendobj\n".encode()

def utf16be(s):
    """A PDF hex string holding UTF-16BE text, BOM and all."""
    return "<" + "FEFF" + s.encode("utf-16-be").hex().upper() + ">"

pages = [
    ("0.77 0.25 0.19 rg 0 0 300 400 re f", False),   # red page
    ("0.23 0.30 0.63 rg 0 0 300 400 re f", False),   # blue page
    # Text page: two Helvetica lines for text-layer extraction tests.
    ("BT /F1 24 Tf 40 340 Td (Hello selection) Tj ET "
     "BT /F1 24 Tf 40 300 Td (Second line here) Tj ET", True),
]
objects = []
objects.append(obj(1, "<< /Type /Catalog /Pages 2 0 R /Outlines 11 0 R /Names 15 0 R >>"))
objects.append(obj(2, "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 >>"))
for i, (content, has_text) in enumerate(pages):
    page_num, content_num = 3 + 2 * i, 4 + 2 * i
    resources = " /Resources << /Font << /F1 9 0 R >> >>" if has_text else ""
    objects.append(obj(page_num,
        f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents {content_num} 0 R{resources} >>"))
    stream = content.encode()
    objects.append(obj(content_num,
        f"<< /Length {len(stream)} >>\nstream\n{content}\nendstream"))
objects.append(obj(9, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"))
objects.append(obj(10, "<< /Title (Minimal Fixture) /Author (Chapbook Tests) >>"))

# Outline: two top-level entries plus an out-of-document one, and a child
# under the second.
objects.append(obj(11, "<< /Type /Outlines /First 12 0 R /Last 14 0 R /Count 4 >>"))
objects.append(obj(12,
    "<< /Title (Red plate) /Parent 11 0 R /Next 13 0 R /Dest [3 0 R /Fit] >>"))
objects.append(obj(13,
    f"<< /Title {utf16be('Blau — Seite zwei')} /Parent 11 0 R /Prev 12 0 R "
    "/Next 14 0 R /First 16 0 R /Last 16 0 R /Count 1 "
    "/A << /S /GoTo /D [5 0 R /XYZ 0 400 0] >> >>"))
objects.append(obj(14,
    "<< /Title (Elsewhere) /Parent 11 0 R /Prev 13 0 R "
    "/A << /S /URI /URI (https://example.invalid/) >> >>"))
# Named-destination store, reached from the catalog's /Names.
objects.append(obj(15, "<< /Dests 17 0 R >>"))
objects.append(obj(16, "<< /Title (Text page) /Parent 13 0 R /Dest (text-page) >>"))
objects.append(obj(17, "<< /Names [ (text-page) [7 0 R /Fit] ] >>"))

out = bytearray(b"%PDF-1.4\n")
offsets = []
for o in objects:
    offsets.append(len(out))
    out += o
xref_at = len(out)
out += f"xref\n0 {len(objects)+1}\n0000000000 65535 f \n".encode()
for off in offsets:
    out += f"{off:010} 00000 n \n".encode()
out += (f"trailer\n<< /Size {len(objects)+1} /Root 1 0 R /Info 10 0 R >>\n"
        f"startxref\n{xref_at}\n%%EOF\n").encode()

path = os.path.join(os.path.dirname(__file__), "minimal.pdf")
open(path, "wb").write(bytes(out))
print(f"built {path} ({len(out)} bytes)")
