#!/usr/bin/env bash
# The local gate's static half (tests are `cargo test --workspace --locked`).
# Beyond fmt + clippy it builds the two configurations CI does not:
# helmsman-plc without its default `box` feature (the runtime with nothing
# native) and the canvas for wasm32. Both broke dev on 2026-10-30 when PRs
# that were each green landed together. It also lints helmsman-plc without
# its `enip-io` feature (the EtherNet/IP scanner, in the default build since
# the E1213 bench passed) so that the enip-less box keeps compiling, and
# helmsman-iec without std for thumbv7em (the edge node, docs/EDGE.md).
# Needs: `scripts/install-dev.sh`.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -q -p helmsman-plc --no-default-features --features box,https --all-targets -- -D warnings
cargo check -q -p helmsman-plc --no-default-features --all-targets
cargo check -q --target wasm32-unknown-unknown -p helmsman-canvas
# helmsman-iec without std (the edge node's build, docs/EDGE.md Phase 0):
# clippy, so a std-only call fails here and not on the Xtensa toolchain.
cargo clippy -q -p helmsman-iec --no-default-features --features libm --target thumbv7em-none-eabihf -- -D warnings
