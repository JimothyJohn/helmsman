#!/usr/bin/env bash
# Runs the interop suite against a live target. Default target is the
# OpENer container published on localhost by docker-compose.yml.
set -euo pipefail
cd "$(dirname "$0")/.."
export TCPETHERNET_OPENER_ADDR="${TCPETHERNET_OPENER_ADDR:-127.0.0.1}"
cargo test --workspace --locked --test opener_interop -- --nocapture
