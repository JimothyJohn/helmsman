# Moxa ioLogik E1213

Remote Ethernet I/O: 8 DI, 4 DO and 4 DIO (source type), two switched
Ethernet ports, EtherNet/IP adapter and Modbus/TCP server (manual pp. 1-2,
2-4). The bench device for the box as an EtherNet/IP *scanner*: helmsman's
`enip-io` driver masters it over one Class 1 connection, and
`helmsman-adapter --profile moxa-e1213` simulates it.

## Sources

| | what | where |
| --- | --- | --- |
| **EDS** | `moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds`, 647 lines, 42 906 bytes, CRLF, SHA-256 `3895fbca8bb036c1b31a9d6b68dea69eca6892fe0a6bce7409ab76311cfd470d`. EZ-EDS 3.27 generated; `[File]` Revision 2.0, ModDate 03-16-2026; `MinRev = 1; $ corresponds to firmware v4.0, EDS revision 1` (l.20). | Nick's download, kept untracked at the repo root. **Not in this repository**: the file carries no terms at all (only the generator's header line), so redistribution is unclear (TODO.md `e1213-eds-license`). `scripts/eds-corpus.sh` checks the hash and runs `crates/helmsman-io/tests/eds_corpus.rs` against it. "l.N" below is a line of this file. |
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
it drives each wired DO and watches the DI it is wired to.

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
  the vendor's file at start.
- [`moxa-iologik-e1213-modbus.map`](moxa-iologik-e1213-modbus.map):
  `modbus-tcp`.

EtherNet/IP gives one exclusive owner and a cyclic image of all 51 bytes
every RPI; Modbus/TCP gives request/reply per point and no ownership.

## Checking it

- Simulated, any machine: `cargo run -p helmsman-io --example conformance
  -- --sim-e1213` (14 PASS: five reads, four DO->DI loops, a point past the
  input assembly, and four Class 1 faults: silent target, short input,
  garbage input, target reboots).
- Stand-alone simulator for other scanners: `helmsman-adapter --profile
  moxa-e1213`.
- The real device: `scripts/bench-e1213.sh IP` (TODO.md `e1213-bench`):
  ListIdentity, the Identity object, then the conformance run over Class 1.
