#!/usr/bin/env bash
# Render the one-page datasheet: docs/datasheet.html -> docs/helmsman-datasheet.pdf,
# through a headless Chrome or Chromium (CHROME=/path overrides the search).
# The fonts come from Google Fonts at render time, so it needs the network.
# Checks the result is exactly one page when pypdf is installed. Idempotent;
# `make datasheet`. The PDF is committed, so run this after editing the HTML.
set -euo pipefail
cd "$(dirname "$0")/.."

src="$PWD/docs/datasheet.html"
out="$PWD/docs/helmsman-datasheet.pdf"

chrome="${CHROME:-}"
if [ -z "$chrome" ]; then
    for c in "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
             "/Applications/Chromium.app/Contents/MacOS/Chromium" \
             google-chrome chromium chromium-browser; do
        if [ -x "$c" ] || command -v "$c" >/dev/null 2>&1; then chrome="$c"; break; fi
    done
fi
[ -n "$chrome" ] || { echo "needs Chrome or Chromium (CHROME=/path/to/binary)" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$chrome" --headless=new --disable-gpu --hide-scrollbars \
    --user-data-dir="$tmp/profile" \
    --no-pdf-header-footer --virtual-time-budget=10000 \
    --print-to-pdf="$out" "file://$src" >"$tmp/chrome.log" 2>&1 \
    || { cat "$tmp/chrome.log" >&2; exit 1; }
[ -s "$out" ] || { echo "no PDF written" >&2; cat "$tmp/chrome.log" >&2; exit 1; }

if python3 -c 'import pypdf' 2>/dev/null; then
    pages="$(python3 -c 'import sys, pypdf; print(len(pypdf.PdfReader(sys.argv[1]).pages))' "$out")"
    [ "$pages" = 1 ] || { echo "datasheet is $pages pages; it must fit on one" >&2; exit 1; }
    echo "docs/helmsman-datasheet.pdf: 1 page, $(wc -c <"$out" | tr -d ' ') bytes"
else
    echo "docs/helmsman-datasheet.pdf written ($(wc -c <"$out" | tr -d ' ') bytes); pypdf missing, page count not checked"
fi
