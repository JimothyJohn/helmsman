# Universal Robots (e-Series) as a fieldbus device

A UR is available for the bench (Nick, 2026-11-10; TODO.md `ur-bringup`).
It is the one device that speaks every protocol the box has a driver for.
Nothing here has been tried against a robot; Modbus TCP has been against
UR's simulators (below). Sources are in docs/SOURCES.md ("Universal
Robots fieldbus files", "Universal Robots Modbus register table").

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

Port 502, always on, the unit id ignored. Addresses below are from UR's
register table (`ModBus Server Data.pdf`, docs/SOURCES.md "Universal
Robots Modbus register table"); what the simulators answered is in the
next section. The profile is `universal-robots-e-series.device` (this
folder) and `ur-modbus/` is a program that runs on it.

- Registers (FC 3 and 4 alike): 0..33 the control box and tool I/O,
  128..255 general purpose (read and write; a robot program reads them
  with `read_port_register`), robot state from 256, TCP from 400, tool
  state from 768. Bits (FC 1 and 2 alike): 0..9 the box's eight digital
  inputs and the tool's two, 16..25 the outputs, 128..143 the
  configurable I/O, 260..265 the state flags, 300..427 and 500..627 the
  general purpose booleans shared with RTDE.
- The table says "all values are unsigned" and gives `val - 65535` for a
  signed one. That is one off: two's complement is `val - 65536` for
  `val >= 32768`, so `65535` is -1 (the simulators' TCP rx of -1 mrad
  read `65535`). The profile reads the signed ones as `i16`.
- Every address `x` also answers at 10000 + x, 20000 + x, 30000 + x and
  40000 + x, and 9999 is 0 (UR's page, for Siemens clients; the
  simulators agreed).

## The simulators (2026-11-20)

URSim e-Series 5.26.0 (`universalrobots/ursim_e-series`, amd64 under
Docker's Rosetta emulation; its Xvfb crashes there, so no PolyScope, but
URControl ran) and PolyScope X 10.13.0 (`universalrobots/ursim_polyscopex`,
arm64 outer image, most inner images amd64 under QEMU). Both answered the
same, register for register, and contradict the forum reports this page
used to repeat: no register read 65535 by default and writes took.
Reproduced in `crates/helmsman-io/tests/universal_robots.rs`, which holds
the PolyScope X capture.

- **Writes:** FC 6 and FC 16 to 130..132 read back as written (1234,
  0xBEEF, 65535); the box's own write through the profile took too.
- **Holes:** only these registers answer: 0..11, 16..22, 24..33,
  128..258, 260..266, 270..275, 280..285, 290..295, 300..305, 310..315,
  320..325, 400..405, 410..415, 420..428, 450..451, 768..770; bits 0..9,
  16..25, 64..143, 260..265, 300..427, 500..627. A request that touches
  any other address is refused whole with exception 2 (Illegal Data
  Address), so 256..260 fails over 259 and a 16-bit read of the input
  word (bits 0..15) fails over 10..15. The box reads one point a request
  today; a driver that batches must keep to these runs.
- **A joint's angle is two registers.** The table says 270..275 are the
  joint angles "in mrad". They are the angle inside one turn, 0..6283,
  and 320..325 hold the turns, signed: the base at -1.6007 rad (the
  realtime interface's `q_actual`, port 30003) read 4682 and -1
  (`65535`), and -1 * 6283 + 4682 = -1601. A client that reads 270 alone
  gets +4.682 rad. All six joints agreed.
- **TCP pose** (400..405) is two's complement: x -0.1440 m read `64096`
  (-1440 tenths of a mm), y -4356, z 2020, rx -1, ry 3116, rz 39 mrad,
  as on 30003.
- **State:** 256..257 the controller version (5.26, 10.13), 258 robot
  mode 3 (power off), 266 safety mode 1, 310..315 joint mode 239 (power
  off), 768 tool mode 253, joint temperatures 23..26.
- **Not tried:** powering the arm on, motion, joint speeds and currents
  with the arm moving (the profile reads them as `i16`: inferred, not
  seen), the configurable outputs and the general purpose booleans from a
  robot program.

EtherNet/IP did not answer on either: URSim 5.26 needs it switched on in
PolyScope, which could not start, and the PolyScope X image does not ship
the `ur-ethernetip` URCap (its bootstrapper: "Failed finding URCapX
ur-ethernetip in urcaps folder"). PROFINET is missing from it the same
way. So the robot, or the URCap file, is still what the EtherNet/IP half
of `ur-bringup` needs.

Running it here: `docker run -d --name ursimx --privileged --network
bridge -e HOST_ARCH=arm64 -e ROBOT_TYPE=UR5 -p 127.0.0.1:8000:80 -p
127.0.0.1:502:502 universalrobots/ursim_polyscopex:10.13.0`; its inner
nginx died under QEMU at boot and came up after `docker exec ursimx
docker restart root-nginx-1` (UI on http://localhost:8000). Stop with
`docker rm -f ursimx`.
