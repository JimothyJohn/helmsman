# CODESYS on a Raspberry Pi 5 as an interop target

Goal: a real, commercial EtherNet/IP adapter on the bench that this
project's originator can discover, read, and run Class 1 I/O against.

## Roles, so nothing gets built twice

This repo builds the **originator** (explicit client + I/O scanner).
CODESYS Control for Raspberry Pi ships its own EtherNet/IP Scanner *and*
Adapter. The useful pairing is:

| Box | Role | Software |
|---|---|---|
| Pi 5 | EtherNet/IP **adapter** (target) | CODESYS Control for Raspberry Pi 64 SL, EtherNet/IP Adapter device in the project |
| Mac Studio / CI / any Linux box | **originator** | `tcpethernet` (this repo) |

CODESYS as the adapter gives a second independent target next to OpENer,
from a vendor whose stack is conformance-tested. If you later want the
reverse (CODESYS scanning *our* code), that needs an adapter
implementation, which this repo does not have.

## What is verified vs not

From the CODESYS store page and datasheet (links in the session summary):

- The SL runtime supports Pi 5 from version 4.11.0.0; current is 4.19.0.0.
- 64-bit only. The runtime package includes the EtherNet/IP Scanner/Adapter.
- Without a licence it runs **two hours**, then stops; restart by hand.
  Fine for bench work. Commercial use needs a licence (store price $66).
- Installation is driven from the **CODESYS Development System**, which is a
  Windows application, via its Deploy Control SL tool over SSH.

Not verified: whether 4.19 supports Raspberry Pi OS 13 (Trixie). Every
current Pi OS Lite image is Trixie. One forum report says a new Pi OS broke
an older runtime. If the runtime will not start, reflash with an older
image via `PI_OS_URL=... scripts/pi-flash.sh`.

## Steps

1. **Flash** (Nick, native terminal): `scripts/pi-flash.sh /dev/diskN`.
   It refuses internal or large disks, verifies the image hash, and enables
   SSH with a one-time password it prints.
2. **Cable** the Pi to the office LAN (10.0.0.0/24). Its USB-C port only
   powers it; the Mac cannot reach it over that cable.
3. **Prep** (here, `! scripts/pi-prep.sh nick@raspberrypi.local`): key-only
   SSH, hostname `codesys-pi`, packages current, confirms 44818/2222 are
   free. If sudo asks for a password it prints the one line to run on the Pi.
4. **Install the runtime** (Windows PC with the CODESYS IDE): CODESYS
   Installer, add "CODESYS Control for Raspberry Pi 64 SL", then Tools ->
   Deploy Control SL against the Pi's IP.
5. **Project**: a Raspberry Pi 64 SL device, an Ethernet adapter on `eth0`,
   an EtherNet/IP Adapter under it with one input and one output assembly.
   Note the assembly instance numbers and sizes CODESYS assigns; they go
   into `docs/INTEROP.md`.
6. **Test from here** once it's running:
   `cargo run --example discover <pi-ip>` then the CLI's `get 1.1.7`.
