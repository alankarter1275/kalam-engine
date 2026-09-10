#!/usr/bin/env python3
"""Generate a Markdown reference of kalam-reader's public API from its source.

Run from the kalam-engine repository root:

    python3 docs/kalam/handoff/api-reference.py > /path/to/kalam-reader-API.md

Extracts every `pub` item (with its `///` doc comment and, for structs and
enums, its body) from crates/kalam-reader/src/{lib,prefs,view}.rs and the
few chapbook-core types the crate re-exports. No dependencies beyond the
standard library, so it runs on the owner's machine as part of
make-bundle.sh and stays in step with the code by construction.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

FILES = [
    ("crates/kalam-reader/src/lib.rs", "Crate root and re-exports"),
    ("crates/kalam-reader/src/prefs.rs", "Preferences: KalamPrefs, KalamTheme, HighlightColor"),
    ("crates/kalam-reader/src/view.rs", "The widget: ReaderView and its callback types"),
]

# Re-exported chapbook types the host will touch, by (file, item name).
REEXPORTS = [
    ("crates/chapbook-core/src/locator.rs", "pub struct LayeredLocator"),
    ("crates/chapbook-core/src/locator.rs", "pub struct Quote"),
    ("crates/chapbook-core/src/locator.rs", "pub const LOCATOR_VERSION"),
    ("crates/chapbook-core/src/book.rs", "pub struct TocEntry"),
    ("crates/chapbook-core/src/geometry.rs", "pub struct Point"),
    ("crates/chapbook-core/src/geometry.rs", "pub struct Size"),
    ("crates/chapbook-core/src/geometry.rs", "pub struct Rect"),
    ("crates/chapbook-reader/src/lib.rs", "pub struct Highlight"),
]

ITEM_RE = re.compile(r"^(\s*)pub(?:\(crate\))? (fn|struct|enum|const|type|use) ")


def doc_above(lines, i):
    """The contiguous `///` block (and `#[derive]` lines) right above line i."""
    j = i - 1
    attrs = []
    while j >= 0 and lines[j].strip().startswith("#["):
        attrs.insert(0, lines[j].strip())
        j -= 1
    docs = []
    while j >= 0 and lines[j].strip().startswith("///"):
        docs.insert(0, lines[j].strip()[3:].strip())
        j -= 1
    return docs, attrs


def item_text(lines, i):
    """The source of the item starting at line i: a signature up to `{`/`;`
    for fns/consts/uses, the whole body for structs and enums."""
    kind = ITEM_RE.match(lines[i]).group(2)
    if kind in ("struct", "enum"):
        # Tuple/unit structs end with `;` on the first line.
        if lines[i].rstrip().endswith(";"):
            return [lines[i]]
        indent = len(lines[i]) - len(lines[i].lstrip())
        out = [lines[i]]
        for k in range(i + 1, len(lines)):
            out.append(lines[k])
            if lines[k].startswith(" " * indent + "}") and len(lines[k]) - len(lines[k].lstrip()) == indent:
                break
        return out
    out = []
    for k in range(i, len(lines)):
        line = lines[k]
        out.append(line.rstrip("\n").rstrip())
        stripped = line.rstrip()
        if stripped.endswith("{"):
            out[-1] = out[-1][:-1].rstrip()
            break
        if stripped.endswith(";"):
            break
    return out


def dedent(block):
    indents = [len(l) - len(l.lstrip()) for l in block if l.strip()]
    cut = min(indents) if indents else 0
    return [l[cut:].rstrip("\n") for l in block]


def emit_item(lines, i, out, keep_private=False):
    m = ITEM_RE.match(lines[i])
    if not m:
        return
    if "pub(crate)" in lines[i] and not keep_private:
        return
    docs, attrs = doc_above(lines, i)
    body = dedent(item_text(lines, i))
    # Strip doc comments inside struct/enum bodies down to the field line
    # plus its docs, which is what a reader of the reference wants.
    for d in docs:
        out.append(f"> {d}" if d else ">")
    out.append("")
    out.append("```rust")
    for a in attrs:
        out.append(a)
    out.extend(body)
    out.append("```")
    out.append("")


def scan_file(rel, title, out):
    path = ROOT / rel
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    out.append(f"## {title}")
    out.append("")
    out.append(f"`{rel}`")
    out.append("")
    in_impl = None
    in_tests = False
    for i, line in enumerate(lines):
        if line.startswith("#[cfg(test)]"):
            in_tests = True
        if in_tests:
            continue
        im = re.match(r"^impl(?:<[^>]*>)? (\w+)", line)
        if im:
            in_impl = im.group(1)
            out.append(f"### impl {in_impl}")
            out.append("")
            continue
        if line.startswith("}") and in_impl:
            in_impl = None
            continue
        if ITEM_RE.match(line):
            emit_item(lines, i, out)


def scan_reexport(rel, needle, out):
    path = ROOT / rel
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    for i, line in enumerate(lines):
        if re.match(re.escape(needle) + r"\b", line):
            out.append(f"`{rel}` — re-exported as `kalam_reader::{needle.split()[-1]}`")
            out.append("")
            emit_item(lines, i, out)
            return
    print(f"warning: {needle} not found in {rel}", file=sys.stderr)


def main():
    try:
        rev = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
    except Exception:
        rev = "unknown"
    out = [
        "# kalam-reader — public API reference",
        "",
        f"Generated by `docs/kalam/handoff/api-reference.py` from kalam-engine commit `{rev}`.",
        "Do not edit; regenerate. Doc comments are the crate's own.",
        "",
        "Everything a host touches is reachable as `kalam_reader::<Name>`.",
        "GTK types are gtk4-rs **0.11** (`gtk4::DrawingArea`, `gtk4::Adjustment`).",
        "",
    ]
    for rel, title in FILES:
        scan_file(rel, title, out)
    out.append("## Re-exported chapbook types")
    out.append("")
    for rel, needle in REEXPORTS:
        scan_reexport(rel, needle, out)
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
