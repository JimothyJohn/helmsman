#!/usr/bin/env bash
# The Moxa ioLogik E1213 on the bench, as the box sees it: helmsman's
# enip-io driver mastering it over one Class 1 exclusive-owner connection
# (docs/devices/moxa-iologik-e1213.md).
#
#   1. its EDS against the built-in profile (if the EDS file is at hand);
#   2. ListIdentity: it answers, vendor 991, product code 515;
#   3. its Identity object over an explicit session;
#   4. the conformance harness over Class 1 at 100 ms: reads of DI bits, a
#      counter and the DO status, every wired DO->DI loop (a write proven by
#      the wire, not by read-back), a point past the 51-byte input assembly.
#
# Needs:
#   - the E1213 on the Studio link (192.168.3.x), powered, its IP given;
#   - EtherNet/IP enabled on its web console AND activated for this unit
#     (manual pp. 3-26, 5-1); without that it ignores ListIdentity or
#     refuses the Forward_Open;
#   - DO-00..DO-03 wired to DI-00..DI-03 (the io-bench wiring), with field
#     power on the DO side; other wiring: LOOPS="do:0=di:4 do:1=di:5";
#   - UDP 2222 free on this machine (nothing else scanning; not the box's
#     own adapter), since the E1213 sends its inputs there;
#   - macOS: the conformance binary is cargo-built and unsigned, so the
#     firewall asks once whether it may accept incoming UDP. Run this in the
#     native terminal the first time (over SSH nobody can click "Allow" and
#     the inputs never arrive: every read then times out).
# Idempotent and read-mostly: it drives DO-00..03 on and off and leaves them
# as they were. Nothing is written to the device's configuration.
#
# Usage: scripts/bench-e1213.sh IP
set -euo pipefail
host="${1:?usage: $0 IP (the E1213 on the Studio link)}"
loops="${LOOPS:-do:0=di:0 do:1=di:1 do:2=di:2 do:3=di:3}"
cd "$(dirname "$0")/.."

eds=moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds
if [ -f "${E1213_EDS:-$eds}" ]; then
  echo "==> EDS"
  E1213_EDS="${E1213_EDS:-$PWD/$eds}" scripts/eds-corpus.sh >/dev/null
  echo "EDS v2.0 resolves to the built-in moxa-e1213 profile"
else
  echo "==> EDS: no $eds here (E1213_EDS=PATH to check it); using the built-in profile"
fi

cargo build --release -q --locked -p helmsman-proto --example discover
cargo build --release -q --locked -p helmsman-io --example conformance
discover=target/release/examples/discover
conformance=target/release/examples/conformance

echo "==> discover (ListIdentity, with the identity it answers)"
found="$("$discover" "$host" 1500)"
echo "$found"
if ! echo "$found" | grep -q "vendor=991 .*code=515"; then
  echo "FAIL: no ListIdentity reply from an E1213 (vendor 991, product code 515) at $host" >&2
  echo "      (EtherNet/IP enabled and activated? manual pp. 3-26, 5-1)" >&2
  exit 1
fi

echo "==> class 1 conformance (exclusive owner, RPI 100 ms)"
args=(--enip "$host" --param profile=moxa-e1213 --param rpi_ms=100 --settle-ms 1000)
for l in $loops; do
  args+=(--loop "$l")
done
args+=(--point di:0 --point in:2:u32 --point in:50.0 --point do:0 --absent in:51.0)
"$conformance" "${args[@]}"
echo "PASS: the E1213 at $host is mastered over Class 1"
