#!/usr/bin/env bash
# Serve the canvas with a rebuild on every edit, reachable from the LAN.
# trunk rebuilds (`trunk watch`) and python's http.server serves the
# result: trunk's own server is an unsigned binary the Studio's firewall
# holds behind a GUI prompt, so from another machine it never answered;
# python is already allowed. No box API behind it: open /?playground (or
# "Simulate in this browser" once the box is unreachable). Refresh the
# page after an edit (no auto-reload).
#
# Usage: scripts/canvas-serve.sh [PORT]      # default 8790
# Stop:  Ctrl-C (stops both), or kill $(lsof -tiTCP:PORT -sTCP:LISTEN)
set -euo pipefail
cd "$(dirname "$0")/.."
port="${1:-8790}"
command -v trunk >/dev/null || { echo "needs trunk: cargo install trunk --locked" >&2; exit 1; }
rustup target add wasm32-unknown-unknown >/dev/null
if lsof -tiTCP:"$port" -sTCP:LISTEN >/dev/null; then
    echo "port $port is taken: kill \$(lsof -tiTCP:$port -sTCP:LISTEN)" >&2
    exit 1
fi
dist="$PWD/target/canvas-dist"
mkdir -p "$dist"
(cd crates/helmsman-canvas && trunk watch --release --dist "$dist") &
watch=$!
trap 'kill "$watch" 2>/dev/null; wait "$watch" 2>/dev/null || true' EXIT
# The first build must land before the page is worth serving.
until [ -f "$dist/index.html" ]; do sleep 1; done
echo "serving $dist on http://0.0.0.0:$port/ (from the Air: http://10.0.0.41:$port/?playground)"
exec /usr/bin/python3 -m http.server "$port" --bind 0.0.0.0 --directory "$dist"
