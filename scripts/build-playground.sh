#!/usr/bin/env bash
# Build the public ST playground (the canvas in playground mode) for the
# browser: crates/helmsman-canvas/playground.html -> OUT_DIR/index.html plus
# its .js loader and .wasm (wasm-opt -Oz through trunk).
#
# Usage: scripts/build-playground.sh [OUT_DIR] [PUBLIC_URL]
#   default OUT_DIR    docs/playground   (GitHub Pages serves master's docs/)
#   default PUBLIC_URL /playground/ (the site is at the root of helmsman.advin.io)
# Needs: trunk (`cargo install trunk --locked`); adds the wasm32 target.
# The built files are committed: Pages builds nothing, it serves docs/ as is,
# and nothing goes live until dev is merged to master.
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-docs/playground}"
public_url="${2:-/playground/}"
command -v trunk >/dev/null || { echo "needs trunk: cargo install trunk --locked" >&2; exit 1; }
rustup target add wasm32-unknown-unknown >/dev/null
# Smaller wasm for a public page (6.2 MB -> 4.7 MB on 2026-10-31), for this
# build only: the workspace's release profile (the box) is untouched.
export CARGO_PROFILE_RELEASE_LTO="${CARGO_PROFILE_RELEASE_LTO:-true}"
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS="${CARGO_PROFILE_RELEASE_CODEGEN_UNITS:-1}"
export CARGO_PROFILE_RELEASE_OPT_LEVEL="${CARGO_PROFILE_RELEASE_OPT_LEVEL:-s}"
dist="$PWD/target/playground-dist"
rm -rf "$dist"
(cd crates/helmsman-canvas && trunk build --release --public-url "$public_url" --dist "$dist" playground.html)
# trunk names the page after its source; the site wants index.html.
if [ -f "$dist/playground.html" ]; then
    mv "$dist/playground.html" "$dist/index.html"
fi
test -f "$dist/index.html" || { echo "trunk produced no page in $dist" >&2; exit 1; }
ls "$dist"/*.wasm >/dev/null 2>&1 || { echo "trunk produced no .wasm in $dist" >&2; exit 1; }
./scripts/patch-wasm-size.sh "$dist"
rm -rf "$out"
mkdir -p "$out"
cp -R "$dist"/. "$out"/
echo "playground built into $out (public URL $public_url):"
ls -l "$out"
du -sh "$out"
