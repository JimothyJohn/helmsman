#!/usr/bin/env bash
# Boot a box image (scripts/pi-image.sh's, or any Raspberry Pi OS arm64
# image) under QEMU and check that it comes up: first boot pairs with a
# scripted relay by code, then the box's API answers on a forwarded port
# with its health answer and serves the canvas page (docs/BOX.md,
# "In CI"). CI runs it on the image the build just made
# (.github/workflows/box-image.yml); the pieces run on a Mac too.
#
# What this proves, and what it does not: the image's userland (systemd, the
# units, helmsman-firstboot, the box and its canvas) and the box's first
# boot on a generic arm64 machine. It does not run the Pi's own kernel,
# firmware or device tree: the first boot on a real Pi is still owed.
#
# Machine: qemu-system-aarch64 -M virt -cpu cortex-a72 -m 2G -smp 4, the
# image's disk as an NVMe drive, virtio-net with user networking (DHCP from
# QEMU, the API's port forwarded). The kernel is Debian trixie's cloud
# arm64 kernel (deploy/pi-image/qemu-kernel.env, pinned by SHA-256). Why
# not QEMU's Pi machines: raspi3b stalls the whole guest in sysinit after
# about 30 KB of console output (CI run 44347130885 and local runs, cause
# not found), and raspi4b needs QEMU 9 and has no network.
#
# Modules, not an initramfs: that kernel has NVMe and ext4 built in, so the
# root filesystem mounts without an initramfs, but virtio-net, vfat (the
# image's /boot/firmware), loop, zram and nftables are modules, and the Pi
# kernel's modules in the image do not match it. So the Debian package's
# /usr/lib/modules/VERSION is copied into the CI copy of the image and
# depmod run there (needs Linux, sudo, kmod: EXPAND=1, which CI sets). The
# shipped image is never touched. The image's /etc/fstab names its
# partitions by PARTUUID, which the NVMe disk carries unchanged (the same
# MBR), so it needs no edit.
#
# Needs: qemu-system-aarch64, qemu-img, mtools (mcopy, mmd), python3, curl,
#        bsdtar or dpkg-deb, xz when IMAGE is .img.xz. EXPAND=1 (CI; Linux,
#        sudo, e2fsprogs, kmod): grow the root partition to 4 GB (pi-gen
#        leaves the image almost full) and inject the modules.
# Does:  fetch and verify the kernel package; copy IMAGE under
#        target/box-boot (never touches the original); put the pairing files
#        on its boot partition (relay-url http://10.0.2.2:PORT, the
#        relay-allow-http marker, relay-box-id, pairing-code); start the
#        scripted relay (scripts/box-boot-relay.py); boot; wait until
#        https://127.0.0.1:HOST_PORT/api/health answers {"up":true,...} or
#        the deadline passes; then GET / (the canvas) and the relay's
#        record of one redeem. The serial console is target/box-boot/serial.log.
# Fails: with the last lines of the serial log, on a deadline, on QEMU
#        exiting, on a missing answer, or when the code or the token shows
#        in the serial log.
#
# Usage: scripts/box-boot-test.sh IMAGE.img[.xz]
#        BOOT_DEADLINE=seconds (default 1500)  HOST_PORT=18080  EXPAND=1
#        PAIR=0 skips the pairing files and the relay check (a stock image)
#        HEALTH=0 only waits for the login prompt (a stock image)
#        EXPECT_BUILD=STAMP the build the box must report (default: this
#        checkout's `git describe --always --abbrev=9`; empty skips the match)
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
work="${WORK:-$repo/target/box-boot}"
cache="${CACHE:-$repo/target/box-boot-cache}"
deadline="${BOOT_DEADLINE:-1500}"
host_port="${HOST_PORT:-18080}"
pair="${PAIR:-1}"
health="${HEALTH:-1}"
code="TESTC-ODE12"
box_id="ci-box"

die() { echo "error: $*" >&2; exit 1; }
for tool in qemu-system-aarch64 qemu-img mcopy mmd python3 curl; do
    command -v "$tool" >/dev/null || die "needs $tool"
done
[ $# -eq 1 ] || die "usage: $0 IMAGE.img[.xz]"
src="$1"
[ -f "$src" ] || die "no image at $src"

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
# The files of a .deb into a directory.
extract_deb() {
    if command -v dpkg-deb >/dev/null; then
        dpkg-deb -x "$1" "$2"
    else
        command -v bsdtar >/dev/null || die "needs dpkg-deb or bsdtar"
        bsdtar -xOf "$1" 'data.tar.*' | bsdtar -xf - -C "$2"
    fi
}

# ---- the kernel: fetched, checked against the pin, unpacked
# shellcheck source=../deploy/pi-image/qemu-kernel.env
. "$repo/deploy/pi-image/qemu-kernel.env"
mkdir -p "$cache"
deb="$cache/$(basename "$KERNEL_URL")"
if [ ! -f "$deb" ] || [ "$(sha256 "$deb")" != "$KERNEL_SHA256" ]; then
    echo "==> fetch $KERNEL_URL"
    curl -fL --retry 3 -o "$deb" "$KERNEL_URL" || die "cannot fetch the kernel package (Debian drops superseded versions: bump deploy/pi-image/qemu-kernel.env)"
fi
[ "$(sha256 "$deb")" = "$KERNEL_SHA256" ] || die "the kernel package does not match the pinned SHA-256"
kdir="$cache/kernel-$KERNEL_VERSION"
if [ ! -f "$kdir/boot/vmlinuz-$KERNEL_VERSION" ]; then
    rm -rf "$kdir"
    mkdir -p "$kdir"
    extract_deb "$deb" "$kdir"
fi
kernel="$kdir/boot/vmlinuz-$KERNEL_VERSION"
[ -f "$kernel" ] || die "the package has no $kernel"

# ---- the image: a copy
rm -rf "$work"
mkdir -p "$work"
img="$work/box.img"
case "$src" in
    *.xz) command -v xz >/dev/null || die "needs xz"; xz -dc "$src" > "$img" ;;
    *) cp "$src" "$img" ;;
esac

# Partition table: the boot (FAT) and root partitions' offset in bytes, from
# the MBR (a Pi OS image is MBR; pi-gen's too).
parts="$(python3 -I - "$img" <<'PY'
import struct, sys
with open(sys.argv[1], "rb") as f:
    mbr = f.read(512)
if mbr[510:512] != b"\x55\xaa":
    sys.exit("not an MBR image")
out = []
for i in (0, 1):
    e = mbr[446 + 16 * i: 462 + 16 * i]
    start, size = struct.unpack("<II", e[8:16])
    out += [start * 512, size * 512]
print(*out)
PY
)"
read -r boot_off _ root_off _ <<< "$parts"
if [ "$boot_off" -le 0 ] || [ "$root_off" -le "$boot_off" ]; then die "unexpected partition table: $parts"; fi
fat="$img@@$boot_off"
export MTOOLS_SKIP_CHECK=1

if [ "${EXPAND:-0}" = 1 ]; then
    echo "==> grow the root partition to 4 GB, add the kernel's modules"
    qemu-img resize -f raw "$img" 4G >/dev/null
    python3 -I - "$img" "$root_off" <<'PY'
import struct, sys
path, root_off = sys.argv[1], int(sys.argv[2])
with open(path, "r+b") as f:
    total = f.seek(0, 2) // 512
    f.seek(446 + 16 + 12)
    f.write(struct.pack("<I", total - root_off // 512))
PY
    loop="$(sudo losetup --find --show -P "$img")"
    mnt="$work/rootfs"
    mkdir -p "$mnt"
    cleanup_loop() { sudo umount "$mnt" 2>/dev/null || true; sudo losetup -d "$loop" 2>/dev/null || true; }
    trap cleanup_loop EXIT
    sudo e2fsck -fy "${loop}p2" >/dev/null || [ $? -le 1 ]
    sudo resize2fs "${loop}p2" >/dev/null
    sudo mount "${loop}p2" "$mnt"
    sudo mkdir -p "$mnt/usr/lib/modules"
    sudo rm -rf "${mnt:?}/usr/lib/modules/$KERNEL_VERSION"
    sudo cp -a "$kdir/usr/lib/modules/$KERNEL_VERSION" "$mnt/usr/lib/modules/"
    sudo depmod -b "$mnt" "$KERNEL_VERSION"
    sudo umount "$mnt"
    sudo losetup -d "$loop"
    trap - EXIT
else
    echo "==> EXPAND is not set: the kernel's modules are not added to the image."
    echo "    The disk mounts (NVMe is built in) but /boot/firmware, the network and zram need them:"
    echo "    expect emergency mode on a real run; this is for the login-prompt check of a stock image."
fi

pids=()
cleanup() {
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
}
trap cleanup EXIT

token="hmr1_$(python3 -I -c 'import secrets; print(secrets.token_hex(32))')"
if [ "$pair" = 1 ]; then
    echo "==> scripted relay"
    python3 -I "$repo/scripts/box-boot-relay.py" "$work/relay.port" "$work/relay.events" "$code" "$box_id" "$token" &
    pids+=($!)
    for _ in $(seq 1 50); do [ -s "$work/relay.port" ] && break; sleep 0.1; done
    [ -s "$work/relay.port" ] || die "the scripted relay did not start"
    relay_port="$(cat "$work/relay.port")"
    echo "==> pairing files on the boot partition (relay at 10.0.2.2:$relay_port)"
    mkdir -p "$work/pairing"
    printf 'http://10.0.2.2:%s\n' "$relay_port" > "$work/pairing/relay-url"
    : > "$work/pairing/relay-allow-http"
    printf '%s\n' "$box_id" > "$work/pairing/relay-box-id"
    printf '%s\n' "$code" > "$work/pairing/pairing-code"
    mmd -i "$fat" ::helmsman 2>/dev/null || true
    for f in relay-url relay-allow-http relay-box-id pairing-code; do
        mcopy -D o -i "$fat" "$work/pairing/$f" "::helmsman/$f"
    done
fi

accel=tcg
cpu=cortex-a72
if [ "$(uname -m)" = aarch64 ] && [ -w /dev/kvm ]; then accel=kvm; cpu=host; fi
serial="$work/serial.log"
: > "$serial"
echo "==> boot on -M virt ($accel, deadline ${deadline}s); serial console: $serial"
qemu-system-aarch64 \
    -M virt -accel "$accel" -cpu "$cpu" -m 2G -smp 4 \
    -kernel "$kernel" \
    -append "console=ttyAMA0 root=/dev/nvme0n1p2 rootfstype=ext4 rootwait rw net.ifnames=0" \
    -drive "file=$img,format=raw,if=none,id=d0" -device nvme,drive=d0,serial=helmsman \
    -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$host_port-:8080" -device virtio-net-pci,netdev=n0 \
    -display none -monitor none -serial "file:$serial" -no-reboot &
qemu_pid=$!
pids+=("$qemu_pid")

fail() {
    echo "error: $*" >&2
    echo "---- last of the serial console ($serial) ----" >&2
    tail -n 80 "$serial" >&2 || true
    exit 1
}

started="$(date +%s)"
if [ "$health" = 1 ]; then
    answer=""
    while :; do
        kill -0 "$qemu_pid" 2>/dev/null || fail "QEMU exited before the box answered"
        answer="$(curl -sk --max-time 5 "https://127.0.0.1:$host_port/api/health" 2>/dev/null || true)"
        case "$answer" in *'"up":true'*) break ;; esac
        [ "$(( $(date +%s) - started ))" -lt "$deadline" ] || fail "no health answer in ${deadline}s"
        sleep 5
    done
    echo "==> up after $(( $(date +%s) - started ))s: $answer"
    # The box must report the commit the image was built from (pi-image.sh
    # stamps it): the same `git describe` as crates/build_stamp.rs, in this
    # checkout. EXPECT_BUILD overrides it; empty skips the match.
    expect="${EXPECT_BUILD-$(git -C "$repo" describe --always --abbrev=9 2>/dev/null || true)}"
    got="$(printf '%s' "$answer" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("build",""))' 2>/dev/null || true)"
    if [ -n "$expect" ] && [ "$got" != "$expect" ]; then fail "the box reports build \"$got\", the image was built from \"$expect\""; fi
    case "$got" in "" | unknown) fail "the box reports no build stamp: $answer" ;; esac
    page="$(curl -sk --max-time 20 -w '\n%{http_code} %{content_type}' "https://127.0.0.1:$host_port/")" || fail "GET / failed"
    status="$(printf '%s' "$page" | tail -n 1)"
    case "$status" in "200 text/html"*) ;; *) fail "GET / answered $status, not 200 text/html" ;; esac
    printf '%s' "$page" | grep -qi '<html' || fail "GET / is not the canvas page"
    echo "==> canvas page served ($status)"
else
    while ! grep -q 'login:' "$serial"; do
        kill -0 "$qemu_pid" 2>/dev/null || fail "QEMU exited before a login prompt"
        [ "$(( $(date +%s) - started ))" -lt "$deadline" ] || fail "no login prompt in ${deadline}s"
        sleep 5
    done
    echo "==> login prompt after $(( $(date +%s) - started ))s"
fi

if [ "$pair" = 1 ] && [ "$health" = 1 ]; then
    [ "$(grep -c '^redeem ok$' "$work/relay.events" || true)" = 1 ] || fail "the relay did not see exactly one good redeem: $(tr '\n' ';' < "$work/relay.events")"
    [ "$(wc -l < "$work/relay.events" | tr -d ' ')" = 1 ] || fail "the relay saw other tries: $(tr '\n' ';' < "$work/relay.events")"
    echo "==> first boot paired by code: one redeem at the scripted relay"
fi
if grep -qF -e "$code" -e "TESTC" -e "$token" "$serial"; then
    fail "the pairing code or the token is in the serial log"
fi
echo "==> ok"
