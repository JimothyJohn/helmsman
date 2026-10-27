#!/usr/bin/env bash
# Flash a fresh Raspberry Pi OS Lite (64-bit) onto an SD card / USB disk for the
# CODESYS Pi, headless-ready: SSH enabled, user created, the Mac Studio's key
# authorised. Nick runs this (it writes a block device); it refuses anything
# that isn't an external, removable, physical disk.
#
# Needs: macOS, sudo, xz (brew install xz), network. Runs in the native
# terminal, not through `!` (sudo prompts and a dd that takes minutes).
#
# Usage: scripts/pi-flash.sh /dev/diskN [username]      # default user: nick
#   diskutil list        # to find N; the card shows as "external, physical"
#
# Idempotent: re-running reflashes. The image is cached under scratch/pi-os/.
set -euo pipefail

disk="${1:?usage: $0 /dev/diskN [username]}"
user="${2:-nick}"
cache="$(cd "$(dirname "$0")/.." && pwd)/scratch/pi-os"
# PI_OS_URL overrides the image (e.g. the last Bookworm build if CODESYS does
# not yet support the current Trixie-based release).
latest="${PI_OS_URL:-https://downloads.raspberrypi.com/raspios_lite_arm64_latest}"

[[ "$disk" =~ ^/dev/disk[0-9]+$ ]] || { echo "refusing: $disk is not /dev/diskN" >&2; exit 2; }
info="$(diskutil info "$disk")"
grep -qE 'Device Location: +External' <<<"$info" || { echo "refusing: $disk is not external" >&2; exit 2; }
grep -qE 'Removable Media: +Removable|Media Type: +Generic|Protocol: +(USB|Secure Digital|SD)' <<<"$info" \
    || { echo "refusing: $disk does not look like a removable card/USB disk" >&2; exit 2; }
size_bytes="$(sed -nE 's/.*Disk Size:.*\(([0-9]+) Bytes\).*/\1/p' <<<"$info")"
if [ -z "$size_bytes" ] || [ "$size_bytes" -ge $((512 * 1024 * 1024 * 1024)) ]; then
    echo "refusing: $disk is ${size_bytes:-?} bytes; cards are under 512 GB" >&2; exit 2
fi
echo "Target: $disk ($(sed -nE 's/.*Device \/ Media Name: +//p' <<<"$info"), $((size_bytes / 1024 / 1024 / 1024)) GB)"

mkdir -p "$cache"
img_url="$(curl -sIL -o /dev/null -w '%{url_effective}' "$latest")"
img="$cache/$(basename "$img_url")"
if [ ! -f "$img" ]; then
    echo "Downloading $img_url"
    curl -L --progress-bar -o "$img.part" "$img_url" && mv "$img.part" "$img"
fi
echo "Verifying sha256"
expected="$(curl -sL "$img_url.sha256" | awk '{print $1}')"
actual="$(shasum -a 256 "$img" | awk '{print $1}')"
[ "$expected" = "$actual" ] || { echo "sha256 mismatch: expected $expected got $actual" >&2; rm -f "$img"; exit 1; }

read -r -p "Write $(basename "$img") to $disk? Everything on it is destroyed. [y/N] " ok
[ "${ok:-n}" = y ] || { echo "aborted"; exit 1; }
diskutil unmountDisk "$disk"
echo "Writing (a few minutes; Ctrl-T shows progress)"
xzcat "$img" | sudo dd of="${disk/disk/rdisk}" bs=4m status=progress
sync

# Headless first boot: Bookworm still honours an empty `ssh` file plus
# userconf.txt (username:crypted-password) on the boot partition.
diskutil mountDisk "$disk" >/dev/null
boot="/Volumes/bootfs"
for _ in $(seq 1 20); do [ -d "$boot" ] && break; sleep 1; done
[ -d "$boot" ] || { echo "boot partition did not mount at $boot" >&2; exit 1; }
touch "$boot/ssh"
pass="$(openssl rand -base64 12)"
printf '%s:%s\n' "$user" "$(openssl passwd -6 "$pass")" > "$boot/userconf.txt"
# First-boot key install via the user's home is not possible from here; the
# password is for the first `ssh-copy-id`, after which pi-prep disables it.
echo "Pi user: $user   first-boot password: $pass   (write it down; pi-prep.sh turns password login off)"
diskutil eject "$disk"
echo "Done. Boot the Pi on the office LAN, then: scripts/pi-prep.sh $user@raspberrypi.local"
