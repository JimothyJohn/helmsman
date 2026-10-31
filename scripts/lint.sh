#!/usr/bin/env bash
# The local gate's static half (tests are `cargo test --workspace --locked`).
# Beyond fmt + clippy it builds the two configurations CI does not:
# helmsman-plc without its default `box` feature (the runtime the canvas
# embeds) and the canvas for wasm32. Both broke dev on 2026-10-30 when PRs
# that were each green landed together. Needs: `scripts/install-dev.sh`.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check -q -p helmsman-plc --no-default-features --all-targets
cargo check -q --target wasm32-unknown-unknown -p helmsman-canvas
