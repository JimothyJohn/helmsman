#!/usr/bin/env bash
# The local gate's static half (tests are `cargo test --workspace --locked`).
# Beyond fmt + clippy it builds the two configurations CI does not:
# helmsman-plc without its default `box` feature (the runtime with nothing
# native) and the canvas for wasm32. Both broke dev on 2026-10-30 when PRs
# that were each green landed together. It also lints helmsman-plc without
# its `enip-io` feature (the EtherNet/IP scanner, in the default build since
# the E1213 bench passed) so that the enip-less box keeps compiling, and
# helmsman-iec, helmsman-modbus, helmsman-profinet, helmsman-iolink,
# helmsman-proto's codecs and helmsman-nodecore without std for thumbv7em (the edge node, docs/EDGE.md),
# and the two node firmwares themselves (crates/helmsman-node-p4, crates/helmsman-node).
# Needs: `scripts/install-dev.sh`.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -q -p helmsman-plc --no-default-features --features box,https --all-targets -- -D warnings
cargo check -q -p helmsman-plc --no-default-features --all-targets
# The canvas for the browser: clippy, not check, so a warning that only
# shows on wasm32 (the web.rs glue is wasm-only) fails here (2026-11-08).
cargo clippy -q --target wasm32-unknown-unknown -p helmsman-canvas -- -D warnings
# helmsman-drawing (the DXF reader) runs in the browser too: clippy for wasm32.
cargo clippy -q --target wasm32-unknown-unknown -p helmsman-drawing -- -D warnings
# helmsman-iec without std (the edge node's build, docs/EDGE.md Phase 0):
# clippy, so a std-only call fails here and not on the Xtensa toolchain.
cargo clippy -q -p helmsman-iec --no-default-features --features libm --target thumbv7em-none-eabihf -- -D warnings
cargo clippy -q -p helmsman-modbus --target thumbv7em-none-eabihf -- -D warnings
cargo clippy -q -p helmsman-profinet --target thumbv7em-none-eabihf -- -D warnings
cargo clippy -q -p helmsman-iolink --target thumbv7em-none-eabihf -- -D warnings
# helmsman-l2's other half: on a Mac the packet-socket code is not compiled
# and on CI's Linux the BPF code is not, so each host lints the half it
# does not build (2026-11-11: three casts in the Linux half failed CI
# after a green local gate).
case "$(uname -s)" in
  Darwin) cargo clippy -q -p helmsman-l2 --target aarch64-unknown-linux-gnu -- -D warnings ;;
  *) ;;
esac
# The Windows half (code behind `cfg(not(unix))` / `cfg(windows)`) is not
# linted here: ring and rusqlite build C, which needs MSVC's cl on the
# target, so `cargo clippy --target x86_64-pc-windows-msvc` stops in their
# build scripts on any Mac or Linux. CI's `box (windows-latest)` job is its
# lint (2026-11-23).
cargo clippy -q -p helmsman-proto --no-default-features --target thumbv7em-none-eabihf -- -D warnings
cargo clippy -q -p helmsman-nodecore --target thumbv7em-none-eabihf -- -D warnings
# The node firmwares are not workspace members (each has its own target
# and lock) and no CI job compiles them, so nothing else notices when the
# crates they share with the workspace change under them: on 2026-11-24
# crates/helmsman-node stopped building when helmsman-nodecore's map types
# changed, and only a build by hand found it. The P4's is plain RISC-V on
# stable; the ESP32's needs the esp toolchain (`espup install`) and is
# skipped, out loud, where that is not installed.
NODE_BOARD=p4-eth scripts/node-build.sh clippy
if rustup toolchain list | grep -q '^esp'; then
  scripts/node-build.sh clippy
else
  echo "lint: no esp toolchain here; crates/helmsman-node (the ESP32 node) was NOT linted" >&2
fi
