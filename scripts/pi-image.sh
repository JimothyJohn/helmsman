#!/usr/bin/env bash
# Build a Raspberry Pi OS image with Helmsman preinstalled: the box binary,
# the canvas, the systemd unit, and a first-boot step that pairs the box
# with the relay from files on the boot partition (docs/BOX.md). The
# `box-sku` card's walking skeleton: an image anyone can flash, no
# enclosure or BOM.
#
# How: pi-gen (RPi-Distro/pi-gen, the official image builder) in its own
# Docker build (`build-docker.sh`), with one custom stage after stage2
# (deploy/pi-image/stage-helmsman) that installs what scripts/pi-deploy-plc.sh
# installs over SSH. The binary is built the way pi-deploy builds it (the
# pinned rust image, linux/arm64, Trixie's glibc); the canvas with trunk.
#
# Needs: Docker here (Apple silicon builds arm64 natively; x86 emulates),
#        trunk (`cargo install trunk --locked`), git, ~15 GB free, an hour.
# Pin:   deploy/pi-image/pi-gen.ref names the pi-gen commit or tag to use;
#        the script refuses a branch name unless PI_GEN_UNPINNED=1, and
#        prints the commit it built from so the ref can be written down.
# Does:  build binary + canvas; clone pi-gen at the ref into
#        target/pi-image/pi-gen; write its config; copy the stage in;
#        run pi-gen; copy the image to target/pi-image/*.img.xz with its
#        SHA-256 beside it.
# Verifies: the stage's manifest (what went in) and the image file; a boot
#        in QEMU is not part of the skeleton (docs/BOX.md says how to check
#        the image by hand).
# Runs in your own shell (long; Docker): scripts/pi-image.sh
#
# Usage: scripts/pi-image.sh [SEED.st]
#        SEED.st seeds a first install as a project named after it
#        (default crates/helmsman-plc/programs/line.st: project line).
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
seed="${1:-$repo/crates/helmsman-plc/programs/line.st}"
work="$repo/target/pi-image"
stage_src="$repo/deploy/pi-image/stage-helmsman"
ref_file="$repo/deploy/pi-image/pi-gen.ref"
image="$(sed -n 's#^FROM \(rust:[^ ]*\) AS build$#\1#p' "$repo/docker/adapter/Dockerfile")"

die() { echo "error: $*" >&2; exit 1; }
command -v docker >/dev/null || die "Docker is not installed"
docker info >/dev/null 2>&1 || die "Docker is not running"
command -v trunk >/dev/null || die "needs trunk for the canvas: cargo install trunk --locked"
command -v git >/dev/null || die "needs git to fetch pi-gen"
[ -f "$seed" ] || die "no seed program at $seed"
[ -d "$stage_src" ] || die "no stage at $stage_src"
[ -f "$ref_file" ] || die "no $ref_file (the pi-gen commit or tag to build from)"
[ -n "$image" ] || die "no rust build image in docker/adapter/Dockerfile"
ref="$(tr -d '[:space:]' < "$ref_file")"
case "$ref" in
    arm64 | master | main)
        [ "${PI_GEN_UNPINNED:-}" = 1 ] || die "$ref_file names a branch ($ref), not a commit or tag; pin it, or PI_GEN_UNPINNED=1 to build from its head once"
        ;;
esac
seed_file="$(basename "$seed")"
seed_name="$(printf '%s' "${seed_file%.*}" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9_\n-' '-')"
case "$seed_name" in
    [a-z0-9]*) ;;
    *) die "cannot name a project after $seed_file" ;;
esac
toolchain="${image#rust:}"
toolchain="${toolchain%%-*}"

echo "==> build linux/arm64 helmsman-plc in $image"
docker run --rm --platform linux/arm64 \
    -v "$repo:/src" -w /src \
    -v helmsman-cargo-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/src/target/pi \
    -e RUSTUP_TOOLCHAIN="$toolchain" \
    "$image" cargo build --release --locked -q -p helmsman-plc
bin="$repo/target/pi/release/helmsman-plc"
file "$bin" | grep -q 'ARM aarch64' || die "build did not produce an aarch64 binary: $(file "$bin")"

echo "==> build the canvas (wasm, trunk)"
"$repo/scripts/canvas-web.sh" >/dev/null
dist="$repo/target/canvas-dist"
[ -f "$dist/index.html" ] || die "the canvas build left no index.html in $dist"

echo "==> pi-gen at $ref"
mkdir -p "$work"
pigen="$work/pi-gen"
if [ ! -d "$pigen/.git" ]; then
    git clone -q https://github.com/RPi-Distro/pi-gen "$pigen"
fi
git -C "$pigen" fetch -q origin
git -C "$pigen" checkout -q "$ref" || git -C "$pigen" checkout -q "origin/$ref"
commit="$(git -C "$pigen" rev-parse HEAD)"
echo "pi-gen commit $commit"
release="$(sed -n 's/^RELEASE=\(.*\)$/\1/p' "$pigen/config" 2>/dev/null || true)"
# The binary is built against Trixie's glibc; the image must be Trixie too.
grep -q 'trixie' "$pigen/stage0/prerun.sh" "$pigen/config" "$pigen/README.md" 2>/dev/null \
    || echo "warning: this pi-gen does not mention trixie; the binary needs Trixie's glibc (check RELEASE)"

echo "==> the helmsman stage"
stage="$pigen/stage-helmsman"
rm -rf "$stage"
cp -R "$stage_src" "$stage"
files="$stage/00-install/files"
mkdir -p "$files/ui" "$files/seed"
install -m 755 "$bin" "$files/helmsman-plc"
cp -R "$dist/." "$files/ui/"
install -m 644 "$seed" "$files/seed/$seed_file"
install -m 644 "$repo/deploy/helmsman-plc.service" "$files/helmsman-plc.service"
install -m 644 "$repo/deploy/helmsman-firstboot.service" "$files/helmsman-firstboot.service"
install -m 755 "$repo/deploy/helmsman-firstboot.sh" "$files/helmsman-firstboot.sh"
printf 'PLC_SOURCE=--projects\nPLC_SEED=%s /etc/helmsman-plc/seed/%s\nPLC_SCAN_MS=10\nPLC_API=0.0.0.0:8080\nPLC_UI=/usr/local/share/helmsman-plc/ui\nPLC_RELAY_ARGS=\n' \
    "$seed_name" "$seed_file" > "$files/env"
{
    echo "helmsman-plc $(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "built $(date -u +%Y-%m-%dT%H:%M:%SZ) with pi-gen $commit"
    echo "seed $seed_name ($seed_file)"
    echo "binary sha256 $(shasum -a 256 "$bin" | cut -d' ' -f1)"
} > "$files/image-manifest"
chmod +x "$stage"/*.sh "$stage"/00-install/*.sh
# Only stage0-2 plus ours: a lite image, no desktop.
for s in stage3 stage4 stage5; do
    touch "$pigen/$s/SKIP" "$pigen/$s/SKIP_IMAGES"
done
touch "$pigen/stage2/SKIP_IMAGES"
cat > "$pigen/config" <<CONF
IMG_NAME=helmsman
RELEASE=trixie
TARGET_HOSTNAME=helmsman
FIRST_USER_NAME=helmsman
DISABLE_FIRST_BOOT_USER_RENAME=1
ENABLE_SSH=1
LOCALE_DEFAULT=en_US.UTF-8
KEYBOARD_KEYMAP=us
KEYBOARD_LAYOUT="English (US)"
TIMEZONE_DEFAULT=UTC
STAGE_LIST="stage0 stage1 stage2 stage-helmsman"
DEPLOY_COMPRESSION=xz
CONF

echo "==> pi-gen build (Docker; this takes a while)"
(cd "$pigen" && CONTINUE=1 PRESERVE_CONTAINER=0 ./build-docker.sh)

echo "==> result"
img="$(find "$pigen/deploy" -name '*.img.xz' -newer "$pigen/config" | head -1)"
[ -n "$img" ] || die "pi-gen left no .img.xz in $pigen/deploy"
out="$work/$(basename "$img")"
cp "$img" "$out"
shasum -a 256 "$out" > "$out.sha256"
cat "$files/image-manifest"
echo "image: $out"
echo "sha256: $(cut -d' ' -f1 "$out.sha256")"
echo "pin: write $commit into $ref_file if this is the build to keep"
echo "next: flash it (Raspberry Pi Imager or dd, your step), put relay-url and relay-token under helmsman/ on the boot partition, boot; docs/BOX.md"
