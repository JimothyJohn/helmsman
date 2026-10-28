#!/usr/bin/env bash
# Differential test of the Structured Text interpreter against ruSTy
# (crates/tcpethernet-iec/tests/oracle.rs). Builds the pinned ruSTy image
# from docker/rusty/ if needed. Needs Docker.
#
# Widen a sweep: TCPETHERNET_RUSTY_CASES=3000 TCPETHERNET_RUSTY_SEED=7 scripts/test-oracle.sh
set -euo pipefail
cd "$(dirname "$0")/.."
image=tcpethernet/rusty:1.0.7
docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" docker/rusty
export TCPETHERNET_RUSTY="${TCPETHERNET_RUSTY:-docker run --rm -i $image}"
cargo test --locked -p tcpethernet-iec --test oracle -- --nocapture
