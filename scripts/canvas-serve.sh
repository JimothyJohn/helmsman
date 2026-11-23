#!/usr/bin/env bash
# Serve the canvas with a rebuild on every edit, reachable from the LAN.
# trunk rebuilds (`trunk watch`) and python's http.server serves the
# result: trunk's own server is an unsigned binary the Studio's firewall
# holds behind a GUI prompt, so from another machine it never answered;
# python is already allowed. No box API behind it: open /?playground (or
# "Simulate in this browser" once the box is unreachable). Refresh the
# page after an edit (no auto-reload).
#
# The Agent tab talks to the relay's free chat (DeepSeek through
# OpenRouter), as the published playground does: HELMSMAN_FREE_CHAT_URL
# defaults to the relay's demo function, whose DemoOrigins include this
# page's LAN and loopback origins on :8790 (agent-relay stack, 2026-11-12).
# Set it empty for no free chat (HELMSMAN_FREE_CHAT_URL= ...): the Agent tab
# then says it is not available, unless the local proxy answers.
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
export HELMSMAN_FREE_CHAT_URL="${HELMSMAN_FREE_CHAT_URL-https://iup63z6bv5rz2fwtwexyw3pvoi0spenq.lambda-url.us-east-1.on.aws}"
if [ -n "$HELMSMAN_FREE_CHAT_URL" ]; then
    echo "Agent tab: the free chat at $HELMSMAN_FREE_CHAT_URL"
else
    echo "Agent tab: no free chat (HELMSMAN_FREE_CHAT_URL is empty)"
fi
# The design entry (the relay's DemoDesignUrl) has no default: the entry
# is there only when the address is set (HELMSMAN_DESIGN_URL=https://...).
export HELMSMAN_DESIGN_URL="${HELMSMAN_DESIGN_URL-}"
if [ -n "$HELMSMAN_DESIGN_URL" ]; then
    echo "Design entry: at $HELMSMAN_DESIGN_URL"
else
    echo "Design entry: none (HELMSMAN_DESIGN_URL is empty)"
fi
dist="$PWD/target/canvas-dist"
mkdir -p "$dist"
# The page waits for the first build below; a stale index.html from the
# last run would let it serve the previous build while trunk compiles
# (a pull was "not showing" for that reason, 2026-11-12).
# A trunk watch left by an earlier run on the same dist rewrites it beside
# this one's: the page then gets an index.html whose .js and .wasm the
# other build swapped out ("A file this page needs did not load",
# 2026-11-21). It is only ever this script's leftover, so stop it.
if leftover=$(pgrep -f "trunk watch .*--dist $dist\$"); then
    echo "stopping a trunk watch left on $dist: $leftover" >&2
    kill $leftover
    while pgrep -f "trunk watch .*--dist $dist\$" >/dev/null; do sleep 1; done
fi
rm -f "$dist/index.html"
(cd crates/helmsman-canvas && exec trunk watch --release --dist "$dist") &
watch=$!
# The watch goes when this shell runs the trap, so the server below runs as
# a child and is not exec'd: an exec'd shell runs no trap, and stopping the
# server alone (the kill by port above, dev-processes.sh --stop) left the
# watch running. The exec above makes $watch trunk itself, not a subshell
# that may fork it.
trap 'kill "$watch" 2>/dev/null; wait "$watch" 2>/dev/null || true' EXIT
trap 'exit 130' INT TERM
# The first build must land before the page is worth serving.
until [ -f "$dist/index.html" ]; do sleep 1; done
echo "serving $dist on http://0.0.0.0:$port/ (from the Air: http://10.0.0.41:$port/?playground)"
# No caching: http.server sends no Cache-Control, so a browser kept serving
# an old index.html (and with it the old build) after a rebuild.
/usr/bin/python3 -I -c '
import functools, http.server, sys
class NoStore(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()
port, root = int(sys.argv[1]), sys.argv[2]
handler = functools.partial(NoStore, directory=root)
http.server.ThreadingHTTPServer(("0.0.0.0", port), handler).serve_forever()
' "$port" "$dist"
