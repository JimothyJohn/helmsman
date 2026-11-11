#!/usr/bin/env bash
set -euo pipefail
rustup component add rustfmt clippy
# scripts/lint.sh checks the canvas's wasm32 build and helmsman-iec's no_std
# build (a cheap upstream target that proves no_std without the Xtensa toolchain).
rustup target add wasm32-unknown-unknown thumbv7em-none-eabihf
# helmsman-l2's Linux half, linted from a Mac (scripts/lint.sh).
if [ "$(uname -s)" = Darwin ]; then rustup target add aarch64-unknown-linux-gnu; fi
cargo --version && cargo fmt --version && cargo clippy --version
