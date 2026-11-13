#!/usr/bin/env bash
# Flash the edge node's firmware onto the ESP32-POE2 (or, with
# NODE_BOARD=p4-eth, the Waveshare ESP32-P4-ETH; NODE_BOARD=poe-iso, the
# ESP32-POE-ISO) over USB and watch its serial output. The POE2 and the P4
# board are Nick's step, run in his own shell: no galvanic isolation, so
# this never runs while PoE is connected. The POE-ISO is isolated and on
# USB for good, so it flashes without a prompt, from an agent session too.
#
#   ⚠ UNPLUG ETHERNET BEFORE USB. Flash over USB, then unplug USB before any
#     Ethernet cable goes back in. Never both.
#
# Phase 1 (default, `bench`): stay on USB and read the numbers in the monitor.
# Phase 2 (`modbus`): the node needs Ethernet, so the order is
#   1. flash here over USB (Ethernet unplugged), Ctrl-C the monitor;
#   2. unplug USB;
#   3. plug the PoE cable;
#   4. read the node's report lines on the Studio:
#        /usr/bin/python3 -I scripts/node-log-sink.py
#      (start it before step 3; it listens on udp/9514, the node's LOG_SINK).
#   The E1213's comms-loss safe state must be set on its console first; then
#   G5 is pulling the node's cable and watching the outputs go safe.
#
# Does: builds the release image (scripts/node-build.sh), finds the one USB
#       serial port (or takes SERIAL_PORT), confirms the rule with you,
#       flashes with espflash and stays in the monitor; everything the board
#       prints is also written to scratch/node-<bin>-<utc>.log for docs/EDGE.md.
# Needs: the esp toolchain, espflash (`cargo install espflash --locked`),
#        the POE2 on USB with Ethernet unplugged.
# Idempotent: flashing the same image again changes nothing but the log.
#
# Usage: scripts/node-flash.sh [bench|modbus]
#        SERIAL_PORT=/dev/cu.usbserial-XXXX scripts/node-flash.sh modbus
#        NODE_BOARD=poe-iso scripts/node-flash.sh modbus
#        NODE_BOARD=p4-eth scripts/node-flash.sh [node|enip|modbus|profinet]
#        (profinet: PN_STATION, PN_SLOTS, PN_IP, PN_CYCLE_MS at build time; crates/helmsman-node-p4/src/bin/profinet.rs)
#        NODE_BOARD=p4-eth NODE_P4_NO_POE=1 ESPFLASH=/path/to/espflash-4 scripts/node-flash.sh node
#
# The port and the board's MAC are remembered per board after a flash that
# completes (scratch/node-board.<board> in the main checkout, untracked,
# also when run from a worktree): with several boards on
# USB the next run takes the remembered port without SERIAL_PORT, reads that
# one board's MAC first (espflash board-info: it resets only the board about
# to be flashed) and refuses if it is not the board remembered under that
# name, so a POE2 is never flashed as a POE-ISO by a moved cable.
set -euo pipefail
cd "$(dirname "$0")/.."
# ESPFLASH=/path/to/espflash picks another build of the tool (the P4 needs
# 4.x; see below).
ESPFLASH="${ESPFLASH:-espflash}"
command -v "$ESPFLASH" >/dev/null || { echo "espflash missing: cargo install espflash --locked" >&2; exit 1; }
board="${NODE_BOARD:-poe2}"
if [ "$board" = p4-eth ]; then
  bin="${1:-node}"
  case "$bin" in
    modbus|enip|node|profinet) ;;
    *) echo "usage: NODE_BOARD=p4-eth $0 [node|enip|modbus|profinet]" >&2; exit 2 ;;
  esac
  # A P4 at chip revision v3.x does not answer espflash 3.3 ("Timeout
  # while running ReadReg command", 2026-11-10); 4.7 flashes it.
  case "$("$ESPFLASH" --version | awk '{ print $2 }')" in
    [0-3].*) echo "the P4 needs espflash 4 or newer ($ESPFLASH is $("$ESPFLASH" --version)): cargo install espflash --locked, or ESPFLASH=/path/to/espflash" >&2; exit 1 ;;
  esac
  build_mode=""
  crate=crates/helmsman-node-p4
  target_dir=target/riscv32imafc-unknown-none-elf/release
else
  bin="${1:-bench}"
  case "$bin" in
    bench) build_mode="" ;;
    modbus) build_mode="net" ;;
    node) build_mode="api" ;;
    *) echo "usage: $0 [bench|modbus|node]" >&2; exit 2 ;;
  esac
  crate=crates/helmsman-node
  target_dir=target/xtensa-esp32-none-elf/release
fi

root="$PWD"
# The memo lives in the main checkout's scratch/ whichever worktree this
# runs from: a flash from a worktree found no memo and stopped at "several
# serial ports" (2026-11-11).
main="$(git rev-parse --path-format=absolute --git-common-dir)"
main="${main%/.git}"
memo="$main/scratch/node-board.$board"
want_mac=""
memo_port=""
if [ -f "$memo" ]; then
  read -r memo_port want_mac < "$memo" || true
fi
if [ -z "${SERIAL_PORT:-}" ] && [ -n "$memo_port" ] && [ -e "$memo_port" ]; then
  SERIAL_PORT="$memo_port"
  echo "$board: the remembered port $SERIAL_PORT (MAC $want_mac)"
fi
if [ -z "${SERIAL_PORT:-}" ]; then
  ports=()
  for p in /dev/cu.usbserial-* /dev/cu.usbmodem* /dev/cu.SLAB_USBtoUART* /dev/ttyUSB* /dev/ttyACM*; do
    [ -e "$p" ] && ports+=("$p")
  done
  case "${#ports[@]}" in
    0) echo "no USB serial port: is the POE2 on USB?" >&2; exit 1 ;;
    1) SERIAL_PORT="${ports[0]}" ;;
    *) echo "several serial ports (${ports[*]}): set SERIAL_PORT= once; it is remembered for $board" >&2; exit 1 ;;
  esac
fi

mac="$("$ESPFLASH" board-info --port "$SERIAL_PORT" 2>/dev/null | awk '/MAC address/ { print $3 }')"
if [ -z "$mac" ]; then
  echo "no ESP32 answers on $SERIAL_PORT (espflash board-info)" >&2
  exit 1
fi
if [ -n "$want_mac" ] && [ "$mac" != "$want_mac" ]; then
  echo "FAIL: $SERIAL_PORT has MAC $mac, but $board was $want_mac: another board is on that port." >&2
  echo "      Set SERIAL_PORT= for $board (and NODE_BOARD= for the other board) to flash it." >&2
  exit 1
fi
echo "board on $SERIAL_PORT: MAC $mac"
case "$board" in
  p4-eth)
    echo "Waveshare ESP32-P4-ETH on $SERIAL_PORT, image: $bin"
    if [ "${NODE_P4_NO_POE:-}" = 1 ]; then
      # Without the PoE module on its header the board takes no power from
      # the cable, so USB and Ethernet together are fine (the bench since
      # 2026-11-10) and an agent session flashes it.
      yes=y
    else
      echo
      echo "  ⚠  Whether this board's PoE module is isolated is not documented. Until"
      echo "     it is measured, treat it like the POE2: confirm the Ethernet cable is OUT."
      echo "     (NODE_P4_NO_POE=1 skips this when the PoE module is not fitted.)"
      echo
      read -r -p "Ethernet unplugged, flash now? [y/N] " yes
    fi
    ;;
  poe-iso)
    # Isolated, on USB for good, identified by MAC above: no prompt, no
    # lecture (Nick, 2026-11-08: the warning on every flash wasted time).
    # An agent session flashes this board itself.
    echo "ESP32-POE-ISO on $SERIAL_PORT, image: $bin"
    yes=y
    ;;
  *)
    echo "ESP32-POE2 on $SERIAL_PORT, image: $bin"
    echo
    echo "  ⚠  The POE2 has no isolation: USB and PoE together can damage the board,"
    echo "     the switch port or this computer. Confirm the Ethernet cable is OUT."
    echo
    read -r -p "Ethernet unplugged, flash now? [y/N] " yes
    ;;
esac
[ "$yes" = "y" ] || [ "$yes" = "Y" ] || { echo "not flashing"; exit 1; }

# shellcheck disable=SC2086
scripts/node-build.sh $build_mode
mkdir -p scratch
log="scratch/node-$bin-$(date -u +%Y%m%dT%H%M%SZ).log"
echo "monitor output also goes to $log; Ctrl-C to leave the monitor"
cd "$crate"
# The interactive monitor needs a terminal; from an agent session (no tty)
# flash alone, and read the console afterwards with
#   espflash monitor --port PORT --non-interactive --before no-reset-no-sync --after no-reset
# (with `--chip esp32p4` on the P4: without a reset espflash cannot ask
# the chip what it is, and says "Chip argument not provided", 2026-11-11).
if [ -t 0 ]; then
  "$ESPFLASH" flash --monitor --port "$SERIAL_PORT" \
    "$target_dir/$bin" 2>&1 | tee "../../$log"
else
  "$ESPFLASH" flash --port "$SERIAL_PORT" \
    "$target_dir/$bin" 2>&1 | tee "../../$log"
  echo "no terminal: flashed without the monitor; read the console with"
  chip=""
  [ "$board" = p4-eth ] && chip=" --chip esp32p4"
  echo "  espflash monitor$chip --port $SERIAL_PORT --non-interactive --before no-reset-no-sync --after no-reset"
fi
echo "log: $log"
if grep -q "Flashing has completed" "$root/$log"; then
  mkdir -p "$(dirname "$memo")"
  printf '%s %s\n' "$SERIAL_PORT" "$mac" > "$memo"
  echo "remembered: $board on $SERIAL_PORT, MAC $mac ($memo)"
fi
if [ "$board" = p4-eth ] && [ "$bin" = node ]; then
  echo "now: first boot answers at http://192.168.127.250:8080/ (the factory record) unless flash"
  echo "     already holds settings; set name, address and token there."
elif [ "$board" = p4-eth ] && [ "$bin" = profinet ]; then
  echo "now: the node looks for the PROFINET station it was built for and reports every 10 s"
  echo "     on serial and to LOG_SINK (/usr/bin/python3 -I scripts/node-log-sink.py)."
elif [ "$board" = p4-eth ]; then
  echo "now: unplug USB, start the sink (/usr/bin/python3 -I scripts/node-log-sink.py), plug PoE."
  echo "     The enip image opens an Exclusive Owner connection to the E1213: stop the try box's"
  echo "     enip-io first, or build with ENIP_INPUT_ONLY=1 to run beside it."
elif [ "$bin" = modbus ] && [ "$board" = poe-iso ]; then
  echo "now: start the sink (/usr/bin/python3 -I scripts/node-log-sink.py), plug PoE (USB may stay),"
  echo "     and rerun: espflash monitor --port $SERIAL_PORT to watch the console."
elif [ "$bin" = node ]; then
  echo "now: plug the cable (USB may stay on an ISO). First boot answers at http://192.168.127.250:8080/"
  echo "     (the factory record) unless flash already holds settings; set name, address and token there."
elif [ "$bin" = modbus ]; then
  echo "now: unplug USB, start the sink (/usr/bin/python3 -I scripts/node-log-sink.py), plug PoE."
else
  echo "paste the '--- round' lines into docs/EDGE.md (Phase 1, numbers)"
fi
