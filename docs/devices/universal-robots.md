# Universal Robots (e-Series) as a fieldbus device

A UR is available for the bench (Nick, 2026-11-10; TODO.md `ur-bringup`).
It is the one device that speaks every protocol the box has a driver for.
Nothing here has been tried against a robot; sources are in
docs/SOURCES.md ("Universal Robots fieldbus files").

## PROFINET IO device

Enabled under Installation > Fieldbus > PROFINET (e-Series guide,
PolyScope 5.12); on PolyScope X under Settings > Services. From
`GSDML-V2.45-UR-PROFIsafe-20251201.xml`:

| | |
| --- | --- |
| Vendor / device id | 0x0361 / 0x0001 |
| Default station name | `URIODev1` as the GSDML writes it; a station name on the wire is lowercase |
| RT class, startup | RT class 1; `Advanced;Legacy` |
| Fastest cycle | `MinDeviceInterval` 256 x 31.25 us = 8 ms |
| Access point | slot 0, module 0x1: submodule 0x1 (subslot 1), an interface submodule 0x8000 (subslot 0x8000) and a port submodule 0x8001 (subslot 0x8001) |

Modules, each with one submodule 0x0001 in subslot 1, each allowed in
exactly the slot shown:

| slot | module | name | in | out |
| --- | --- | --- | --- | --- |
| 1 | 0x30 | State | 32 | |
| 2 | 0x31 | IO | 68 | |
| 3 | 0x32 | Joints | 104 | |
| 4 | 0x33 | TCP | 76 | |
| 5 | 0x34 | General purpose bit registers | 8 | |
| 6 | 0x35 | General purpose int registers | 96 | |
| 7 | 0x36 | General purpose float registers | 96 | |
| 8 | 0x40 | Robot IO | | 24 |
| 9 | 0x41 | General purpose registers 1 | | 100 |
| 10 | 0x42 | General purpose registers 2 | | 100 |
| 11 | 0x3B000200 | PROFIsafe (submodule 0x3B000201) | | |
| 12 | 0x43 | Robot command | 256 | 256 |
| 13 | 0x37 | AUX | 88 | |
| 14 | 0x44 | Robot control | | 40 |

Slots 1..10 are the 480 input and 224 output bytes of UR's layout
document (`pn-iomessage.pdf`). No module has parameter records.

Open for the bench: whether the robot accepts a connection that leaves
the interface and port submodules out (the device line below does), and
whether this unit's PROFINET needs a licence.

```text
device ur profinet-io uriodev1 if=en0 vendor=0x0361 device=0x0001 slots=0:0x1/1:0x1,1:0x30/1:0x1:i32,5:0x34/1:0x1:i8,6:0x35/1:0x1:i96,8:0x40/1:0x1:o24,9:0x41/1:0x1:o100
```

UR's pages say the robot stops its program when the PROFINET controller
goes away, and that PROFINET and EtherNet/IP cannot be enabled together.

## EtherNet/IP adapter

From `UniversalRobot.eds` (VendCode 1399, ProdType 43, ProdCode 1,
revision 1.1): input assembly 100 (0x64), output assembly 112 (0x70),
the exclusive-owner path `20 04 24 01 2C 70 2C 64`. The sizes (480 and
224 bytes) and the RPI range (from 2 ms) are as a research pass read
them from the EDS and UR's `eip-iomessage.pdf` on 2026-11-10; the box's
EDS import will say what it resolves to.

## Modbus TCP server

Port 502, always on. As UR's "Modbus Server" article and register table
describe it (read by a research pass on 2026-11-10, not re-read): function
codes 1..6, 15 and 16; registers 0..33 the robot's I/O, 128..255 general
purpose (read and write), robot state from 256, tool state from 768; the
same I/O as coils; every value an unsigned 16-bit word, so a signed
quantity (a joint angle in mrad, a TCP position in tenths of a
millimetre) is converted by the reader; the unit id is ignored.

## URSim is not a stand-in

Forum reports (no UR statement found): the simulator's PROFINET part is
not installed, its EtherNet/IP adapter works in some versions and not in
others, and its Modbus registers read 65535 and take no writes. The
bring-up needs the robot.
