# Moxa ioLogik E1213

Remote Ethernet I/O: 8 DI, 4 DO and 4 DIO (source type), two switched
Ethernet ports, EtherNet/IP adapter and Modbus/TCP server (manual pp. 1-2,
2-4). The bench device for the box as an EtherNet/IP *scanner*: helmsman's
`enip-io` driver masters it over one Class 1 connection, and
`helmsman-adapter --profile moxa-e1213` simulates it. The bench passed on
2026-11-02 ("On the bench", below), so `enip-io` is in the default box
build; the same I/O also runs over Modbus/TCP
([`moxa-iologik-e1213-modbus.map`](moxa-iologik-e1213-modbus.map)).

## Sources

| | what | where |
| --- | --- | --- |
| **EDS** | `moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds`, 647 lines, 42 906 bytes, CRLF, SHA-256 `3895fbca8bb036c1b31a9d6b68dea69eca6892fe0a6bce7409ab76311cfd470d`. EZ-EDS 3.27 generated; `[File]` Revision 2.0, ModDate 03-16-2026; `MinRev = 1; $ corresponds to firmware v4.0, EDS revision 1` (l.20). | Nick's download, kept untracked in `devices/` at the repo root (`*.eds` is ignored). **Not in this repository**: the file carries no terms at all (only the generator's header line), so redistribution is unclear (TODO.md `e1213-eds-license`). `scripts/eds-corpus.sh` checks the hash and runs `crates/helmsman-io/tests/eds_corpus.rs` against it. "l.N" below is a line of this file. |
| **Manual** | *ioLogik E1200 Series User's Manual*, Version 15.10, May 2021, 10 137 362 bytes, SHA-256 `f8788816b2e0062a68f9b7f1ef9850a2d57f92946ac8b64077c4e77097744454`. | Fetched 2026-10-30 from a university mirror (`pct.wiki.uib.no/images/2/29/Moxa-iologik-e1200-series-manual-v15.10.pdf`); Moxa's own links to v15.8 and v15.2 answered with an HTML page instead of the PDF. "p. X-N" is the manual's own page number. |

The EDS says what to connect to and how big the data is; it says nothing
about what the bytes mean (the input assembly is one 408-bit member, l.466).
The meaning is from the manual's Appendix B. Where neither says, this page
says **inferred**.

## Identity (`[Device]`, l.13–22)

| attribute | value | line |
| --- | --- | --- |
| vendor | 991 "Moxa" | 14–15 |
| device type | 12 "Communications Adapter" | 16–17 |
| product code | 515 | 18 |
| revision | 4.1 ("firmware v4.0, EDS revision 1") | 19–20 |
| product name | "E1213", catalog "ioLogik E1200 Series" | 21–22 |

`[Device Classification]` `Class1 = EtherNetIP` (l.403–404). The EDS
covers the E1213 and E1213-T (file name; `[File]` DescText is just
"EtherNet/IP Adapter", l.5).

A real unit's Identity object may say more (serial number, the product
name it reports); `scripts/bench-e1213.sh` prints it. An electronic key
(`key=exact|compatible` on the device line) checks vendor, type, product
code and revision against the values above before the device accepts the
connection.

## Assemblies (`[Assembly]`, l.447–473)

| instance | name | bytes | members | line |
| --- | --- | --- | --- | --- |
| 101 (0x65) | Input Data | 51 | one of 408 bits, no reference | 460–466 |
| 102 (0x66) | Output Data | 1 | one of 8 bits | 467–473 |

`MaxInst = 2` (l.450). Instance 100 (configuration) and 111 (the
input-only heartbeat) appear only in connection paths, as zero-length
instances. The manual's Assembly object table agrees on 0x65 input / 0x66
output (p. B-1).

## Parameters (`[Params]`, l.406–445)

| param | name | type | min, max, default | line |
| --- | --- | --- | --- | --- |
| Param1 | Output Size | 0xC7 UINT, 2 bytes | 1, 1, 1 | 407–419 |
| Param2 | Input Size | 0xC7 UINT, 2 bytes | 51, 51, 51 | 420–432 |
| Param3 | RPI Range | 0xC8 UDINT, 4 bytes | 100000, 3000000, 100000 (µs) | 433–445 |

So the RPI is 100 ms ..= 3 s, default 100 ms, both directions. The manual
recommends at least 100 ms (p. 6-10).

## Class 1 connections (`[Connection Manager]`, l.475–573)

| | Connection1 (l.484–528) | Connection2 (l.529–573) |
| --- | --- | --- |
| name | "Exclusive Owner" (l.526) | "Input Only" (l.571) |
| trigger/transport word | `0x04010002` (l.485): class 1, cyclic, exclusive owner | `0x02010002` (l.530): class 1, cyclic, input only |
| connection word | `0x44640405` (l.496) | `0x44640305` (l.541) |
| O->T | RPI Param3, size Param1 = **1 byte**, format Assem102 (l.522); real-time format 4: **32-bit run/idle header** | RPI Param3, size 0, no format (l.567); real-time format 3: **heartbeat** |
| T->O | RPI Param3, size Param2 = **51 bytes**, format Assem101 (l.523); real-time format 0: **modeless** (no header) | the same (l.568) |
| config | none (proxy and target config empty, l.524–525) | none |
| path | `20 04 24 64 2C 66 2C 65` (l.528): Assembly, config 100, O->T 102, T->O 101 | `20 04 24 64 2C 6F 2C 65` (l.573): config 100, heartbeat 111, T->O 101 |

There is **no listen-only connection**: the EDS declares none.

The connection word, bit by bit, read with the bit legend the generator
prints beside it (l.496–521): bits 0 and 2 set (O->T and T->O fixed
size); bits 8–10 = 4 and 12–14 = 0 (the real-time formats above); bit 18
(O->T point-to-point); bits 21 and 22 (T->O multicast or point-to-point;
`enip-io` asks for point-to-point); bits 26 and 30 (scheduled priority
both ways).

**Inferred:** the names of real-time formats 0, 3 and 4. Neither EDS read
spells them out. `OpENer`'s sample EDS declares the same words for its
own exclusive owner (`0x44640405`) and input-only (`0x44640305`)
connections, and `OpENer` consumes O->T data with a run/idle header and
produces T->O without one, with an O->T size of 0 for input-only
(`helmsman_proto::eds::RealTimeFormat` cites both).

At most **10 connections, "9 for read-only, and 1 for read/write"**
(manual p. 3-27): one exclusive owner at a time. EtherNet/IP is off until
enabled on the web console *and* activated per device (pp. 3-26, 5-1);
an E1213 that doesn't answer `ListIdentity` or refuses every
`Forward_Open` may simply not be activated. Ports: 44818/tcp explicit,
2222/udp implicit (Appendix D).

What the box does about 2222: its `enip-io` link and its own adapter share
the box's one I/O socket (docs/ARCHITECTURE.md, "One Class 1 I/O port per
box"), so on 2222 the E1213 sends T->O where it would anyway and the
`Forward_Open` carries no socket-address item, as EIPScanner's doesn't
(`src/ConnectionManager.cpp`). If the box's `--io-port` moves it off 2222
the `Forward_Open` adds a T->O socket-address item (CPF 0x8001) naming the
port; `OpENer` honours one (`cipioconnection.c`
`OpenProducingPointToPointConnection`), whether the E1213 does is
unverified until `e1213-bench` runs off 2222.

## What the bytes mean (manual Appendix B, pp. B-1–B-2)

Input assembly 101, 51 bytes:

| bytes | name | type | channels | `enip-io` points |
| --- | --- | --- | --- | --- |
| 0–1 | diStatus, "DI mode status (0: OFF, 1: ON)" | BOOL | DI-00..DI-11 | `di:0`..`di:11`, `in:0.0`..`in:1.3`, `in:0:u16` |
| 2–49 | diCounterValue, "Counter mode value" | UDINT | DI-00..DI-11 | `in:2:u32` (DI-00) .. `in:46:u32` (DI-11) |
| 50 | doStatus, "DO mode status" | BOOL | DO-00..DO-07 | `in:50.0`..`in:50.7`, `in:50:u8` |

Output assembly 102, 1 byte:

| byte | name | type | channels | `enip-io` points |
| --- | --- | --- | --- | --- |
| 0 | doStatus | BOOL | DO-00..DO-07 | `do:0`..`do:7`, `out:0.0`..`out:0.7`, `out:0:u8` |

12 + 48 + 1 = 51 bytes, which is Param2 and Assem101's size: the manual's
table and the EDS agree.

**Inferred:**

- Bit *n* of the little-endian word is channel *n* (DI-00 is byte 0 bit 0).
  The manual gives byte ranges ("1…0") and channel ranges ("DI-11…00"),
  not bit positions; this is CIP's packing of BOOL arrays.
- Counter *n* is the UDINT at byte `2 + 4n`, little-endian (CIP's byte
  order). The manual gives only "49…2".
- DI-08..DI-11 and DO-04..DO-07 are the four hybrid DIO channels, which
  read as DI or DO by jumper (default DO, p. 2-5; "4 pure DO channels and 4
  hybrid DIO channels", p. 2-6). The manual doesn't say which channel
  numbers they take.
- What the outputs do when the owner goes idle or the connection times
  out. The manual's "safe mode" is described for the Modbus/TCP watchdog
  (pp. 3-4, 3-11, 3-21); for EtherNet/IP it doesn't say. The simulator turns
  every output off.
- The counters count only in counter mode (off by default, Appendix E); the
  simulator counts every DI's rising edges.

The bench check (`scripts/bench-e1213.sh`) is what turns these into facts:
it drives each wired DO and watches the DI it is wired to. What the bench
unit showed so far is under "On the bench" below: the bit order and the
DO status byte are confirmed, the DIO numbering and counter byte order are
still inferred.

## Modbus/TCP (manual p. A-7)

The same I/O on port 502. Addresses are the manual's 0-based "Start
Address" (our `modbus-tcp` driver's address); its "Start Register" column is
the same plus the 1-based table prefix.

| name | table | address | count | `modbus-tcp` point |
| --- | --- | --- | --- | --- |
| DI_status | discrete input | 0 | 12 | `di:0`..`di:11` |
| DI_counterValue, "high/low word" | input register | 16 | 24 (2 a channel) | `ir:16:u32` (DI-00) .. `ir:38:u32` |
| DI-all_statusFromDI-00 | input register | 48 | 1 | `ir:48` |
| DO_status | coil | 0 | 8 | `coil:0`..`coil:7` |
| DO-all_statusFromDO-00 | holding register | 32 | 1 | `hr:32` |
| DO_pulseStatus / pulseCount / OnWidth / OffWidth | coil 16, hr 36, 52, 68 | | 8 | (pulse mode; not used here) |
| DI_counterStatus / Reset / OverflowFlagClear | coil 256, 272, 288 | | 12 | (counter control) |

"High/low word" is high word first, the driver's default for `u32`
(no `sw`). The unit id is not in the manual; `unit=1` is assumed.
**Inferred:** counter *n* at `16 + 2n`.

## I/O maps

The same signals over either transport; both files are checked by
`crates/helmsman-io/tests/enip.rs` (`the_e1213_example_maps_validate`).

- [`moxa-iologik-e1213-enip.map`](moxa-iologik-e1213-enip.map): `enip-io`,
  `profile=moxa-e1213` (the facts above, built in), or `eds=PATH` to read
  the vendor's file at start; in a project, Import device… on the Devices
  tab keeps the EDS as `NAME.eds` and writes `eds=NAME.eds`.
- [`moxa-iologik-e1213-modbus.map`](moxa-iologik-e1213-modbus.map):
  `modbus-tcp`.

EtherNet/IP gives one exclusive owner and a cyclic image of all 51 bytes
every RPI; Modbus/TCP gives request/reply per point and no ownership.

## On the bench (2026-11-02)

The unit on the bench: E1213, serial TAIGB1106222, firmware **V3.1
Build19110615**, MAC 00-90-e8-7e-af-be, on the office LAN (10.0.0.x) by
DHCP at 10.0.0.57 when this was written (its web console, "Ethernet
Configuration", says DHCP; it was not at the factory 192.168.127.254).
EtherNet/IP was already activated and enabled on it. DO-00..02 are wired
to DI-00..02. Observed with a stand-alone Python scanner and the Modbus
driver, both sides at once, so each fact below is from the wire:

- **ListIdentity** answers vendor 991, type 0x0C, product code 515,
  **revision 3.1**, status 0x0004, product name "ioLogik E1213". The EDS
  v2.0 describes revision 4.1 (firmware 4.0): an electronic key with
  `key=exact` against the built-in profile would be refused by this unit;
  `key=compatible` or `key=off` (the default) connects.
- **O->T goes to UDP 5222, not 2222.** The `Forward_Open` reply carries an
  O->T socket-address item (CPF type 0x8000: `AF_INET`, port 5222, address
  0.0.0.0). O->T datagrams sent to 2222 are accepted silently and ignored:
  the outputs never move and the DO status byte stays 0. Sent to 5222,
  every bit lands. helmsman's scanner honours the item since this day
  (`helmsman_proto::scanner`, `IoConnection::target`); the simulated E1213
  (`enip_sim`, `helmsman-adapter --o-to-t-port`) answers the same way and
  drops O->T on its I/O port, so the conformance run exercises it.
- **T->O comes from UDP source port 1032** on the device, to the
  originator's 2222, every 100 ms at RPI 100 ms (29 datagrams in 3 s).
- **Bit order confirmed:** O->T byte 0 bit *n* is DO-*n*; T->O byte 0 bit
  *n* is DI-*n*; T->O byte 50 bit *n* is DO-*n*'s status. A DO written over
  Modbus (coil *n*) shows in byte 50 bit *n* within one RPI, and the two
  transports agree on every channel.
- **The loops close only with EXT field power.** Until 24 V was on the
  EXT terminals the DOs reported ON (coils, byte 50) while no DI followed,
  and the console's overview said "DO(DO00-DO03) Power Source: Unselect or
  No External Power". The E1213's DO is source type: it drives the EXT
  supply out, so without EXT nothing reaches the DI (manual p. 2-6, "EXT
  Power Configuration": JP10 field power is the default; the EXT LED
  shows it, p. 1-4). The console's "DO(DO04-DO07) Power Source: Field
  Power (Fixed)" is the hybrid DIO group.
- **A second `Forward_Open` while a connection is open** is refused with
  extended status 0x0100 (connection in use); a connection whose owner
  vanishes without `Forward_Close` is held for about 10 s before another
  owner is accepted.
- **Counters read 0** in DI mode (counter mode is off, as Appendix E
  says).
- The console's DO page says "DO4-7 Channel Equal DIO0-3 Channel": the
  hybrid channels are DO-04..07 / DI-08..11, as inferred above.
- Modbus/TCP: `di:0..11`, `coil:0..7`, `ir:16..` and `hr:32` (DO-all, 8
  bits wide: writing 0xA55A reads back 0x5A) all answer with unit id 1.

What the macOS firewall does to this: the Mac allows incoming UDP only for
software signed by a developer, and a cargo binary is ad-hoc signed, so
T->O never reaches `conformance` or `helmsman-plc` until each is allowed
(`scratch/02-allow-firewall.sh`, `sudo socketfilterfw --add --unblockapp`,
again after every rebuild). Apple's own `python3` is allowed, which is how
the facts above were taken over SSH.

## Checking it

- Simulated, any machine: `cargo run -p helmsman-io --example conformance
  -- --sim-e1213` (14 PASS: five reads, four DO->DI loops, a point past the
  input assembly, and four Class 1 faults: silent target, short input,
  garbage input, target reboots).
- Stand-alone simulator for other scanners: `helmsman-adapter --profile
  moxa-e1213`.
- The real device: `scripts/bench-e1213.sh IP` (TODO.md `e1213-bench`):
  ListIdentity, the Identity object, then the conformance run over Class 1.
  On a Mac, `scratch/02-allow-firewall.sh` first (above).
- The loop check as a program: [`e1213-loop/`](e1213-loop/) (`main.st`,
  `io.map` over Modbus/TCP, `io-enip.map` over EtherNet/IP) steps DO-00..02
  and counts cycles and faults; `helmsman-plc docs/devices/e1213-loop/main.st
  --io-map docs/devices/e1213-loop/io.map --ui target/canvas-dist` shows it
  live in the canvas. Over Modbus on the bench: 11 cycles, 0 faults in 20 s.
