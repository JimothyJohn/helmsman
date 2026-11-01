#!/usr/bin/env bash
# Puts the real .wasm size into a page trunk built. trunk 0.21 writes the size
# from before wasm-opt into the page's loader, so the loading bar would stop
# near three quarters and then jump. tests/playground_page.rs checks the result.
#
# Usage: scripts/patch-wasm-size.sh DIST_DIR   (holding index.html and one .wasm)
set -euo pipefail
dist="${1:?usage: patch-wasm-size.sh DIST_DIR}"
page="$dist/index.html"
wasm="$(find "$dist" -maxdepth 1 -name '*_bg.wasm' -print -quit)"
test -n "$wasm" || { echo "no .wasm in $dist" >&2; exit 1; }
size="$(wc -c <"$wasm" | tr -d ' ')"
sed -E "s/(__trunkInitializer\(init, '[^']*_bg\.wasm', )[0-9]+,/\1$size,/" "$page" >"$page.tmp"
mv "$page.tmp" "$page"
grep -q "_bg\.wasm', $size," "$page" || { echo "no loader to patch in $page" >&2; exit 1; }
