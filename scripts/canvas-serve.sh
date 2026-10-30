#!/usr/bin/env bash
# Serve the node canvas with live rebuild on every edit. Listens on all
# interfaces so another machine on the LAN can open it.
#
# Usage: scripts/canvas-serve.sh [PORT]      # default 8790
# Stop:  Ctrl-C, or kill $(lsof -tiTCP:PORT -sTCP:LISTEN)
set -euo pipefail
cd "$(dirname "$0")/.."
port="${1:-8790}"
command -v trunk >/dev/null || { echo "needs trunk: cargo install trunk --locked" >&2; exit 1; }
rustup target add wasm32-unknown-unknown >/dev/null
cd crates/tcpethernet-canvas
exec trunk serve --release --address 0.0.0.0 --port "$port" --dist ../../target/canvas-dist
