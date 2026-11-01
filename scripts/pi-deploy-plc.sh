#!/usr/bin/env bash
# Build helmsman-plc (IEC 61131-3 runtime + CIP adapter) for a 64-bit ARM
# Pi on this machine and run it there as a systemd service, serving the
# canvas (the browser editor, built here as wasm) from its API.
#
# Projects (the default): the service runs `helmsman-plc --projects`, the
# editor's projects in /var/lib/helmsman-plc/projects/ and the deployed one
# booted from its store. On a first install (no project, nothing deployed)
# it seeds project `line` from crates/helmsman-plc/programs/line.st (or the
# .st given) and deploys it, so the box runs it from the first boot; a later
# deploy never touches projects (`helmsman-plc project seed`, run by the
# unit before every start, only acts on a first install).
# PLC_MODE=program keeps the old mode: one PROGRAM.st or PROJECT.xml file.
#
# Editing, saving, deploying and forcing need an engineer login. A fresh
# box has no accounts (its role PINs work until one exists); make one:
#   scripts/pi-account.sh user@host nick engineer      (asks the password)
# which runs `helmsman-plc account add nick engineer` on the Pi as root and
# hands the accounts file to the service's user.
#
# Safe to re-run: rebuilds, reinstalls the binary, canvas and unit, restarts.
# Also retires the pre-rename tcpethernet-plc unit and its firewall rules.
#
# Needs: Docker here (the build runs in the pinned rust:*-slim-trixie image as
# linux/arm64: native on the Studio, emulated on x86), key SSH and
# passwordless sudo on the Pi (scripts/pi-prep.sh), Debian Trixie on the Pi
# (glibc must match the build image), and trunk here for the canvas
# (`cargo install trunk --locked`; the wasm is the same on every machine, so
# it is built here, not in the arm64 image). Runs here: `! scripts/pi-deploy-plc.sh nick@<pi-ip>`.
# Then smoke it from here: scripts/adapter-smoke.sh <pi-ip> (needs echo.st).
#
# Usage: scripts/pi-deploy-plc.sh user@host [SEED.st]
#        PLC_MODE=program scripts/pi-deploy-plc.sh user@host [PROGRAM.st|PROJECT.xml]
#        PLC_MODE=projects (default) | program
#        projects: SEED.st seeds a first install as project <its name>
#                  (default crates/helmsman-plc/programs/line.st: project line)
#        program:  default program crates/helmsman-plc/programs/echo.st
#        PLC_SCAN_MS=5     scan period in ms (default 10)
#        PLC_ALLOW=10.0.0.0/24   firewall source (default 192.168.3.0/24)
#        PLC_API=0.0.0.0:8080    box API + canvas address (default
#                 127.0.0.1:8080: the Pi only, reach it with
#                 `ssh -L 8080:127.0.0.1:8080 user@host`; 0.0.0.0:8080 serves
#                 the LAN and opens 8080/tcp from PLC_ALLOW. Reads are open:
#                 TODO.md api-lan-reads)
#        PLC_UI_ALLOW=10.0.0.0/24  source for the API + canvas port only
#                 (default PLC_ALLOW), e.g. the Wi-Fi a laptop browses from
#                 while the CIP ports stay on the wired link
# Undo: sudo systemctl disable --now helmsman-plc, remove the unit,
#       /usr/local/bin/helmsman-plc, /usr/local/share/helmsman-plc and
#       /etc/helmsman-plc; the firewall
#       rules are runtime-only and go with a reboot or perceptronics reload.
set -euo pipefail

target="${1:?usage: $0 user@host [SEED.st] (PLC_MODE=program: [PROGRAM.st|PROJECT.xml])}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
mode="${PLC_MODE:-projects}"
case "$mode" in
    projects) program="${2:-$repo/crates/helmsman-plc/programs/line.st}" ;;
    program) program="${2:-$repo/crates/helmsman-plc/programs/echo.st}" ;;
    *) echo "PLC_MODE must be projects or program: $mode" >&2; exit 2 ;;
esac
scan_ms="${PLC_SCAN_MS:-10}"
allow="${PLC_ALLOW:-192.168.3.0/24}"
api="${PLC_API:-127.0.0.1:8080}"
ui_allow="${PLC_UI_ALLOW:-$allow}"
api_port="${api##*:}"
ui_dir=/usr/local/share/helmsman-plc/ui
image="$(sed -n 's#^FROM \(rust:[^ ]*\) AS build$#\1#p' "$repo/docker/adapter/Dockerfile")"

[ -f "$program" ] || { echo "no program at $program" >&2; exit 2; }
case "$program" in
    *.st | *.ST) ext=st ;;
    *.xml | *.XML) ext=xml ;;
    *) echo "program must be .st or .xml: $program" >&2; exit 2 ;;
esac
# The seeded project's name and file: the .st's own (line.st: project line).
seed_file="$(basename "$program")"
seed_name="$(printf '%s' "${seed_file%.*}" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9_\n-' '-')"
if [ "$mode" = projects ]; then
    [ "$ext" = st ] || { echo "a project seed is one .st file: $program" >&2; exit 2; }
    case "$seed_name" in
        [a-z0-9]*) ;;
        *) echo "cannot name a project after $seed_file" >&2; exit 2 ;;
    esac
fi
[ -n "$image" ] || { echo "no rust build image in docker/adapter/Dockerfile" >&2; exit 1; }
toolchain="${image#rust:}"
toolchain="${toolchain%%-*}"
docker info >/dev/null 2>&1 || { echo "Docker is not running here" >&2; exit 1; }
command -v trunk >/dev/null || { echo "needs trunk here for the canvas: cargo install trunk --locked" >&2; exit 1; }
case "$api_port" in
    "" | *[!0-9]*) echo "PLC_API must be IP:PORT: $api" >&2; exit 2 ;;
esac

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

echo "==> build the canvas (wasm, trunk)"
"$repo/scripts/canvas-web.sh" >/dev/null
dist="$repo/target/canvas-dist"
wasm="$(cd "$dist" && find . -maxdepth 1 -name '*.wasm' -print -quit)"
if [ ! -f "$dist/index.html" ] || [ -z "$wasm" ]; then
    echo "the canvas build left no index.html and .wasm in $dist" >&2
    exit 1
fi
wasm="${wasm#./}"

echo "==> copy"
ssh "$target" 'rm -rf ~/.helmsman-plc/ui && mkdir -p ~/.helmsman-plc'
scp -q "$bin" "$repo/deploy/helmsman-plc.service" "$target:.helmsman-plc/"
scp -q -r "$dist" "$target:.helmsman-plc/ui"
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
if [ "$mode" = projects ]; then
    source_arg=--projects
    seed_arg="$seed_name /etc/helmsman-plc/seed/$seed_file"
else
    source_arg="/etc/helmsman-plc/program.$ext"
    seed_arg=
fi
# shellcheck disable=SC2029 # $ext, $scan_ms, $api, $ui_dir and the seed are meant to expand here.
ssh "$target" "set -e
cd ~/.helmsman-plc
sudo install -m 755 helmsman-plc /usr/local/bin/helmsman-plc
sudo install -d -m 755 /etc/helmsman-plc /etc/helmsman-plc/seed
sudo rm -f /etc/helmsman-plc/program.st /etc/helmsman-plc/program.xml /etc/helmsman-plc/seed/*.st
if [ '$mode' = projects ]; then
    sudo install -m 644 program.$ext /etc/helmsman-plc/seed/$seed_file
else
    sudo install -m 644 program.$ext /etc/helmsman-plc/program.$ext
fi
# The canvas: replaced whole (its file names carry content hashes), read-only
# for the service.
sudo rm -rf $ui_dir
sudo install -d -m 755 $ui_dir
sudo cp -R ui/. $ui_dir/
sudo chown -R root:root $ui_dir
sudo chmod -R u=rwX,go=rX $ui_dir
printf 'PLC_SOURCE=%s\nPLC_SEED=%s\nPLC_SCAN_MS=%s\nPLC_API=%s\nPLC_UI=%s\n' \
    '$source_arg' '$seed_arg' $scan_ms $api $ui_dir | sudo tee /etc/helmsman-plc/env >/dev/null
sudo install -m 644 helmsman-plc.service /etc/systemd/system/helmsman-plc.service
sudo systemctl daemon-reload
sudo systemctl enable helmsman-plc >/dev/null 2>&1
sudo systemctl restart helmsman-plc"

echo "==> firewall"
# A pick PC carries perceptronics' nftables table (input policy drop). Accept
# the PLC's ports from $PLC_ALLOW only, as runtime rules: they vanish on reboot
# or a perceptronics firewall reload, and a redeploy puts them back. No table,
# no change.
# shellcheck disable=SC2029 # $allow, $ui_allow, $api and $api_port are meant to expand here.
ssh "$target" "set -e
if sudo nft list table inet perceptronics >/dev/null 2>&1; then
    if sudo nft list chain inet perceptronics input | grep -q 'comment \"helmsman-plc\"'; then
        echo 'rules already present'
    else
        sudo nft add rule inet perceptronics input ip saddr $allow tcp dport 44818 accept comment '\"helmsman-plc\"'
        sudo nft add rule inet perceptronics input ip saddr $allow udp dport '{ 44818, 2222 }' accept comment '\"helmsman-plc\"'
        echo 'opened 44818 tcp+udp, 2222 udp from $allow'
    fi
    case '$api' in
        127.*) ;;
        *)
            if sudo nft list chain inet perceptronics input | grep -q 'comment \"helmsman-plc-api\"'; then
                echo 'API rule already present'
            else
                sudo nft add rule inet perceptronics input ip saddr $ui_allow tcp dport $api_port accept comment '\"helmsman-plc-api\"'
                echo 'opened $api_port tcp (API + canvas) from $ui_allow'
            fi ;;
    esac
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
# The token line stays in the journal, never in this script's output.
ssh "$target" "systemctl is-active helmsman-plc; journalctl -u helmsman-plc -n 5 --no-pager | grep -v 'API token'" || true
[ -n "$up" ] || { echo "FAIL: helmsman-plc is not listening on 44818 (journal above)" >&2; exit 1; }
# The box API on the Pi's loopback: proves StateDirectory= let it write its
# token under the unit's hardening. The token itself stays in the journal.
local_api="127.0.0.1:$api_port"
answering=
for _ in $(seq 1 10); do
    # shellcheck disable=SC2029 # $local_api is meant to expand here.
    if ssh "$target" "curl -fsS --max-time 2 http://$local_api/api/modules >/dev/null"; then
        answering=1
        break
    fi
    sleep 1
done
[ -n "$answering" ] || { echo "FAIL: box API not answering on the Pi's $local_api" >&2; exit 1; }
echo "box API answers on the Pi's $local_api; token: ssh $target journalctl -u helmsman-plc | grep 'API token'"
# The canvas: the page, and its wasm with the type browsers insist on.
# shellcheck disable=SC2029 # $local_api and $wasm are meant to expand here.
ssh "$target" "curl -fsS --max-time 2 http://$local_api/ | grep -q '<canvas' \
    && curl -fsS --max-time 5 -o /dev/null -w '%{content_type}' http://$local_api/$wasm | grep -qx application/wasm" \
    || { echo "FAIL: the canvas is not served at http://$local_api/" >&2; exit 1; }
# HTTPS on the same port (box-https): plain HTTP above is served only
# because it comes from the Pi itself. -k: the certificate is self-signed;
# its fingerprint, printed below, is what a person checks instead.
# shellcheck disable=SC2029 # $local_api is meant to expand here.
ssh "$target" "curl -fsSk --max-time 2 -o /dev/null https://$local_api/api/modules" \
    || { echo "FAIL: the box API does not answer over HTTPS on the Pi's $local_api" >&2; exit 1; }
# shellcheck disable=SC2029 # $local_api is meant to expand here.
fingerprint=$(ssh "$target" "curl -fsS --max-time 2 http://$local_api/api/policy" \
    | grep -o '"fingerprint":"[0-9A-F:]*"' | cut -d'"' -f4 || true)
if [ "$mode" = projects ]; then
    # shellcheck disable=SC2029 # $local_api is meant to expand here.
    running="$(ssh "$target" "curl -fsS --max-time 2 http://$local_api/api/running")" \
        || { echo "FAIL: the box answers without projects (is it running --projects?)" >&2; exit 1; }
    case "$running" in
        *'"current":true'*) echo "running: $running" ;;
        *) echo "projects on, nothing deployed and current: $running (journal: ssh $target journalctl -u helmsman-plc)" ;;
    esac
fi
host="${target#*@}"
echo
if [ "$mode" = projects ]; then
    echo "PLC is up on $host with projects (first install seeds $seed_name), scanning every ${scan_ms} ms"
    echo "  engineer:   scripts/pi-account.sh $target USER engineer   (once, to edit and deploy)"
else
    echo "PLC is up on $host running $(basename "$program") every ${scan_ms} ms"
fi
echo "  from here:  scripts/adapter-smoke.sh $host"
case "$api" in
    127.*) echo "  canvas:     ssh -L $api_port:127.0.0.1:$api_port $target, then https://localhost:$api_port/" ;;
    *) echo "  canvas:     https://$host:$api_port/ from $allow" ;;
esac
echo "  HTTPS certificate SHA-256 (self-signed: check the browser shows this, then trust it):"
echo "              ${fingerprint:-unknown; ssh $target journalctl -u helmsman-plc | grep fingerprint}"
