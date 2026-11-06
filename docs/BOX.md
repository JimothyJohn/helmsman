# The box image

A Raspberry Pi OS image with Helmsman preinstalled: flash it, add two
files for the relay if you want the agent, boot, open the canvas. The
`box-sku` card's first cut: the image and its first boot, not the
enclosure or the bill of materials.

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
box's own certificate, fingerprint in Help › About); reads are open, every
change needs an engineer. A fresh box has no accounts: its role PINs work
until one exists (`helmsman-plc account add`, or `scripts/pi-account.sh`).
SSH is on with the default user `helmsman` (set the password with
Raspberry Pi Imager when flashing, or in `pi-gen`'s `FIRST_USER_PASS`).

## First boot: pairing with the relay

The relay issues a box its token out of band today (agent-relay's
`scripts/issue-box-token.sh`). To pair a flashed card, mount its boot
partition (FAT, readable anywhere) and put under `helmsman/`:

- `relay-url`: the relay's `https://` URL, one line;
- `relay-token`: the box's `hmr1_…` token, one line;
- `site` (optional): the site the box joins, 1 to 32 of `A-Z a-z 0-9 . _ -`;
- `box-name` (optional, needs `site`): the box's name on its site, at most
  48 of `A-Z a-z 0-9 _ -` (other characters become `-`).

The URL and the token go together: one without the other sets no relay.
A value the box would refuse is left out with a line in the journal, so a
bad pairing file never stops the PLC service.

`helmsman-firstboot` runs before the PLC service at every boot: it writes
the token into `/etc/helmsman-plc/relay-token` (root, 0600), sets
`PLC_RELAY_ARGS` in `/etc/helmsman-plc/env` (`--relay URL --relay-token
/run/credentials/helmsman-plc.service/relay-token [--site SITE [--box-name NAME]]`) and
removes the token from the boot partition. Without the files it does
nothing; add them later and `sudo systemctl start helmsman-firstboot`.
A pairing *code* the box exchanges for a token itself is relay work (the
relay has no such endpoint yet); the files are the skeleton's answer.

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
write it into `pi-gen.ref` to pin the build (the file ships naming the
`arm64` branch, which the script refuses to build from unless
`PI_GEN_UNPINNED=1`, so the first build is a deliberate choice).

Why pi-gen and not rpi-image-gen: pi-gen is the builder Raspberry Pi OS
itself is made with, has a Docker path that runs on a Mac, and a custom
stage is three small files; rpi-image-gen is newer and declarative but
wants a Debian host. Both are the Raspberry Pi Foundation's; the choice
is about building here.

## Checking an image

The skeleton checks that pi-gen finished and the image exists, and prints
the manifest of what went in. It does not boot the image. To check one by
hand:

1. Flash it to a card (Raspberry Pi Imager, or `dd`: your step, with the
   device verified).
2. Boot a Pi 4 or 5 on it with Ethernet. `ssh helmsman@helmsman.local`.
3. `systemctl status helmsman-plc helmsman-firstboot`;
   `journalctl -u helmsman-plc | grep 'API token'` shows the first start.
4. Open `https://helmsman.local:8080/`, accept the certificate, open the
   seeded project, deploy it as an engineer.

A boot in QEMU in CI (`qemu-system-aarch64` with the image's kernel and
device tree) is the next step for this card, not done here.

## Not in the skeleton

The enclosure, the bill of materials, a pairing code exchanged with the
relay, OTA of the box itself (`fleet-console`), signed images.
