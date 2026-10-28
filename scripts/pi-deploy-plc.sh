#!/usr/bin/env bash
# Build tcpethernet-plc on the Pi from this checkout and run it as a systemd
# service. Safe to re-run: it rebuilds, reinstalls, restarts, keeps state.
#
# Needs: key SSH and passwordless sudo on the Pi (scripts/pi-prep.sh), the Pi
# on the network. Installs rustup on the Pi on first run (official installer,
# minimal profile). Runs here: `! scripts/pi-deploy-plc.sh nick@<pi-ip>`.
#
# Usage: scripts/pi-deploy-plc.sh user@host
set -euo pipefail

target="${1:?usage: $0 user@host}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
host="${target#*@}"

ssh -o BatchMode=yes "$target" true || { echo "cannot ssh to $target with a key; run scripts/pi-prep.sh first" >&2; exit 1; }
ssh "$target" 'sudo -n true' || { echo "sudo on $target needs a password; see scripts/pi-prep.sh" >&2; exit 1; }
[ "$(ssh "$target" uname -m)" = aarch64 ] || { echo "$target is not a 64-bit ARM Linux" >&2; exit 1; }

echo "==> sync source"
ssh "$target" 'mkdir -p ~/tcpethernet'
rsync -a --delete --exclude target --exclude .git --exclude scratch --exclude .claude \
    "$repo/" "$target:tcpethernet/"

echo "==> toolchain"
ssh "$target" 'command -v cargo >/dev/null || [ -x ~/.cargo/bin/cargo ] || {
    sudo apt-get -y -qq install build-essential curl >/dev/null
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
}'

echo "==> build (first build takes a few minutes on a Pi 5)"
ssh "$target" 'cd ~/tcpethernet && ~/.cargo/bin/cargo build --release --locked -p tcpethernet-plc -p tcpethernet-cli 2>&1 | tail -n 3'

echo "==> install + restart"
ssh "$target" 'set -e
cd ~/tcpethernet
sudo install -m 755 target/release/tcpethernet-plc /usr/local/bin/tcpethernet-plc
sudo install -m 755 target/release/tcpethernet /usr/local/bin/tcpethernet
sudo install -m 644 deploy/tcpethernet-plc.service /etc/systemd/system/tcpethernet-plc.service
sudo systemctl daemon-reload
sudo systemctl enable --now tcpethernet-plc >/dev/null
sudo systemctl restart tcpethernet-plc'

echo "==> verify"
for _ in $(seq 1 20); do
    ssh "$target" 'systemctl is-active --quiet tcpethernet-plc && ss -ltn | grep -q ":8080 "' && break
    sleep 1
done
ssh "$target" 'systemctl is-active tcpethernet-plc; journalctl -u tcpethernet-plc -b -n 5 --no-pager'
token="$(ssh "$target" 'sudo cat /var/lib/private/tcpethernet-plc/token 2>/dev/null || sudo cat /var/lib/tcpethernet-plc/token')"
ip="$(ssh "$target" "hostname -I | awk '{print \$1}'")"
echo
echo "PLC is up on ${ip:-$host}"
echo "  browser:  http://${ip:-$host}:8080/#token=${token}"
echo "  from the Studio:  scripts/plc-smoke.sh ${ip:-$host}"
