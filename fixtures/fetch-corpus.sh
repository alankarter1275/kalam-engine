#!/usr/bin/env bash
# Download a real-world test corpus into fixtures/corpus/ (gitignored).
# Used by #[ignore]-by-default integration tests and manual pipeline runs.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p corpus
cd corpus

fetch() {
    local url=$1 out=$2
    if [[ -e $out ]]; then
        echo "have    $out"
    else
        echo "fetch   $out"
        curl -sSfL "$url" -o "$out"
    fi
}

# IDPF/W3C EPUB 3 sample documents (reflowable selection)
IDPF=https://github.com/IDPF/epub3-samples/releases/download/20230704
fetch "$IDPF/accessible_epub_3.epub" accessible_epub_3.epub
fetch "$IDPF/childrens-literature.epub" childrens-literature.epub
fetch "$IDPF/moby-dick.epub" moby-dick.epub

# Standard Ebooks (well-crafted, CSS-rich, liberally licensed)
SE=https://standardebooks.org/ebooks
fetch "$SE/jane-austen/pride-and-prejudice/downloads/jane-austen_pride-and-prejudice.epub?source=download" \
    se_pride-and-prejudice.epub
fetch "$SE/lewis-carroll/alices-adventures-in-wonderland/john-tenniel/downloads/lewis-carroll_alices-adventures-in-wonderland_john-tenniel.epub?source=download" \
    se_alice-in-wonderland.epub
fetch "$SE/h-g-wells/the-time-machine/downloads/h-g-wells_the-time-machine.epub?source=download" \
    se_the-time-machine.epub

echo "corpus ready: $(ls -1 | wc -l) files in fixtures/corpus/"
