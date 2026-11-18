#!/usr/bin/env bash
# Boot a box image (scripts/pi-image.sh's, or any Raspberry Pi OS arm64
# image) under QEMU and check that it comes up: first boot pairs with a
# scripted relay by code, then the box's API answers on a forwarded port
# with its health answer and serves the canvas page (docs/BOX.md,
# "Checking an image"). CI runs it on the image the build just made
# (.github/workflows/box-image.yml); it runs the same way on a Mac or a
# Linux box with the tools below.
#
# Machine: qemu-system-aarch64 -M raspi3b, with the image's own kernel8.img
# and bcm2710-rpi-3-b.dtb, its SD card as the image, a USB network card
# (the Pi 3's NIC is on USB). Why not the others: `raspi4b` needs QEMU 9
# (Ubuntu's runner has 8.2) and has no working network; `virt` cannot
# take the image's kernel (the Pi kernel has no virtio driver built in and
# the image has no initramfs). The image's own kernel is the one a Pi runs.
# One quirk found by booting it: QEMU starts the BCM2835 watchdog armed and
# nothing feeds it, so the guest reset itself every ~9 s until the driver was
# kept out (initcall_blacklist=bcm2835_wdt_driver_init, not a Pi concern).
#
# Needs: qemu-system-aarch64, qemu-img, mtools (mcopy, mmd), python3, curl,
#        xz when IMAGE is .img.xz. EXPAND=1 (CI; Linux, sudo, e2fsprogs):
#        grow the root partition to 4 GB first, because pi-gen leaves the
#        image almost full and the box needs room to write its state.
# Does:  copy IMAGE under target/box-boot (never touches the original);
#        take the kernel and device tree off its boot partition; put the
#        pairing files on it (relay-url http://10.0.2.2:PORT, the
#        relay-allow-http marker, relay-box-id, pairing-code); start the
#        scripted relay (scripts/box-boot-relay.py); boot; wait until
#        https://127.0.0.1:HOST_PORT/api/health answers {"up":true,...} or
#        the deadline passes; then GET / (the canvas) and the relay's
#        record of one redeem. The serial console is target/box-boot/serial.log.
# Fails:  with the last lines of the serial log, on a deadline, on QEMU
#        exiting, on a missing answer, or when the code or the token shows
#        in the serial log.
#
# Usage: scripts/box-boot-test.sh IMAGE.img[.xz]
#        BOOT_DEADLINE=seconds (default 900)  HOST_PORT=18080  EXPAND=1
#        PAIR=0 skips the pairing files and the relay check (a stock image)
#        HEALTH=0 only waits for the login prompt (a stock image)
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
work="${WORK:-$repo/target/box-boot}"
deadline="${BOOT_DEADLINE:-900}"
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

rm -rf "$work"
mkdir -p "$work/boot"
img="$work/box.img"
case "$src" in
    *.xz) command -v xz >/dev/null || die "needs xz"; xz -dc "$src" > "$img" ;;
    *) cp "$src" "$img" ;;
esac

# Partition table: the boot (FAT) and root partitions' offset and size in
# bytes, from the MBR (a Pi OS image is MBR; pi-gen's too).
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
[ "$boot_off" -gt 0 ] && [ "$root_off" -gt "$boot_off" ] || die "unexpected partition table: $parts"
fat="$img@@$boot_off"
export MTOOLS_SKIP_CHECK=1

echo "==> kernel and device tree from the image's boot partition"
for f in kernel8.img bcm2710-rpi-3-b.dtb; do
    mcopy -n -i "$fat" "::$f" "$work/boot/$f" || die "the image's boot partition has no $f"
done

if [ "${EXPAND:-0}" = 1 ]; then
    echo "==> grow the root partition to 4 GB"
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
    sudo e2fsck -fy "${loop}p2" >/dev/null || [ $? -le 1 ]
    sudo resize2fs "${loop}p2" >/dev/null
    sudo losetup -d "$loop"
else
    # QEMU's SD card must be a power of two in size.
    size="$(wc -c < "$img" | tr -d ' ')"
    pow=$((1 << 30))
    while [ "$pow" -lt "$size" ]; do pow=$((pow * 2)); done
    [ "$pow" -eq "$size" ] || qemu-img resize -f raw "$img" "$pow" >/dev/null
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

serial="$work/serial.log"
: > "$serial"
echo "==> boot (deadline ${deadline}s); serial console: $serial"
qemu-system-aarch64 \
    -M raspi3b -m 1G -smp 4 \
    -kernel "$work/boot/kernel8.img" -dtb "$work/boot/bcm2710-rpi-3-b.dtb" \
    -append "rw earlycon=pl011,0x3f201000 console=ttyAMA0,115200 root=/dev/mmcblk0p2 rootfstype=ext4 rootwait net.ifnames=0 dwc_otg.fiq_fsm_enable=0 initcall_blacklist=bcm2835_wdt_driver_init systemd.show_status=1" \
    -drive "file=$img,format=raw,if=sd" \
    -device usb-net,netdev=n0 \
    -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$host_port-:8080" \
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
    case "$answer" in *'"build":"'?*) ;; *) fail "health carries no build: $answer" ;; esac
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
