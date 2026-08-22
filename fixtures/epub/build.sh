#!/usr/bin/env bash
# Rebuild the checked-in .epub fixtures from their source directories.
# EPUB requires the `mimetype` entry first and stored (uncompressed).
set -euo pipefail
cd "$(dirname "$0")"

for src in src/*/; do
    name=$(basename "$src")
    out="$PWD/$name.epub"
    rm -f "$out"
    (
        cd "$src"
        zip -X0 -q "$out" mimetype
        zip -Xr9 -q "$out" . -x mimetype
    )
    echo "built $name.epub"
done
