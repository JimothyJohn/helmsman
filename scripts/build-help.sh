#!/usr/bin/env bash
# Build the help pages on the site: docs/help/*.md -> docs/help/*.html
# (GitHub Pages serves master's docs/, so the output is committed), with
# two pages generated from the code so they cannot drift: keyboard.md
# (the shortcut table, hotkeys::TABLE) and starters.md (the examples and
# the machines and templates, helmsman-project's STARTERS).
#
# Needs: pandoc (brew install pandoc), the workspace building natively.
# Runs here: `! scripts/build-help.sh`   or   `make help-site`.
# `--check` builds into a temp folder and fails if docs/help/ differs:
# the gate for "the committed pages are current".
set -euo pipefail
cd "$(dirname "$0")/.."
command -v pandoc >/dev/null || { echo "needs pandoc: brew install pandoc" >&2; exit 1; }

out="docs/help"
check=""
if [ "${1:-}" = "--check" ]; then
    check=1
    out="$(mktemp -d)/help"
    mkdir -p "$out"
    cp docs/help/*.md docs/help/template.html "$out/"
fi

# The generated pages, from the code.
cargo run -q -p helmsman-canvas -- --help-tables keyboard > "$out/keyboard.md"
cargo run -q -p helmsman-canvas -- --help-tables starters > "$out/starters.md"

# Every page, in nav order; the title is the first heading.
pages="index editor simulation projects box agent keyboard starters"
nav=""
for p in $pages; do
    title="$(grep -m1 '^# ' "$out/$p.md" | sed 's/^# //')"
    nav="$nav<a href=\"$p.html\">$title</a>"
done
for p in $pages; do
    title="$(grep -m1 '^# ' "$out/$p.md" | sed 's/^# //')"
    pandoc "$out/$p.md" --from gfm --to html5 --standalone \
        --template "$out/template.html" \
        --metadata "pagetitle=Helmsman Help · $title" \
        --variable "nav=$nav" \
        --variable "current=$p" \
        -o "$out/$p.html"
done

if [ -n "$check" ]; then
    for f in "$out"/*.html "$out"/keyboard.md "$out"/starters.md; do
        base="$(basename "$f")"
        if ! diff -q "$f" "docs/help/$base" >/dev/null; then
            echo "docs/help/$base is out of date: run scripts/build-help.sh and commit" >&2
            exit 1
        fi
    done
    echo "docs/help is current"
else
    echo "built $(echo $pages | wc -w | tr -d ' ') pages into $out"
fi
