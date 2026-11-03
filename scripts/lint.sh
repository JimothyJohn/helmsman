#!/usr/bin/env bash
# The local gate's static half (tests are `cargo test --workspace --locked`).
# Beyond fmt + clippy it builds the two configurations CI does not:
# helmsman-plc without its default `box` feature (the runtime with nothing
# native) and the canvas for wasm32. Both broke dev on 2026-10-30 when PRs
# that were each green landed together. It also lints helmsman-plc without
# its `enip-io` feature (the EtherNet/IP scanner, in the default build since
# the E1213 bench passed) so that the enip-less box keeps compiling.
# Needs: `scripts/install-dev.sh`.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -q -p helmsman-plc --no-default-features --features box,https --all-targets -- -D warnings
cargo check -q -p helmsman-plc --no-default-features --all-targets
cargo check -q --target wasm32-unknown-unknown -p helmsman-canvas
