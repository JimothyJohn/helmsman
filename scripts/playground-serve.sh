#!/usr/bin/env bash
# The ST playground on this machine with Claude behind a local proxy: the
# page holds no key; the proxy (helmsman-agent's playground-proxy example)
# adds x-api-key from ANTHROPIC_API_KEY to the page's requests and sends
# them to api.anthropic.com. Loopback only.
#
# Usage: ANTHROPIC_API_KEY=... scripts/playground-serve.sh [PORT]   # default 8791
# Open:  http://127.0.0.1:PORT/  (over SSH: ssh -L PORT:127.0.0.1:PORT <host>)
# Stop:  Ctrl-C, or kill $(lsof -tiTCP:PORT -sTCP:LISTEN)
# Needs: trunk (`cargo install trunk --locked`), ANTHROPIC_API_KEY.
set -euo pipefail
cd "$(dirname "$0")/.."
port="${1:-8791}"
: "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY: the proxy adds it to every request}"
out="target/playground-local"
./scripts/build-playground.sh "$out" / >/dev/null
cargo build -q --release -p helmsman-agent --example playground-proxy
exec target/release/examples/playground-proxy "$out" "$port"
