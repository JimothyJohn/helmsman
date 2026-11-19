# The box image

A Raspberry Pi OS image with Helmsman preinstalled: flash it, pair it with
the relay if you want the agent (a one-time code and three small files),
boot, open the canvas. CI builds the image and boots it under QEMU. The
`box-sku` card so far: the image, its first boot and its boot check, not
the enclosure or the bill of materials.

## What is on it

Raspberry Pi OS Lite (arm64, Trixie) plus what `scripts/pi-deploy-plc.sh`
installs over SSH on a Pi you already have:

| path | what |
| --- | --- |
| `/usr/local/bin/helmsman-plc` | the box, built for linux/arm64 against Trixie's glibc |
| `/usr/local/share/helmsman-plc/ui/` | the canvas, served at `/` on the API's port |
| `/etc/helmsman-plc/env` | the unit's settings: projects mode, the seed, scan period, `PLC_API=0.0.0.0:8080`, `PLC_RELAY_ARGS` |
| `/etc/helmsman-plc/seed/NAME.st` | the project a first install seeds (default `line`); nothing runs until an engineer deploys it |
| `/etc/helmsman-plc/relay-token` | the relay token, root-only, empty until paired; the unit loads it as a systemd credential |
| `/etc/helmsman-plc/image-manifest` | which helmsman commit, which pi-gen commit, when, the binary's SHA-256 |
| `/etc/systemd/system/helmsman-plc.service` | the same unit as a deployed Pi (`deploy/helmsman-plc.service`) |
| `/etc/systemd/system/helmsman-firstboot.service` | the pairing step, below |
| `/boot/firmware/helmsman/` | where the pairing files go; a README there says the same |

The API and the canvas listen on the LAN (`0.0.0.0:8080`, HTTPS with the
box's own certificate, fingerprint in Help › About); every read needs a
sign in (only `/api/health` answers a bare poll), every change needs an
engineer. A fresh box has no accounts: its role PINs work
until one exists (`helmsman-plc account add`, or `scripts/pi-account.sh`).
SSH is on, but the image carries no password and asks nobody for one: the
two first-boot wizards that would hold a headless box at its console
(systemd's, and Raspberry Pi OS's first-user step) are masked, and locale,
keyboard, timezone and hostname come from the build. The box is reached
through its API after pairing. To get a shell, put a cloud-init `user-data`
file on the boot partition before first boot, with a user and your public
SSH key (what Raspberry Pi Imager's settings write for you); cloud-init is
on the image and reads it. No password is baked in: the build leaves
pi-gen's `FIRST_USER_PASS` unset, and a password in a public image is no
password. CI does not exercise this path.

## First boot: pairing with the relay

A box gets its relay token one of two ways. Either way, mount the flashed
card's boot partition (FAT, readable anywhere) and put these under
`helmsman/`:

- `relay-url`: the relay's `https://` URL, one line;
- the box's credential, one of:
  - `relay-token`: the box's `hmr1_…` token, one line, issued by the
    operator out of band (agent-relay's `scripts/issue-box-token.sh`);
  - `pairing-code` and `relay-box-id`: a one-time code the operator made
    for this box (below) and the id it was made for, one line each. The
    box trades the code for its own token at first boot, so no token is
    ever copied onto the card.
- `site` (optional): the site the box joins, 1 to 32 of `A-Z a-z 0-9 . _ -`;
- `box-name` (optional, needs `site`): the box's name on its site, at most
  48 of `A-Z a-z 0-9 _ -` (other characters become `-`).

The URL and a token go together: one without the other sets no relay.
A value the box would refuse is left out with a line in the journal, so a
bad pairing file never stops the PLC service.

`helmsman-firstboot` runs before the PLC service at every boot: it writes
the token into `/etc/helmsman-plc/relay-token` (root, 0600), sets
`PLC_RELAY_ARGS` in `/etc/helmsman-plc/env` (`--relay URL --relay-token
/run/credentials/helmsman-plc.service/relay-token [--site SITE [--box-name NAME]]`) and
removes the token, or the code, from the boot partition. Without the files
it does nothing; add them later and `sudo systemctl start helmsman-firstboot`.

### Pairing by code

The operator makes the code on the relay, for the box id the box will
use (agent-relay `docs/relay-protocol.md`, "Pairing by code";
`scripts/pair-code.sh BOX-ID ADMIN-TOKEN-FILE` there). The code is ten
letters and digits, such as `7K3QM-X9R2D`, works once, for that id, for ten
minutes. Put it on the card with the relay URL and the id:

```
helmsman/relay-url      https://RELAY-HOST
helmsman/relay-box-id   plc-7
helmsman/pairing-code   7K3QM-X9R2D
```

At boot `helmsman-firstboot` runs `helmsman-plc pair`, which reads the code
from the file (never from the command line), posts it to the relay's
`/v1/pair`, and writes the token it gets back to
`/etc/helmsman-plc/relay-token` with mode 0600. Neither the code nor the
token is printed or logged. The code file is deleted once it has been tried
for good, and what happens to it depends on the answer:

| the relay says | the code file | the box |
| --- | --- | --- |
| a token | removed | paired; the PLC service starts with the relay |
| refused (wrong, used, expired, another id) | removed, so the next boot does not spend tries | starts without a relay; make a new code and run `sudo systemctl start helmsman-firstboot` |
| nothing reachable, 429, a relay error | kept | starts without a relay; the next start tries again (up to 30 s of retries when the relay is unreachable or says to slow down) |

A `relay-token` file beside a code wins and the code is dropped. An
`http://` relay URL is refused unless an empty `relay-allow-http` file is
beside it: a relay on the plant's own network has no certificate, and with
the file the code and the token cross that network in the clear. The
operator reads the code out or sends it by another channel; the console
prompt that some images offer is left out on purpose (a headless box would
sit at it).

## Building the image

```
scripts/pi-image.sh [SEED.st]
```

Needs Docker (Apple silicon builds arm64 natively), `trunk`, `git`,
about 15 GB and an hour. It builds the binary (the pinned `rust:*-slim-trixie`
image, as the Pi deploy does) and the canvas, clones
[pi-gen](https://github.com/RPi-Distro/pi-gen) at the commit or tag in
`deploy/pi-image/pi-gen.ref` into `target/pi-image/pi-gen`, adds the stage
`deploy/pi-image/stage-helmsman` after stage2 (a Lite image, no desktop),
runs pi-gen's own Docker build, and leaves `target/pi-image/*.img.xz` with
a `.sha256` beside it. The script prints the pi-gen commit it built from;
the ref is pinned to the commit CI built from (2026-11-19), and the script
refuses a branch name unless `PI_GEN_UNPINNED=1`. To move it, put the commit
the script prints into `pi-gen.ref` and let CI build it.

Why pi-gen and not rpi-image-gen: pi-gen is the builder Raspberry Pi OS
itself is made with, has a Docker path that runs on a Mac, and a custom
stage is three small files; rpi-image-gen is newer and declarative but
wants a Debian host. Both are the Raspberry Pi Foundation's; the choice
is about building here.

## Checking an image

### In CI

`.github/workflows/box-image.yml` builds the image and boots it, on an
arm64 runner (pi-gen builds natively there) with a 150 minute cap. It runs
when the image's inputs change (`deploy/`, the build and boot scripts, the
pairing client), every Monday, and on demand. It is not a required check.
`scripts/box-boot-test.sh IMAGE` is the same boot on your own machine, with
`qemu-system-aarch64`, `mtools`, `python3`, `curl` and `bsdtar` or
`dpkg-deb`:

1. It fetches Debian trixie's arm64 cloud kernel package (pinned by version
   and SHA-256 in `deploy/pi-image/qemu-kernel.env`) and unpacks it.
2. It copies the image and puts `relay-url` (a scripted relay on the host),
   `relay-allow-http`, `relay-box-id` and `pairing-code` under `helmsman/`
   on the copy's boot partition. In CI (`EXPAND=1`, which needs Linux and
   sudo) it also grows the copy's root partition to 4 GB and adds the
   kernel package's modules to it (below).
3. It boots `qemu-system-aarch64 -M virt -cpu cortex-a72 -m 2G -smp 4` with
   the Debian kernel, the image's disk as an NVMe drive, and virtio-net with
   QEMU's user networking (DHCP from QEMU, host port 18080 forwarded to the
   box's 8080).
4. `scripts/box-boot-relay.py` plays the relay: it hands out one token for
   the one code, once, for the one id, and records it.
5. The script polls `https://127.0.0.1:18080/api/health` until it answers
   `{"up":true,"build":...}` or 25 minutes pass, then fetches `/` and checks
   it is the canvas page, that the relay saw exactly one redeem, and that
   neither the code nor the token is in the serial log.

A failure prints the end of the serial console, and CI keeps the whole
console and the relay's record as the `box-boot` artifact.

**What this proves, and what it does not.** It proves the image's userland
and the box's first boot on a generic arm64 machine: the root filesystem,
systemd and the units, `helmsman-firstboot` redeeming a pairing code, the
PLC service coming up, the API and the canvas answering. It does not run
the Pi's own kernel, firmware, bootloader or device tree, a Pi's network
hardware, or a Pi's SD card. The first boot of this image on a real Pi is
still owed (the `box-sku` card).

**Why this machine.** QEMU's Pi machines were tried first and set aside:
`raspi3b` (the one Ubuntu's QEMU 8.2 can boot the image's own kernel on)
froze the whole guest in sysinit after about 30 KB of console output, on a
stock Raspberry Pi OS image as well as on this one, with the vCPUs idle and
the network silent from then on; the cause was not found (CI run
44347130885; not the first-boot wizard, DMA, audit or threaded emulation).
`raspi4b` needs QEMU 9 and has no working network. The generic `virt`
machine boots the same root filesystem with a stock Debian kernel, which is
what a Raspberry Pi OS userland (Debian trixie) runs on anyway.

**Modules, and why not an initramfs.** The Debian cloud kernel has NVMe and
ext4 built in, so the root filesystem mounts with no initramfs. Its
virtio-net driver, and vfat (the image's `/boot/firmware`), loop, zram and
nftables, are modules, and the image carries the Pi kernel's modules, which
do not match. The script therefore copies the Debian package's
`/usr/lib/modules/VERSION` into the CI copy of the image by loop mount and
runs `depmod` there. An initramfs would need building for another
architecture's userland on the runner. The image's `/etc/fstab` names its
partitions by PARTUUID, and the NVMe disk carries the image's partition
table, so it needs no change. The shipped image is never touched. Without
`EXPAND=1` (a Mac) the modules are not added, the boot ends in emergency
mode at `/boot/firmware`, and only the kernel, the disk and the console are
checked.

### By hand

1. Flash it to a card (Raspberry Pi Imager, or `dd`: your step, with the
   device verified).
2. Boot a Pi 4 or 5 on it with Ethernet. `ssh helmsman@helmsman.local`.
3. `systemctl status helmsman-plc helmsman-firstboot`;
   `journalctl -u helmsman-plc | grep 'API token'` shows the first start.
4. Open `https://helmsman.local:8080/`, accept the certificate, open the
   seeded project, deploy it as an engineer.

## Not built

The enclosure, the bill of materials, OTA of the box itself
(`fleet-console`), signed images, and the deployed relay's side of pairing
by code (written, in a draft PR to agent-relay, not deployed).
