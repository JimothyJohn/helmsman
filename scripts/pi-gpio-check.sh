#!/usr/bin/env bash
# Check the box's own channels on a Pi: the GPIO character device (uAPI v2)
# and /dev/i2c-N, through helmsman-io's board backend (the code behind
# `helmsman-plc --gpio-chip`). Builds the board_check example for linux/arm64
# here, copies it to the Pi, finds the 40-pin header's GPIO chip, lists the
# I2C bus, reads the channels you name, and proves a loop if you give one.
# Safe to re-run; changes nothing on the Pi but ~/.helmsman-plc/board_check.
#
# Needs: Docker here (same pinned rust image as scripts/pi-deploy-plc.sh),
# key SSH to the Pi (scripts/pi-prep.sh), on the Pi the `gpiod` and
# `i2c-tools` packages (`sudo apt install gpiod i2c-tools`), I2C enabled
# (`dtparam=i2c_arm=on` in /boot/firmware/config.txt, then reboot), and the
# SSH user in the `gpio` and `i2c` groups. Runs here, from the repo:
#   ! scripts/pi-gpio-check.sh nick@<pi-ip>
#
# Environment (all optional):
#   GPIO_CHIP=/dev/gpiochip0   skip detection and use this chip
#   READ="di:4 di:5 i2c:1:0x48:0:i16"   channels to read (default: di:4)
#   LOOP="do:17=di:27"   DRIVES do:17 on and off and expects di:27 to follow.
#                        Only with a jumper (or the bench's 24 V stage) from
#                        that output to that input and nothing else on it.
# A running helmsman-plc holding the same lines makes their reads fail
# (busy); stop it first if it uses them: sudo systemctl stop helmsman-plc.
set -euo pipefail

target="${1:?usage: $0 user@host}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
image="$(sed -n 's#^FROM \(rust:[^ ]*\) AS build$#\1#p' "$repo/docker/adapter/Dockerfile")"
reads="${READ:-di:4}"
loop="${LOOP:-}"

[ -n "$image" ] || { echo "no rust build image in docker/adapter/Dockerfile" >&2; exit 1; }
toolchain="${image#rust:}"
toolchain="${toolchain%%-*}"
docker info >/dev/null 2>&1 || { echo "Docker is not running here" >&2; exit 1; }
ssh -o BatchMode=yes "$target" true || { echo "cannot ssh to $target with a key; run scripts/pi-prep.sh first" >&2; exit 1; }
[ "$(ssh "$target" uname -m)" = aarch64 ] || { echo "$target is not a 64-bit ARM Linux" >&2; exit 1; }

echo "==> kernel and tools on $target"
ssh "$target" 'set -e
uname -r
major=$(uname -r | cut -d. -f1); minor=$(uname -r | cut -d. -f2)
if [ "$major" -lt 5 ] || { [ "$major" -eq 5 ] && [ "$minor" -lt 10 ]; }; then
    echo "kernel older than 5.10: no GPIO uAPI v2" >&2; exit 1
fi
for t in gpiodetect i2cdetect; do
    command -v "$t" >/dev/null || { echo "missing $t: sudo apt install gpiod i2c-tools" >&2; exit 1; }
done
id -nG | tr " " "\n" | grep -qx gpio || echo "warning: $(id -un) is not in the gpio group"
id -nG | tr " " "\n" | grep -qx i2c || echo "warning: $(id -un) is not in the i2c group"'

echo "==> GPIO chips"
ssh "$target" gpiodetect
chip="${GPIO_CHIP:-}"
if [ -z "$chip" ]; then
    # The header's chip: pinctrl-rp1 on a Pi 5 (gpiochip4 before kernel
    # 6.6.45, gpiochip0 after), pinctrl-bcm2711 / -bcm2835 on a Pi 4 / 3.
    name="$(ssh "$target" gpiodetect | awk '/pinctrl-(rp1|bcm2711|bcm2835)/ { print $1; exit }')"
    [ -n "$name" ] || { echo "no header GPIO chip found; set GPIO_CHIP" >&2; exit 1; }
    chip="/dev/$name"
fi
echo "using $chip"

echo "==> I2C buses"
ssh "$target" 'ls /dev/i2c-* 2>/dev/null || echo "no /dev/i2c-*: enable dtparam=i2c_arm=on"'
if ssh "$target" test -e /dev/i2c-1; then
    ssh "$target" i2cdetect -y 1
fi

echo "==> build board_check for linux/arm64 in $image"
docker run --rm --platform linux/arm64 \
    -v "$repo:/src" -w /src \
    -v helmsman-cargo-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/src/target/pi \
    -e RUSTUP_TOOLCHAIN="$toolchain" \
    "$image" cargo build --release --locked -q -p helmsman-io --features gpio --example board_check
bin="$repo/target/pi/release/examples/board_check"
file "$bin" | grep -q 'ARM aarch64' || { echo "build did not produce an aarch64 binary: $(file "$bin")" >&2; exit 1; }
ssh "$target" 'mkdir -p ~/.helmsman-plc'
scp -q "$bin" "$target:.helmsman-plc/board_check"

echo "==> board_check"
args=(--chip "$chip")
for r in $reads; do args+=(--read "$r"); done
if [ -n "$loop" ]; then args+=(--loop "$loop"); fi
# shellcheck disable=SC2029 # the arguments are meant to expand here.
ssh "$target" "./.helmsman-plc/board_check ${args[*]}"
echo "==> ok: run helmsman-plc with --gpio-chip $chip"
