#!/usr/bin/env bash
# Rebuild the checked-in .epub fixtures from their source directories.
# EPUB requires the `mimetype` entry first and stored (uncompressed).
#
# long.epub is not built here and has no source directory: it is generated
# filler, long enough to paginate, and lives in build-long.py instead.
#
# Name one or more fixtures to rebuild just those:
#
#     ./build.sh bidi
#
# Worth doing. zip stores a timestamp per entry, so rebuilding a fixture
# nobody touched still rewrites its bytes, and a diff that says every
# fixture changed is a diff nobody reads.
set -euo pipefail
cd "$(dirname "$0")"

if [ "$#" -gt 0 ]; then
    sources=()
    for name in "$@"; do
        [ -d "src/$name" ] || { echo "no such fixture source: src/$name" >&2; exit 1; }
        sources+=("src/$name/")
    done
else
    sources=(src/*/)
fi

for src in "${sources[@]}"; do
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
