#!/usr/bin/env sh
# Build the handoff bundle for the calibre-alt (Kalam) repository.
#
# Run from the kalam-engine repository root, on a checkout of the arena
# branch:
#
#     sh docs/kalam/handoff/make-bundle.sh
#
# It writes ./kalam-handoff/ — a directory to copy into calibre-alt as
# docs/engine-handoff/ (see README.md inside it). Everything in it is
# derived from this repo, so rerun after any engine change and re-copy.
set -eu

cd "$(dirname "$0")/../../.."
rev=$(git rev-parse --short HEAD)
out=kalam-handoff
rm -rf "$out"
mkdir -p "$out/patch"

cp docs/kalam/handoff/AGENT-BRIEF.md   "$out/README.md"
cp docs/kalam/INTEGRATION.md           "$out/INTEGRATION.md"
cp docs/kalam/patch/src/pages/reader/engine.rs "$out/patch/engine.rs"
cp crates/kalam-reader/README.md       "$out/kalam-reader-README.md"
python3 docs/kalam/handoff/api-reference.py > "$out/kalam-reader-API.md"

# The exact commit the bundle describes, so both sides can say "engine
# at <rev>" and mean the same code.
printf 'kalam-engine commit: %s\nbranch: %s\ngenerated: %s\n' \
    "$rev" "$(git rev-parse --abbrev-ref HEAD)" "$(date -u +%Y-%m-%dT%H:%MZ)" \
    > "$out/ENGINE-VERSION.txt"

echo "wrote ./$out (engine $rev):"
ls -1 "$out" "$out/patch"
echo
echo "Next: copy it into calibre-alt as docs/engine-handoff/ and commit:"
echo "    rm -rf ../calibre-alt/docs/engine-handoff && cp -r $out ../calibre-alt/docs/engine-handoff"
echo "(adjust the path to wherever calibre-alt is checked out)"
