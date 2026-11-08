#!/usr/bin/env bash
# Build the edge node's firmware: crates/helmsman-node for the ESP32-POE2 /
# POE-ISO (default), or crates/helmsman-node-p4 for the Waveshare
# ESP32-P4-ETH with NODE_BOARD=p4-eth.
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
#        NODE_BOARD=p4-eth scripts/node-build.sh [build|clippy]   (the P4: `modbus` and `enip`;
#                                              E1213_ADDR, LOG_SINK and ENIP_INPUT_ONLY=1 apply)
set -euo pipefail
cd "$(dirname "$0")/.."
# NODE_BOARD=p4-eth: the Waveshare ESP32-P4-ETH (crates/helmsman-node-p4),
# RISC-V on upstream stable, no esp toolchain, no build-std; both binaries
# (`modbus`, `enip`) build in one go. Everything below this block is the
# Xtensa POE2 / POE-ISO path (crates/helmsman-node).
if [ "${NODE_BOARD:-poe2}" = p4-eth ]; then
  cd crates/helmsman-node-p4
  case "${1:-build}" in
    build|net) cargo build --release ;;
    clippy) cargo clippy --release -- -D warnings; exit 0 ;;
    *) echo "usage: NODE_BOARD=p4-eth $0 [build|clippy]" >&2; exit 2 ;;
  esac
  # The main stack is what the statics leave of SRAM; refuse an image that
  # leaves less than MIN_STACK (the POE2 died on 1.7 KiB, 2026-11-03).
  MIN_STACK=$((32 * 1024))
  NM=""
  for c in llvm-nm /opt/homebrew/opt/llvm/bin/llvm-nm; do
    command -v "$c" >/dev/null 2>&1 && { NM="$c"; break; }
  done
  if [ -z "$NM" ]; then
    echo "note: no llvm-nm on PATH (brew install llvm); skipping the stack check"
  else
    for b in modbus enip; do
      elf="target/riscv32imafc-unknown-none-elf/release/$b"
      start=$("$NM" "$elf" | awk '$3 == "_stack_start" { print $1 }')
      end=$("$NM" "$elf" | awk '$3 == "_stack_end" { print $1 }')
      [ -n "$start" ] && [ -n "$end" ] || { echo "$b: no stack symbols in $elf" >&2; exit 1; }
      stack=$(( 0x$start - 0x$end ))
      echo "$b: main stack $stack bytes"
      if [ "$stack" -lt "$MIN_STACK" ]; then
        echo "$b: the main stack is under $MIN_STACK bytes; shrink HEAP_SIZE in src/bin/$b.rs" >&2
        exit 1
      fi
    done
  fi
  exit 0
fi
cd crates/helmsman-node
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
# NODE_BOARD=poe-iso builds for the ESP32-POE-ISO (RMII clock on GPIO17);
# the default is the ESP32-POE2.
# The POE-ISO is a WROOM: no PSRAM, so the default `psram` feature is off
# (with it on, esp_hal::init brings up PSRAM over GPIO16/17, and GPIO17 is
# that board's RMII clock pin; docs/EDGE.md, 2026-11-08).
case "${NODE_BOARD:-poe2}" in
  poe2) ;;
  poe-iso)
    if [ ${#features[@]} -gt 0 ]; then
      features=(--no-default-features --features poe-iso)
    else
      features=(--no-default-features)
    fi
    ;;
  *) echo "NODE_BOARD: poe2, poe-iso or p4-eth, not ${NODE_BOARD}" >&2; exit 2 ;;
esac
case "$mode" in
  build|net) cargo build --release ${features[@]+"${features[@]}"} ;;
  clippy) cargo clippy --release ${features[@]+"${features[@]}"} -- -D warnings ;;
  *) echo "usage: $0 [build|net|clippy [net]]" >&2; exit 2 ;;
esac
# The main stack is what the statics leave of the DRAM segment
# (_stack_start - _stack_end); a heap that grows eats it silently, and the
# node then dies on its stack guard (2026-11-03: 1.7 KiB left). Refuse an
# image with less than MIN_STACK.
MIN_STACK=$((32 * 1024))
if [ "$mode" != clippy ]; then
  bins=(bench)
  [ ${#features[@]} -gt 0 ] && bins+=(modbus)
  for b in "${bins[@]}"; do
    elf="target/xtensa-esp32-none-elf/release/$b"
    start=$(xtensa-esp32-elf-nm "$elf" | awk '$3 == "_stack_start_cpu0" { print $1 }')
    end=$(xtensa-esp32-elf-nm "$elf" | awk '$3 == "_stack_end_cpu0" { print $1 }')
    [ -n "$start" ] && [ -n "$end" ] || { echo "$b: no stack symbols in $elf" >&2; exit 1; }
    stack=$(( 0x$start - 0x$end ))
    echo "$b: main stack $stack bytes"
    if [ "$stack" -lt "$MIN_STACK" ]; then
      echo "$b: the main stack is under $MIN_STACK bytes; shrink HEAP_SIZE in src/bin/$b.rs" >&2
      exit 1
    fi
  done
fi
echo "built: crates/helmsman-node/target/xtensa-esp32-none-elf/release/{bench$( [ ${#features[@]} -gt 0 ] && echo ,modbus )}"
