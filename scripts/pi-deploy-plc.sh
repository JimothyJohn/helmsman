#!/usr/bin/env bash
# Build helmsman-plc (IEC 61131-3 runtime + CIP adapter) for a 64-bit ARM
# Pi on this machine and run it there as a systemd service with one program.
# Safe to re-run: rebuilds, reinstalls the binary, program and unit, restarts.
# Also retires the pre-rename tcpethernet-plc unit and its firewall rules.
#
# Needs: Docker here (the build runs in the pinned rust:*-slim-trixie image as
# linux/arm64: native on the Studio, emulated on x86), key SSH and
# passwordless sudo on the Pi (scripts/pi-prep.sh), Debian Trixie on the Pi
# (glibc must match the build image). Runs here: `! scripts/pi-deploy-plc.sh nick@<pi-ip>`.
# Then smoke it from here: scripts/adapter-smoke.sh <pi-ip> (needs echo.st).
#
# Usage: scripts/pi-deploy-plc.sh user@host [PROGRAM.st|PROJECT.xml]
#        default program: crates/helmsman-plc/programs/echo.st
#        PLC_SCAN_MS=5     scan period in ms (default 10)
#        PLC_ALLOW=10.0.0.0/24   firewall source (default 192.168.3.0/24)
# Undo: sudo systemctl disable --now helmsman-plc, remove the unit,
#       /usr/local/bin/helmsman-plc and /etc/helmsman-plc; the firewall
#       rules are runtime-only and go with a reboot or perceptronics reload.
set -euo pipefail

target="${1:?usage: $0 user@host [PROGRAM.st|PROJECT.xml]}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
program="${2:-$repo/crates/helmsman-plc/programs/echo.st}"
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
    -v helmsman-cargo-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/src/target/pi \
    -e RUSTUP_TOOLCHAIN="$toolchain" \
    "$image" cargo build --release --locked -q -p helmsman-plc
bin="$repo/target/pi/release/helmsman-plc"
file "$bin" | grep -q 'ARM aarch64' || { echo "build did not produce an aarch64 binary: $(file "$bin")" >&2; exit 1; }

echo "==> copy"
ssh "$target" 'mkdir -p ~/.helmsman-plc'
scp -q "$bin" "$repo/deploy/helmsman-plc.service" "$target:.helmsman-plc/"
scp -q "$program" "$target:.helmsman-plc/program.$ext"

echo "==> retire the pre-rename tcpethernet-plc, if present"
# Before the Helmsman rename the same runtime ran as tcpethernet-plc. Both
# bind 44818/2222, so the old unit must be gone before the new one starts.
ssh "$target" "set -e
if systemctl list-unit-files tcpethernet-plc.service >/dev/null 2>&1 \
    && systemctl list-unit-files tcpethernet-plc.service | grep -q tcpethernet-plc; then
    sudo systemctl disable --now tcpethernet-plc >/dev/null 2>&1 || true
    sudo rm -f /etc/systemd/system/tcpethernet-plc.service /usr/local/bin/tcpethernet-plc
    sudo rm -rf /etc/tcpethernet-plc
    rm -rf ~/.tcpethernet-plc
    sudo systemctl daemon-reload
    echo 'removed tcpethernet-plc'
else
    echo 'none'
fi
if sudo nft list table inet perceptronics >/dev/null 2>&1; then
    for h in \$(sudo nft -a list chain inet perceptronics input | sed -n 's/.*comment \"tcpethernet-plc\".*# handle \([0-9]*\)/\1/p'); do
        sudo nft delete rule inet perceptronics input handle \$h
    done
fi"

echo "==> install + restart"
# shellcheck disable=SC2029 # $ext and $scan_ms are meant to expand here.
ssh "$target" "set -e
cd ~/.helmsman-plc
sudo install -m 755 helmsman-plc /usr/local/bin/helmsman-plc
sudo install -d -m 755 /etc/helmsman-plc
sudo rm -f /etc/helmsman-plc/program.st /etc/helmsman-plc/program.xml
sudo install -m 644 program.$ext /etc/helmsman-plc/program.$ext
printf 'PLC_PROGRAM=/etc/helmsman-plc/program.%s\nPLC_SCAN_MS=%s\n' $ext $scan_ms \
    | sudo tee /etc/helmsman-plc/env >/dev/null
sudo install -m 644 helmsman-plc.service /etc/systemd/system/helmsman-plc.service
sudo systemctl daemon-reload
sudo systemctl enable helmsman-plc >/dev/null 2>&1
sudo systemctl restart helmsman-plc"

echo "==> firewall"
# A pick PC carries perceptronics' nftables table (input policy drop). Accept
# the PLC's ports from $PLC_ALLOW only, as runtime rules: they vanish on reboot
# or a perceptronics firewall reload, and a redeploy puts them back. No table,
# no change.
# shellcheck disable=SC2029 # $allow is meant to expand here.
ssh "$target" "set -e
if sudo nft list table inet perceptronics >/dev/null 2>&1; then
    if sudo nft list chain inet perceptronics input | grep -q 'comment \"helmsman-plc\"'; then
        echo 'rules already present'
    else
        sudo nft add rule inet perceptronics input ip saddr $allow tcp dport 44818 accept comment '\"helmsman-plc\"'
        sudo nft add rule inet perceptronics input ip saddr $allow udp dport '{ 44818, 2222 }' accept comment '\"helmsman-plc\"'
        echo 'opened 44818 tcp+udp, 2222 udp from $allow'
    fi
else
    echo 'no perceptronics table; nothing to do'
fi"

echo "==> verify"
up=
for _ in $(seq 1 20); do
    if ssh "$target" 'systemctl is-active --quiet helmsman-plc && ss -ltn | grep -q ":44818 "'; then
        up=1
        break
    fi
    sleep 1
done
ssh "$target" 'systemctl is-active helmsman-plc; journalctl -u helmsman-plc -n 5 --no-pager' || true
[ -n "$up" ] || { echo "FAIL: helmsman-plc is not listening on 44818 (journal above)" >&2; exit 1; }
# The box API on the Pi's loopback: proves StateDirectory= let it write its
# token under the unit's hardening. The token itself stays in the journal.
api=
for _ in $(seq 1 10); do
    if ssh "$target" 'curl -fsS --max-time 2 http://127.0.0.1:8080/api/modules >/dev/null'; then
        api=1
        break
    fi
    sleep 1
done
[ -n "$api" ] || { echo "FAIL: box API not answering on the Pi's 127.0.0.1:8080" >&2; exit 1; }
echo "box API answers on the Pi's 127.0.0.1:8080; token: ssh $target journalctl -u helmsman-plc | grep 'API token'"
host="${target#*@}"
echo
echo "PLC is up on $host running $(basename "$program") every ${scan_ms} ms"
echo "  from here:  scripts/adapter-smoke.sh $host"
