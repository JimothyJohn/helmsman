#!/usr/bin/env bash
# Build the edge node's firmware (crates/helmsman-node) for the ESP32-POE2.
#
# The node crate is not a workspace member: it needs the esp toolchain
# (`espup install`, rustup channel `esp`) and build-std for the Xtensa
# target, which the host gate must not pull in. This builds it from its own
# folder with its own Cargo.lock; nothing else in the workspace is touched.
#
# Usage: scripts/node-build.sh                (Phase 1: the `bench` binary)
#        scripts/node-build.sh net            (Phase 2: `bench` and `modbus`;
#                                              E1213_ADDR=a.b.c.d LOG_SINK=a.b.c.d:port
#                                              in the environment change the defaults)
#        scripts/node-build.sh clippy [net]   (the same, through clippy -D warnings)
set -euo pipefail
cd "$(dirname "$0")/../crates/helmsman-node"
if ! rustup toolchain list | grep -q '^esp'; then
  echo "needs the esp toolchain: cargo install espup --locked && espup install" >&2
  exit 1
fi
# The Xtensa GCC that links the image lives in the esp toolchain, off PATH;
# espup writes the PATH line to ~/export-esp.sh (supervictor's Quickstart
# sources the same file).
# shellcheck disable=SC1091
[ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
command -v xtensa-esp32-elf-gcc >/dev/null || { echo "xtensa-esp32-elf-gcc not on PATH: espup install writes ~/export-esp.sh" >&2; exit 1; }
mode="${1:-build}"
features=()
case "${2:-${1:-}}" in
  net) features=(--features net) ;;
esac
case "$mode" in
  build|net) cargo build --release ${features[@]+"${features[@]}"} ;;
  clippy) cargo clippy --release ${features[@]+"${features[@]}"} -- -D warnings ;;
  *) echo "usage: $0 [build|net|clippy [net]]" >&2; exit 2 ;;
esac
echo "built: crates/helmsman-node/target/xtensa-esp32-none-elf/release/{bench$( [ ${#features[@]} -gt 0 ] && echo ,modbus )}"
