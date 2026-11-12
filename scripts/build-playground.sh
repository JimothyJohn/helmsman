#!/usr/bin/env bash
# Build the public ST playground (the canvas in playground mode) for the
# browser: crates/helmsman-canvas/playground.html -> OUT_DIR/index.html plus
# its .js loader and .wasm (wasm-opt -Oz through trunk).
#
# Usage: scripts/build-playground.sh [OUT_DIR] [PUBLIC_URL]
#   default OUT_DIR    docs/playground   (GitHub Pages serves master's docs/)
#   default PUBLIC_URL /playground/ (the site is at the root of helmsman.advin.io)
#   HELMSMAN_FREE_CHAT_URL=https://...  the free chat's address (the relay's
#     demo function, its DemoUrl output, no trailing slash): the Agent tab
#     then sends there and has no key field. Unset: visitors paste their
#     own model API key, as before. The script says which it built.
# Needs: trunk (`cargo install trunk --locked`); adds the wasm32 target.
# The built files are committed: Pages builds nothing, it serves docs/ as is,
# and nothing goes live until dev is merged to master.
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-docs/playground}"
public_url="${2:-/playground/}"
command -v trunk >/dev/null || { echo "needs trunk: cargo install trunk --locked" >&2; exit 1; }
# The free chat's address goes into the page's code and into its
# Content-Security-Policy, so it must be an origin and nothing more.
free="${HELMSMAN_FREE_CHAT_URL:-}"
free="${free%/}"
if [ -n "$free" ]; then
    if ! printf '%s' "$free" | grep -Eq '^https?://[A-Za-z0-9.-]+(:[0-9]+)?$'; then
        echo "HELMSMAN_FREE_CHAT_URL must be https://host (no path): got '$free'" >&2
        exit 1
    fi
    export HELMSMAN_FREE_CHAT_URL="$free"
fi
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
# The page may connect to itself and to one other origin: the model API for
# a visitor's own key, or the free chat instead of it. A page on the free
# chat has no key, so it gets no way to the model API at all.
key_csp="connect-src 'self' https://api.anthropic.com;"
grep -qF "$key_csp" "$dist/index.html" \
    || { echo "the page's connect-src is not the one this script knows; update both" >&2; exit 1; }
if [ -n "$free" ]; then
    sed -i.bak "s|$key_csp|connect-src 'self' $free;|" "$dist/index.html"
    rm -f "$dist/index.html.bak"
    grep -qF "connect-src 'self' $free;" "$dist/index.html" \
        || { echo "could not set the page's connect-src to $free" >&2; exit 1; }
    case "$free" in
        https://*) ;;
        *) echo "warning: $free is not https; a page served over https cannot call it" >&2 ;;
    esac
    echo "Agent tab: the free chat at $free (no key field; the page may connect only there and to itself)"
else
    echo "Agent tab: the visitor's own key (HELMSMAN_FREE_CHAT_URL is not set)"
fi
rm -rf "$out"
mkdir -p "$out"
cp -R "$dist"/. "$out"/
echo "playground built into $out (public URL $public_url):"
ls -l "$out"
du -sh "$out"
