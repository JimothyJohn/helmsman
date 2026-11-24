#!/usr/bin/env bash
# Pack one release archive: the box binary, the browser editor it serves
# and its systemd unit, with a .sha256 beside it. scripts/install.sh
# installs exactly this and refuses an archive that holds anything else.
#
#   scripts/package-release.sh TARGET BINARY UI_DIR OUT_DIR
#
#   TARGET   aarch64-unknown-linux-musl | x86_64-unknown-linux-musl
#   BINARY   the built helmsman-plc for TARGET
#   UI_DIR   target/canvas-dist (make canvas-web)
#   OUT_DIR  where helmsman-TARGET.tar.gz and its .sha256 are written
#
# Needs: tar, sha256sum or shasum. The .github/workflows/release.yml jobs
# call it; run it by hand to try an install from a local build:
#   sudo scripts/install.sh --local OUT_DIR/helmsman-TARGET.tar.gz
set -euo pipefail
export LC_ALL=C

[ $# -eq 4 ] || {
    sed -n '2,/^set -e/p' "$0" | sed 's/^# \{0,1\}//' | sed '$d' >&2
    exit 2
}
target="$1"
bin="$2"
ui="$3"
out="$4"
repo="$(cd "$(dirname "$0")/.." && pwd)"

case "$target" in
aarch64-unknown-linux-musl | x86_64-unknown-linux-musl) ;;
*)
    echo "[package] not a release target: $target" >&2
    exit 2
    ;;
esac
[ -f "$bin" ] || {
    echo "[package] no binary at $bin" >&2
    exit 1
}
[ -f "$ui/index.html" ] || {
    echo "[package] no editor at $ui (make canvas-web)" >&2
    exit 1
}

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
install -m 0755 "$bin" "$stage/helmsman-plc"
install -m 0644 "$repo/deploy/helmsman-plc.service" "$stage/helmsman-plc.service"
cp -R "$ui" "$stage/ui"

mkdir -p "$out"
out="$(cd "$out" && pwd)"
name="helmsman-$target.tar.gz"
# Named members in a fixed order, and nothing of the machine that packs
# it: a Mac's tar adds a `._name` beside any file that carries extended
# attributes, and install.sh refuses an archive that holds one.
xattrs=""
tar --no-xattrs -cf /dev/null -T /dev/null 2>/dev/null && xattrs="--no-xattrs"
(cd "$stage" && COPYFILE_DISABLE=1 tar $xattrs -czf "$out/$name" helmsman-plc helmsman-plc.service ui)
if tar -tzf "$out/$name" | grep -Ev '^(helmsman-plc|helmsman-plc\.service|ui/.*)$' >&2; then
    echo "[package] the archive holds the names above, which a release must not" >&2
    rm -f "$out/$name"
    exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$out" && sha256sum "$name" >"$name.sha256")
else
    (cd "$out" && shasum -a 256 "$name" >"$name.sha256")
fi
echo "[package] $out/$name"
cat "$out/$name.sha256"
