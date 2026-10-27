#!/usr/bin/env bash
set -euo pipefail
rustup component add rustfmt clippy
cargo --version && cargo fmt --version && cargo clippy --version
