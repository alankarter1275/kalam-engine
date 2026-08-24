#!/usr/bin/env python3
"""Rebuild fixtures/pdf/minimal.pdf: two 300x400pt pages, each a solid
colored rectangle, plus an Info dict. Pure stdlib, deterministic."""
import os

def obj(n, body):
    return f"{n} 0 obj\n{body}\nendobj\n".encode()

pages = [
    ("0.77 0.25 0.19 rg 0 0 300 400 re f",),   # red page
    ("0.23 0.30 0.63 rg 0 0 300 400 re f",),   # blue page
]
objects = []
objects.append(obj(1, "<< /Type /Catalog /Pages 2 0 R >>"))
objects.append(obj(2, "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>"))
for i, (content,) in enumerate(pages):
    page_num, content_num = 3 + 2 * i, 4 + 2 * i
    objects.append(obj(page_num,
        f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents {content_num} 0 R >>"))
    stream = content.encode()
    objects.append(obj(content_num,
        f"<< /Length {len(stream)} >>\nstream\n{content}\nendstream"))
objects.append(obj(7, "<< /Title (Minimal Fixture) /Author (Chapbook Tests) >>"))

out = bytearray(b"%PDF-1.4\n")
offsets = []
for o in objects:
    offsets.append(len(out))
    out += o
xref_at = len(out)
out += f"xref\n0 {len(objects)+1}\n0000000000 65535 f \n".encode()
for off in offsets:
    out += f"{off:010} 00000 n \n".encode()
out += (f"trailer\n<< /Size {len(objects)+1} /Root 1 0 R /Info 7 0 R >>\n"
        f"startxref\n{xref_at}\n%%EOF\n").encode()

path = os.path.join(os.path.dirname(__file__), "minimal.pdf")
open(path, "wb").write(bytes(out))
print(f"built {path} ({len(out)} bytes)")
