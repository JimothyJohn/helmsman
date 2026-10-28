#!/usr/bin/env bash
# End-to-end check of a tcpethernet-plc (or any OpENer-sample-compatible
# adapter) from this machine: discover it, read its identity, open a Class 1
# exclusive-owner connection and confirm the output comes back as input
# (the PLC's default program echoes I to Q).
#
# Needs: the release CLI (`cargo build --release -p tcpethernet-cli`), UDP
# 2222 free here and reachable from the target (macOS may ask to allow
# incoming connections for the binary the first time).
#
# Usage: scripts/plc-smoke.sh HOST
set -euo pipefail
host="${1:?usage: $0 HOST}"
cd "$(dirname "$0")/.."
bin=target/release/tcpethernet
[ -x "$bin" ] || cargo build --release -q -p tcpethernet-cli

echo "==> discover"
"$bin" discover "$host" --window 1500 | tee /dev/stderr | grep -q . || { echo "no ListIdentity reply from $host" >&2; exit 1; }
echo "==> identity"
"$bin" identity "$host"
pattern=$(printf '%02x' $(seq 1 32))
echo "==> class 1 echo (3 s)"
out="$("$bin" io "$host" --config 151 --out 150 --in 100 --out-len 32 --in-len 32 --rpi 10 --seconds 3 --set "$pattern")"
echo "$out" | tail -n 2
want="$(printf '%02x ' $(seq 1 32) | sed 's/ $//')"
if echo "$out" | grep -qx "$want"; then
    echo "PASS: $host echoed 32 bytes over Class 1"
else
    echo "FAIL: never saw the output come back as input (is the default program loaded?)" >&2
    exit 1
fi
