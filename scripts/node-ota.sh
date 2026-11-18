#!/usr/bin/env bash
# Update a node's firmware over the network, no cable: serve the image from
# this machine, tell the node to fetch it, wait until it has restarted into
# it and confirmed it (docs/EDGE.md, "Updating a node without a cable").
#
# Usage: NODE=192.168.3.56 NODE_TOKEN=... scripts/node-ota.sh [image.bin]
#
#   NODE         the node's address (its API port is 8080 unless NODE_PORT)
#   NODE_TOKEN   the node's API token (the page's Settings; never printed)
#   image.bin    default target/node-images/node.bin, which
#                `NODE_BOARD=p4-eth scripts/node-build.sh image node` writes
#   HOST_IP      this machine's address as the node sees it (default: the
#                address of the interface that routes to the node)
#   SERVE_PORT   default 8000
#   CONFIRM_NOW=1  confirm as soon as the node is up, instead of letting it
#                confirm itself after 30 s of running
#
# The node needs the A/B flash layout (one USB flash with
# scripts/node-flash.sh, once). A node with the old layout answers 404 on
# /api/firmware: this stops there. If the new image never confirms the node
# puts the old one back by itself; this waits for that too and says so.
#
# Apple's python and curl serve and ask: macOS Local Network privacy refuses
# LAN traffic to other binaries started from an agent session or over SSH.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${NODE:?set NODE to the node's address}"
: "${NODE_TOKEN:?set NODE_TOKEN to the node's API token}"
port="${NODE_PORT:-8080}"
serve_port="${SERVE_PORT:-8000}"
image="${1:-target/node-images/node.bin}"
[ -f "$image" ] || { echo "no image at $image (NODE_BOARD=p4-eth scripts/node-build.sh image node)" >&2; exit 1; }
api="http://$NODE:$port"

ask() { /usr/bin/curl -sS --max-time 10 "$@"; }

state="$(ask "$api/api/firmware" || true)"
case "$state" in
  *'"fetch"'*) ;;
  *) echo "the node at $NODE has no firmware API: $state" >&2
     echo "flash it once over USB with the A/B table: NODE_BOARD=p4-eth scripts/node-flash.sh node" >&2
     exit 1 ;;
esac
field() { /usr/bin/python3 -I -c 'import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {}, {"d": d}))' "$1"; }
before="$(echo "$state" | field 'd.get("build") or d["version"]')"
slot_before="$(echo "$state" | field 'd["slot"]')"
echo "node $NODE runs $before from slot $slot_before"

host_ip="${HOST_IP:-}"
if [ -z "$host_ip" ]; then
  iface="$(/sbin/route -n get "$NODE" 2>/dev/null | awk '/interface:/ { print $2 }')"
  host_ip="$(/usr/sbin/ipconfig getifaddr "$iface" 2>/dev/null || true)"
fi
[ -n "$host_ip" ] || { echo "cannot tell which address of this machine the node can reach: set HOST_IP" >&2; exit 1; }

dir="$(mktemp -d "${TMPDIR:-/tmp}/node-ota.XXXXXX")"
trap 'kill "${server:-0}" 2>/dev/null || true; rm -rf "$dir"' EXIT
cp "$image" "$dir/firmware.bin"
sha="$(/usr/bin/shasum -a 256 "$dir/firmware.bin" | awk '{ print $1 }')"
/usr/bin/python3 -I -m http.server "$serve_port" --bind "$host_ip" --directory "$dir" >"$dir/server.log" 2>&1 &
server=$!
disown "$server"
sleep 1
kill -0 "$server" 2>/dev/null || { echo "could not serve on $host_ip:$serve_port:"; cat "$dir/server.log"; exit 1; }

echo "serving $image ($sha) at http://$host_ip:$serve_port/firmware.bin"
reply="$(ask -X POST "$api/api/firmware" -H "Authorization: Bearer $NODE_TOKEN" -H 'Content-Type: application/json' \
  -d "{\"url\":\"http://$host_ip:$serve_port/firmware.bin\",\"sha256\":\"$sha\"}" -w ' [%{http_code}]')"
case "$reply" in
  *'[202]') echo "the node took the request" ;;
  *) echo "the node refused: $reply" >&2; exit 1 ;;
esac

# Fetching, then a restart (the API is gone for a few seconds), then on trial.
deadline=$(( $(date +%s) + 300 ))
while :; do
  [ "$(date +%s)" -lt "$deadline" ] || { echo "gave up after 5 minutes" >&2; exit 1; }
  sleep 2
  s="$(ask "$api/api/firmware" 2>/dev/null || true)"
  [ -n "$s" ] || continue
  fetch="$(echo "$s" | field 'd["fetch"]["state"]' 2>/dev/null || true)"
  case "$fetch" in
    failed) echo "the fetch failed: $(echo "$s" | field 'd["fetch"]["error"]')" >&2; exit 1 ;;
    fetching) printf '\r  %s of %s bytes' "$(echo "$s" | field 'd["fetch"]["received"]')" "$(echo "$s" | field 'd["fetch"]["size"]')" ;;
    idle)
      slot="$(echo "$s" | field 'd["slot"]')"
      if [ "$slot" != "$slot_before" ]; then break; fi ;;
  esac
done
echo
echo "restarted: $(echo "$s" | field 'd.get("build") or d["version"]') in slot $slot, trial $(echo "$s" | field 'd["trial"]["state"]')"
if [ "${CONFIRM_NOW:-}" = 1 ]; then
  ask -X POST "$api/api/firmware/confirm" -H "Authorization: Bearer $NODE_TOKEN" && echo
fi
# Confirmed by itself (30 s of running) or by the call above. A node that
# keeps restarting never gets there: after its fourth boot it is back on
# the old image, in the slot it was in.
deadline=$(( $(date +%s) + 240 ))
while :; do
  [ "$(date +%s)" -lt "$deadline" ] || { echo "still not confirmed after 4 minutes: $s" >&2; exit 1; }
  sleep 3
  s="$(ask "$api/api/firmware" 2>/dev/null || true)"
  [ -n "$s" ] || continue
  slot="$(echo "$s" | field 'd["slot"]' 2>/dev/null || true)"
  t="$(echo "$s" | field 'd["trial"]["state"]' 2>/dev/null || true)"
  if [ "$t" = confirmed ]; then
    if [ "$slot" = "$slot_before" ]; then
      echo "the new image did not confirm; the node is back on $(echo "$s" | field 'd.get("build") or d["version"]') in slot $slot" >&2
      exit 1
    fi
    echo "confirmed: $(echo "$s" | field 'd.get("build") or d["version"]') in slot $slot"
    exit 0
  fi
done
