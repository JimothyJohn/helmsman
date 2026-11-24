#!/usr/bin/env bash
#
# install.sh: put Helmsman on a Linux machine from a release.
#
# One static binary, the browser editor it serves, one systemd unit. No
# Docker and no toolchain on the machine. Run as root on it:
#
#   curl -fsSL https://raw.githubusercontent.com/JimothyJohn/helmsman/master/scripts/install.sh \
#     | sudo bash -s -- v0.1.0
#
#   # or from an archive already on the machine (no network, or a build
#   # of your own; its .sha256 beside it is checked when it is there):
#   sudo ./install.sh --local helmsman-x86_64-unknown-linux-musl.tar.gz
#
# Add --lan to serve the editor on the network (0.0.0.0:8080) instead of
# this machine only (127.0.0.1:8080). The installer opens no firewall port.
#
# Run again with a newer tag to upgrade: the binary, the editor and the
# unit are replaced and the service restarted. /etc/helmsman-plc/env, the
# relay token and everything under /var/lib/helmsman-plc (projects,
# history, accounts) are never touched.
#
# Afterwards:
#   helmsman-plc account add <name> engineer    the first login (asks the password)
#   journalctl -u helmsman-plc | grep 'API token'
#
set -e
set -o nounset
set -o pipefail
# The patterns below are over bytes whatever the caller's locale.
export LC_ALL=C

REPO="${REPO:-JimothyJohn/helmsman}"
# A test root: every path below it, no root needed, systemd left alone.
ROOT="${HELMSMAN_INSTALL_ROOT:-}"
PREFIX="$ROOT${PREFIX:-/usr/local}"
ETC="$ROOT/etc/helmsman-plc"
UNIT_DIR="$ROOT/etc/systemd/system"
SERVICE="helmsman-plc"
UI_DIR="$PREFIX/share/$SERVICE/ui"

log() { printf '[install] %s\n' "$*"; }
die() {
    printf '[install] ERROR: %s\n' "$*" >&2
    exit 1
}

usage() {
    sed -n '2,/^set -e/p' "$0" | sed 's/^# \{0,1\}//' | sed '$d'
    exit "${1:-0}"
}

lan=0
local_file=""
tag=""
while [ $# -gt 0 ]; do
    case "$1" in
    -h | --help) usage 0 ;;
    --lan) lan=1 ;;
    --local)
        [ -n "${2:-}" ] || usage 1
        local_file="$2"
        shift
        ;;
    -*) die "unknown option: $1" ;;
    *)
        [ -z "$tag" ] || die "one release tag, not two: $tag and $1"
        tag="$1"
        ;;
    esac
    shift
done
[ -n "$tag" ] || [ -n "$local_file" ] || usage 1
[ -z "$tag" ] || [ -z "$local_file" ] || die "a release tag or --local FILE, not both"
# A tag goes into a URL: v, digits and dots, and nothing that could leave
# the releases folder.
if [ -n "$tag" ]; then
    case "$tag" in
    v[0-9]*) ;;
    *) die "not a release tag: $tag (expected e.g. v0.1.0)" ;;
    esac
    case "$tag" in
    *[!A-Za-z0-9.-]* | *..*) die "not a release tag: $tag (expected e.g. v0.1.0)" ;;
    esac
fi

if [ -z "$ROOT" ]; then
    [ "$(id -u)" -eq 0 ] || die "run as root (sudo): installs under $PREFIX, $ETC and $UNIT_DIR"
    command -v systemctl >/dev/null 2>&1 || die "systemd not found: this installer only knows systemd"
fi

machine="${HELMSMAN_INSTALL_MACHINE:-$(uname -m)}"
case "$machine" in
aarch64 | arm64) TARGET="aarch64-unknown-linux-musl" ;;
x86_64 | amd64) TARGET="x86_64-unknown-linux-musl" ;;
*) die "unsupported machine: $machine (releases are built for aarch64 and x86_64)" ;;
esac
ARCHIVE="helmsman-$TARGET.tar.gz"

sha256_check() { # <dir> <file with "HASH  NAME">
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$1" && sha256sum -c "$2" >/dev/null 2>&1)
    else
        (cd "$1" && shasum -a 256 -c "$2" >/dev/null 2>&1)
    fi
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ -n "$local_file" ]; then
    [ -f "$local_file" ] || die "no such file: $local_file"
    log "using the archive $local_file"
    cp "$local_file" "$WORK/$ARCHIVE"
    if [ -f "$local_file.sha256" ]; then
        # The line names the archive as it was built; check it under ours.
        sed "s|  .*|  $ARCHIVE|" "$local_file.sha256" >"$WORK/$ARCHIVE.sha256"
    fi
else
    BASE="https://github.com/$REPO/releases/download/$tag"
    command -v curl >/dev/null 2>&1 || die "curl not found"
    log "fetching $ARCHIVE from $BASE"
    curl -fsSL -o "$WORK/$ARCHIVE" "$BASE/$ARCHIVE" || die "download failed: $BASE/$ARCHIVE"
    # A release always carries the checksum: without it nothing is installed.
    curl -fsSL -o "$WORK/$ARCHIVE.sha256" "$BASE/$ARCHIVE.sha256" || die "download failed: $BASE/$ARCHIVE.sha256"
fi

if [ -f "$WORK/$ARCHIVE.sha256" ]; then
    log "verifying sha256"
    sha256_check "$WORK" "$ARCHIVE.sha256" || die "checksum mismatch for $ARCHIVE: nothing installed"
else
    log "WARNING: no .sha256 beside the archive: its integrity is not verified"
fi

# Only what a release holds, and nothing that could land outside $WORK:
# the binary, the unit, and files under ui/.
names="$(tar -tzf "$WORK/$ARCHIVE")" || die "$ARCHIVE is not a readable archive: nothing installed"
while IFS= read -r name; do
    case "$name" in
    "" | helmsman-plc | helmsman-plc.service | ui/) ;;
    /* | *..* | *//*) die "the archive holds a path it should not ($name): nothing installed" ;;
    ui/*) ;;
    *) die "the archive holds a file it should not ($name): nothing installed" ;;
    esac
done <<EOF
$names
EOF
mkdir "$WORK/x"
tar -xzf "$WORK/$ARCHIVE" -C "$WORK/x"
[ -f "$WORK/x/helmsman-plc" ] && [ ! -L "$WORK/x/helmsman-plc" ] || die "the archive holds no helmsman-plc: nothing installed"
[ -f "$WORK/x/helmsman-plc.service" ] || die "the archive holds no helmsman-plc.service: nothing installed"
[ -f "$WORK/x/ui/index.html" ] || die "the archive holds no editor (ui/index.html): nothing installed"
if [ -n "$(find "$WORK/x" -type l)" ]; then
    die "the archive holds a symbolic link: nothing installed"
fi

log "installing $PREFIX/bin/$SERVICE"
install -d -m 0755 "$PREFIX/bin"
install -m 0755 "$WORK/x/helmsman-plc" "$PREFIX/bin/$SERVICE.new"
mv -f "$PREFIX/bin/$SERVICE.new" "$PREFIX/bin/$SERVICE"

# The editor, whole: built aside and swapped in, so a browser never loads
# half of the old one and half of the new.
log "installing the editor in $UI_DIR"
install -d -m 0755 "$(dirname "$UI_DIR")"
rm -rf "$UI_DIR.new"
cp -R "$WORK/x/ui" "$UI_DIR.new"
chmod -R u=rwX,go=rX "$UI_DIR.new"
rm -rf "$UI_DIR.old"
[ ! -d "$UI_DIR" ] || mv "$UI_DIR" "$UI_DIR.old"
mv "$UI_DIR.new" "$UI_DIR"
rm -rf "$UI_DIR.old"

log "preparing $ETC"
install -d -m 0755 "$ETC"
# The unit loads the relay token as a credential; the file has to be there.
[ -f "$ETC/relay-token" ] || install -m 0600 /dev/null "$ETC/relay-token"
if [ ! -f "$ETC/env" ]; then
    api="127.0.0.1:8080"
    [ "$lan" -eq 0 ] || api="0.0.0.0:8080"
    # The unit's paths are the real ones, with or without a test root.
    printf 'PLC_SOURCE=--projects\nPLC_SEED=\nPLC_SCAN_MS=10\nPLC_API=%s\nPLC_UI=%s\nPLC_RELAY_ARGS=\n' \
        "$api" "${UI_DIR#"$ROOT"}" >"$ETC/env"
    chmod 0644 "$ETC/env"
    log "  wrote $ETC/env (the editor on $api)"
else
    log "  keeping the $ETC/env that is there"
    [ "$lan" -eq 0 ] || log "  --lan changes nothing on an install that has its env: set PLC_API in it"
fi

log "installing the unit"
install -d -m 0755 "$UNIT_DIR"
install -m 0644 "$WORK/x/helmsman-plc.service" "$UNIT_DIR/$SERVICE.service"

if [ -n "$ROOT" ]; then
    log "installed under $ROOT (a test root: systemd left alone)"
    exit 0
fi
systemctl daemon-reload
systemctl enable "$SERVICE" >/dev/null 2>&1 || true
log "(re)starting $SERVICE"
systemctl restart "$SERVICE"
sleep 1
systemctl --no-pager --lines=5 status "$SERVICE" || true
addr="$(sed -n 's/^PLC_API=//p' "$ETC/env" | head -1)"
case "$addr" in
127.* | "") log "done. The editor, from this machine: http://${addr:-127.0.0.1:8080}/" ;;
*) log "done. The editor, from the network: https://<this machine's address>:${addr##*:}/ (its certificate's fingerprint is in the journal)" ;;
esac
log "  the first login:   $SERVICE account add <name> engineer"
log "  the box's token:   journalctl -u $SERVICE | grep 'API token'"
