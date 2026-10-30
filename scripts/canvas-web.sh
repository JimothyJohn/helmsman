#!/usr/bin/env bash
# Build the node canvas for the browser into target/canvas-dist (index.html,
# .js loader, .wasm). Needs trunk (`cargo install trunk --locked`); adds the
# wasm32 target to the toolchain on first run.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v trunk >/dev/null || { echo "needs trunk: cargo install trunk --locked" >&2; exit 1; }
rustup target add wasm32-unknown-unknown >/dev/null
cd crates/helmsman-canvas
trunk build --release --dist ../../target/canvas-dist
