#!/usr/bin/env bash
# Build tcpethernet-plc (IEC 61131-3 runtime + CIP adapter) for a 64-bit ARM
# Pi on this machine and run it there as a systemd service with one program.
# Safe to re-run: rebuilds, reinstalls the binary, program and unit, restarts.
#
# Needs: Docker here (the build runs in the pinned rust:*-slim-trixie image as
# linux/arm64: native on the Studio, emulated on x86), key SSH and
# passwordless sudo on the Pi (scripts/pi-prep.sh), Debian Trixie on the Pi
# (glibc must match the build image). Runs here: `! scripts/pi-deploy-plc.sh nick@<pi-ip>`.
# Then smoke it from here: scripts/adapter-smoke.sh <pi-ip> (needs echo.st).
#
# Usage: scripts/pi-deploy-plc.sh user@host [PROGRAM.st|PROJECT.xml]
#        default program: crates/tcpethernet-plc/programs/echo.st
#        PLC_SCAN_MS=5     scan period in ms (default 10)
#        PLC_ALLOW=10.0.0.0/24   firewall source (default 192.168.3.0/24)
# Undo: sudo systemctl disable --now tcpethernet-plc, remove the unit,
#       /usr/local/bin/tcpethernet-plc and /etc/tcpethernet-plc; the firewall
#       rules are runtime-only and go with a reboot or perceptronics reload.
set -euo pipefail

target="${1:?usage: $0 user@host [PROGRAM.st|PROJECT.xml]}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
program="${2:-$repo/crates/tcpethernet-plc/programs/echo.st}"
scan_ms="${PLC_SCAN_MS:-10}"
allow="${PLC_ALLOW:-192.168.3.0/24}"
image="$(sed -n 's#^FROM \(rust:[^ ]*\) AS build$#\1#p' "$repo/docker/adapter/Dockerfile")"

[ -f "$program" ] || { echo "no program at $program" >&2; exit 2; }
case "$program" in
    *.st | *.ST) ext=st ;;
    *.xml | *.XML) ext=xml ;;
    *) echo "program must be .st or .xml: $program" >&2; exit 2 ;;
esac
[ -n "$image" ] || { echo "no rust build image in docker/adapter/Dockerfile" >&2; exit 1; }
toolchain="${image#rust:}"
toolchain="${toolchain%%-*}"
docker info >/dev/null 2>&1 || { echo "Docker is not running here" >&2; exit 1; }

ssh -o BatchMode=yes "$target" true || { echo "cannot ssh to $target with a key; run scripts/pi-prep.sh first" >&2; exit 1; }
ssh "$target" 'sudo -n true' || { echo "sudo on $target needs a password; see scripts/pi-prep.sh" >&2; exit 1; }
[ "$(ssh "$target" uname -m)" = aarch64 ] || { echo "$target is not a 64-bit ARM Linux" >&2; exit 1; }
ssh "$target" '. /etc/os-release; [ "$VERSION_CODENAME" = trixie ]' \
    || { echo "$target is not Debian Trixie; the binary is built against Trixie's glibc" >&2; exit 1; }

echo "==> build linux/arm64 in $image"
# Own target dir so the host's target/ (macOS objects) is never mixed in;
# registry in a named volume so rebuilds don't refetch. RUSTUP_TOOLCHAIN uses
# the image's toolchain; rust-toolchain.toml's "stable" would download another.
docker run --rm --platform linux/arm64 \
    -v "$repo:/src" -w /src \
    -v tcpethernet-cargo-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/src/target/pi \
    -e RUSTUP_TOOLCHAIN="$toolchain" \
    "$image" cargo build --release --locked -q -p tcpethernet-plc
bin="$repo/target/pi/release/tcpethernet-plc"
file "$bin" | grep -q 'ARM aarch64' || { echo "build did not produce an aarch64 binary: $(file "$bin")" >&2; exit 1; }

echo "==> copy"
ssh "$target" 'mkdir -p ~/.tcpethernet-plc'
scp -q "$bin" "$repo/deploy/tcpethernet-plc.service" "$target:.tcpethernet-plc/"
scp -q "$program" "$target:.tcpethernet-plc/program.$ext"

echo "==> install + restart"
# shellcheck disable=SC2029 # $ext and $scan_ms are meant to expand here.
ssh "$target" "set -e
cd ~/.tcpethernet-plc
sudo install -m 755 tcpethernet-plc /usr/local/bin/tcpethernet-plc
sudo install -d -m 755 /etc/tcpethernet-plc
sudo rm -f /etc/tcpethernet-plc/program.st /etc/tcpethernet-plc/program.xml
sudo install -m 644 program.$ext /etc/tcpethernet-plc/program.$ext
printf 'PLC_PROGRAM=/etc/tcpethernet-plc/program.%s\nPLC_SCAN_MS=%s\n' $ext $scan_ms \
    | sudo tee /etc/tcpethernet-plc/env >/dev/null
sudo install -m 644 tcpethernet-plc.service /etc/systemd/system/tcpethernet-plc.service
sudo systemctl daemon-reload
sudo systemctl enable tcpethernet-plc >/dev/null 2>&1
sudo systemctl restart tcpethernet-plc"

echo "==> firewall"
# A pick PC carries perceptronics' nftables table (input policy drop). Accept
# the PLC's ports from $PLC_ALLOW only, as runtime rules: they vanish on reboot
# or a perceptronics firewall reload, and a redeploy puts them back. No table,
# no change.
# shellcheck disable=SC2029 # $allow is meant to expand here.
ssh "$target" "set -e
if sudo nft list table inet perceptronics >/dev/null 2>&1; then
    if sudo nft list chain inet perceptronics input | grep -q 'comment \"tcpethernet-plc\"'; then
        echo 'rules already present'
    else
        sudo nft add rule inet perceptronics input ip saddr $allow tcp dport 44818 accept comment '\"tcpethernet-plc\"'
        sudo nft add rule inet perceptronics input ip saddr $allow udp dport '{ 44818, 2222 }' accept comment '\"tcpethernet-plc\"'
        echo 'opened 44818 tcp+udp, 2222 udp from $allow'
    fi
else
    echo 'no perceptronics table; nothing to do'
fi"

echo "==> verify"
up=
for _ in $(seq 1 20); do
    if ssh "$target" 'systemctl is-active --quiet tcpethernet-plc && ss -ltn | grep -q ":44818 "'; then
        up=1
        break
    fi
    sleep 1
done
ssh "$target" 'systemctl is-active tcpethernet-plc; journalctl -u tcpethernet-plc -n 5 --no-pager' || true
[ -n "$up" ] || { echo "FAIL: tcpethernet-plc is not listening on 44818 (journal above)" >&2; exit 1; }
host="${target#*@}"
echo
echo "PLC is up on $host running $(basename "$program") every ${scan_ms} ms"
echo "  from here:  scripts/adapter-smoke.sh $host"
