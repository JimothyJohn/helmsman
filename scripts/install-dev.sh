#!/usr/bin/env bash
set -euo pipefail
rustup component add rustfmt clippy
# scripts/lint.sh checks the canvas's wasm32 build.
rustup target add wasm32-unknown-unknown
cargo --version && cargo fmt --version && cargo clippy --version
