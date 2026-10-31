#!/usr/bin/env bash
# Differential test of the Structured Text interpreter against ruSTy
# (crates/helmsman-iec/tests/oracle.rs). Builds the pinned ruSTy image
# from docker/rusty/ if needed. Needs Docker.
#
# Widen a sweep: HELMSMAN_RUSTY_CASES=3000 HELMSMAN_RUSTY_SEED=7 scripts/test-oracle.sh
set -euo pipefail
cd "$(dirname "$0")/.."
image=helmsman/rusty:1.0.7
docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" docker/rusty
export HELMSMAN_RUSTY="${HELMSMAN_RUSTY:-docker run --rm -i $image}"
cargo test --locked -p helmsman-iec --test oracle -- --nocapture
