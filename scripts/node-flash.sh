#!/usr/bin/env bash
# Flash the edge node's bench firmware onto the ESP32-POE2 over USB and
# watch its serial output. Nick's step, run in his own shell: the board has
# NO galvanic isolation, so this never runs while PoE is connected, and an
# agent session never runs it at all.
#
#   ⚠ UNPLUG ETHERNET BEFORE USB. Flash over USB, read the numbers, then
#     unplug USB before any Ethernet cable goes back in. Never both.
#
# Does: builds the release image (scripts/node-build.sh), finds the one
#       USB serial port (or takes SERIAL_PORT), confirms the rule with you,
#       flashes with espflash and stays in the monitor; everything the
#       board prints is also written to scratch/node-bench-<utc>.log for
#       docs/EDGE.md. Ctrl-C leaves the monitor.
# Needs: the esp toolchain, espflash (`cargo install espflash --locked`),
#        the POE2 on USB with Ethernet unplugged.
# Idempotent: flashing the same image again changes nothing but the log.
#
# Usage: scripts/node-flash.sh            (asks before flashing)
#        SERIAL_PORT=/dev/cu.usbserial-XXXX scripts/node-flash.sh
set -euo pipefail
cd "$(dirname "$0")/.."
command -v espflash >/dev/null || { echo "espflash missing: cargo install espflash --locked" >&2; exit 1; }

if [ -z "${SERIAL_PORT:-}" ]; then
  ports=()
  for p in /dev/cu.usbserial-* /dev/cu.usbmodem* /dev/cu.SLAB_USBtoUART* /dev/ttyUSB* /dev/ttyACM*; do
    [ -e "$p" ] && ports+=("$p")
  done
  case "${#ports[@]}" in
    0) echo "no USB serial port: is the POE2 on USB?" >&2; exit 1 ;;
    1) SERIAL_PORT="${ports[0]}" ;;
    *) echo "several serial ports (${ports[*]}): set SERIAL_PORT=" >&2; exit 1 ;;
  esac
fi

echo "ESP32-POE2 on $SERIAL_PORT"
echo
echo "  ⚠  The POE2 has no isolation: USB and PoE together can damage the board,"
echo "     the switch port or this computer. Confirm the Ethernet cable is OUT."
echo
read -r -p "Ethernet unplugged, flash now? [y/N] " yes
[ "$yes" = "y" ] || [ "$yes" = "Y" ] || { echo "not flashing"; exit 1; }

scripts/node-build.sh
mkdir -p scratch
log="scratch/node-bench-$(date -u +%Y%m%dT%H%M%SZ).log"
echo "monitor output also goes to $log; Ctrl-C to leave the monitor"
cd crates/helmsman-node
espflash flash --monitor --port "$SERIAL_PORT" \
  target/xtensa-esp32-none-elf/release/helmsman-node 2>&1 | tee "../../$log"
echo "log: $log — paste the '--- round' lines into docs/EDGE.md (Phase 1, numbers)"
