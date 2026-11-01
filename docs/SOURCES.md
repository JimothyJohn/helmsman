# Source review: stacks, docs, and what to build on

Written 2026-10-26. Every claim below is traced to the source it came from; the
"How to cite" line under each entry is the citation form to use in code
comments, docs, and PRs when that source is actually leaned on.

## The one-paragraph verdict

The listed sources are mostly **adapter** (device-side, target) stacks. The
thing you described — an open-source box that *configures and talks to*
customers' devices — is a **scanner / explicit-message client** (originator).
Only one of the six code links (EIPScanner) is on that side of the wire, and it
is the least maintained of the three real stacks. OpENer and CIPster are the
right references for *how the wire protocol behaves* and for building a
test target, but they are not the thing you wrap. The TI link is a
hardware-locked vendor SDK page and is useful only as a feature checklist. The
ibexuk link is unrelated. Of the three PDFs, the ODVA Developers Guide
(PUB00213) is the one worth reading end to end; PUB00138 is a brochure; the
Rockwell 2001 white paper is a historical curiosity.

## Licensing, in plain terms

The license you pasted is CIPster's copy of the OpENer license (Rockwell 2009 +
SoftPLC 2016). It is BSD-shaped with two riders:

1. **Trademark.** "nor does this software result in any license to use the
   EtherNet/IP mark owned by ODVA." Your project must not carry "EtherNet/IP"
   in its name, logo, or marketing. "CIP" is also an ODVA trademark
   (PUB00213R0, Copyright Notice, p.2). Describe the protocol; don't brand with
   it. The repo name `tcpethernet` is fine.
2. **Patents / Terms of Usage.** The license says it conveys no right to
   "make, have made, use, import, offer to sell, sell ... any products that
   implement this software," and clause (c) extends that to "enabling
   technology products, such as any other EtherNet/IP or other network protocol
   stack ... to the extent they implement portions of the Final Specification."
   A scanner-side client *does* implement portions of the spec. That rider
   binds whoever ships a product, not the open-source code itself. Your
   downstream users who sell devices still need their own ODVA Terms of Usage
   Agreement and a Vendor ID (PUB00213R0, "Getting started," p.10, and the
   Development Checklist, p.27). Your project doesn't change that for them, and
   shouldn't claim to.

Practical consequence: get ODVA's "we're okay with it" in writing (an email is
enough), file it in the repo under `docs/`, and put a short notice in the
README that the project is not an ODVA product and does not grant any license
to the EtherNet/IP technology. The verbal confirmation is worth nothing if
someone at ODVA changes roles.

Also note the Rockwell white paper is © IEEE / Rockwell and PUB00213 says
"For permission to reproduce excerpts ... contact ODVA." Quote sparingly;
paraphrase and cite page numbers instead.

## Code sources

### OpENer — EIPStackGroup/OpENer

- **What:** C adapter stack. "OpENer is an EtherNet/IP stack for I/O adapter
  devices. It supports multiple I/O and explicit connections" (README).
- **License:** the adapted-BSD text above, Rockwell 2009 copyright.
- **Health (checked 2026-10-26 via GitHub API):** 867 stars, 318 forks, 96
  open issues, 14 open PRs. Last human commit 2026-07-24 ("Fix: Memory leaks",
  Tomasz Lisowski); maintainers Martin Melik Merkumians and Alois Zoitl were
  active in late 2025 on CI and branding. Last tagged release is **v2.3 from
  March 2019**; everything since lives on `master` only. Dependabot keeps the
  Actions pinned, and there's an AFL fuzz harness (`fuzz/`) and a MegaLinter
  config, which is more hygiene than any other stack here.
- **Objects implemented** (from `source/src/cip/`): Identity, Message Router,
  Assembly, Connection Manager, Connection Object, I/O connection, Class 3
  connection, Electronic Key, EPATH, Ethernet Link, TCP/IP Interface, QoS,
  **DLR**, CIP strings. No CIP Security, no CIP Sync, no Time Sync object.
- **Ports:** POSIX, WIN32, MINGW, **STM32** (bare metal via
  `ports/STM32/`), plus `nvdata` for persisted TCP/IP config.
- **Application surface** (`source/src/opener_api.h`): `CipStackInit`,
  `CreateCipClass`/`AddCipInstance`/`InsertAttribute`/`InsertService` to
  define objects; `CreateAssemblyObject` plus
  `ConfigureExclusiveOwnerConnectionPoint` / `...InputOnly...` /
  `...ListenOnly...` for I/O; a full `EncodeCip*` / `DecodeCip*` family for
  every CIP elementary type. The app implements the callbacks in
  `sampleapplication.c`: `ApplicationInitialization`, `HandleApplication`,
  `CheckIoConnectionEvent`, `AfterAssemblyDataReceived`,
  `BeforeAssemblyDataSend`, `ResetDevice`, `RunIdleChanged`.
- **Use it for:** (a) the reference semantics when writing the originator side
  (the Forward_Open parsing in `cipconnectionmanager.c` is the most readable
  description of the connection path encoding you'll find outside the spec);
  (b) a *test target* in CI — build the POSIX sample, run it in Docker, and
  point your client at it. The README's "swarm" Docker setup exists for exactly
  this. (c) The `Encode/Decode` tables are a checklist for your own codec.
- **Don't use it for:** the client role. There's no originator, no
  List_Identity scanning, no Forward_Open *sending*.
- **How to cite:** `OpENer v2.3+master (EIPStackGroup/OpENer, commit <sha>),
  source/src/cip/<file>.c`.
- **Class 1 connection IDs and ports** (commit `0e1ab7ee`, the one
  `docker/opener/Dockerfile` pins; read 2026-10-31 for `enip-io-port`,
  `helmsman_proto::endpoint`):
  - `cipconnectionobject.c` `ConnectionObjectInitializeFromMessage`: the
    `Forward_Open`'s O->T ID becomes the consumed ID, its T->O ID the
    produced ID; `ConnectionObjectGeneralConfiguration` then, for a
    point-to-point O->T, replaces the consumed ID with `GetConnectionId()`
    ("if we have a point to point connection for the O to T direction the
    target shall choose the connection ID"). So each direction's ID is
    the consumer's choice: O->T the target's, T->O (point-to-point) the
    originator's.
  - `cipconnectionmanager.c` `HandleReceivedConnectedData` /
    `GetConnectedObject`: an incoming I/O datagram is matched to an
    established connection by its consumed connection ID, then dropped
    unless the sender's IP equals the connection's `originator_address`
    (the IP only, not the port).
  - `cipioconnection.c` `SendConnectedData`: the sequenced address item
    carries `cip_produced_connection_id`.
    `OpenProducingPointToPointConnection`: T->O goes to the session peer's
    IP at `kOpenerEipIoUdpPort` (2222), "the default port to be used if
    no port information is part of the forward open request", or at the
    `sin_port` of a T->O socket-address item (0x8001) in it.
    `OpenConsumingPointToPointConnection` stores the peer IP "for packet
    scanning".

### CIPster — liftoff-sr/CIPster

- **What:** "CIPster is a C++ port of C based OpENer with additional
  features" (README). Still adapter-only. Library builds to `libeip.a`.
- **License:** same adapted-BSD, with SoftPLC 2016 added. README: "this
  project does not require copyright assignment."
- **Health:** 183 stars, 53 forks, 3 open issues, **no tagged releases**. But
  it is the most *recently* alive of the three: Dick Hollenbeck landed a
  security fix on 2026-10-19 ("Fix UDP reply length disclosing stale buffer
  contents"), and July 2026 merged a batch of hardening PRs from one
  contributor (assembly write-access enforcement, I/O size lower bound, type
  confusion on assembly size, connection-slot leak on Forward_Open failure)
  with regression tests under `tests/security`. That batch is worth reading
  because each one is a bug class your client must tolerate on the *other* end.
- **Verdict:** Prefer OpENer as the reference and test target. CIPster is a
  one-maintainer C++ fork with no releases and a freelists.org mailing list.
  Its value to you is the July 2026 security PR set as a list of adversarial
  cases. Don't take a dependency on it.
- **How to cite:** `CIPster (liftoff-sr/CIPster, commit <sha>), source/src/cip/<file>`.

### EIPScanner — nimbuscontrols/EIPScanner

- **What:** "Free implementation of Ethernet/IP scanner in C++" (README).
  The only **originator** in the list. MIT, © 2019 Aleksey Timin.
- **Features (README):** explicit messaging; implicit messaging "(only
  point-to-point)"; discovery (List_Identity); Identity, File (upload only)
  and Parameter (read only) objects; one vendor-specific object (RA DPI
  Fault). C++20, CMake, Linux/macOS/Windows.
- **Health:** 319 stars, 125 forks, 42 open issues. **Last commit
  2024-11-26**, last release 1.3.0 in June 2021. Open issues as of today
  include "Incorrect session closing" (#127, Aug 2026), two competing PRs to
  add multicast T2O (#125/#126, Jul 2026), "Device stops sending due to
  timeout" (#118), and "Incomplete installation process" (#119). That is a
  project with users and no maintainer.
- **Why it still matters:** `examples/ImplicitMessagingExample.cpp` is the
  most compact working description of what an originator has to send in a
  Forward_Open: connection path `{0x20,0x04, 0x24,<cfg>, 0x2C,<out>, 0x2C,<in>}`
  (class 4 = Assembly, config instance, then O->T and T->O connection
  points), `o2tRealTimeFormat` (the 32-bit run/idle header), originator
  vendor ID + serial, the network-connection-parameter bitfields (P2P,
  scheduled priority, size), RPIs in microseconds, and `CLASS1` in the
  transport trigger. Port that shape, not the code.
- **Verdict:** Use as a *behavioral reference* and as a second interop
  partner in tests (it's trivially buildable). Do not wrap it or bind to it;
  you'd inherit 42 issues and a C++20 toolchain requirement.
- **How to cite:** `EIPScanner 1.3.0 (nimbuscontrols/EIPScanner), examples/<file>.cpp`.
- **`src/ConnectionManager.cpp`** (master, read 2026-10-31 for
  `enip-io-port`): `forwardOpen` sends no socket-address item (an empty
  additional-items list), so targets use their default T->O port; it
  reads an O->T socket-address item from the reply (address 0.0.0.0
  meaning the session's host). Received T->O is dispatched by connection
  ID through `_connectionMap`, keyed by the reply's T->O ID, with no check
  of the sender's address ("TODO: Check TypeIDs and sequence").

### TI AM64x Industrial Communications SDK — EtherNet/IP Adapter quickstart

Two links were given (09_00_00_03 and `latest`). They are the same page across
SDK versions.

- **What:** Adapter example for Sitara AM64x. "Designed for Texas Instruments
  Sitara AM64x based platforms with PRU-ICSS IP" (index page); runs on an R5F
  core under FreeRTOS with protocol handling in PRU firmware.
- **Licensing:** The quickstart and index pages state no license, no source
  availability, and no evaluation terms. The datasheet page splits features
  into Standard vs Premium and says "This feature is part of premium SDK ...
  please contact your regional TI sales representative for additional
  details." Treat it as a proprietary, hardware-locked binary SDK.
- **Useful bit, the datasheet (`eip_datasheet.html`):** this is a concrete
  feature list for a conformance-tested (ODVA CT22) adapter and is a good
  checklist of what a *real device* on the other end of your client may do:
  Identity, Message Router, Assembly, Connection Manager, Time Sync, DLR,
  QoS, TCP/IP Interface, Ethernet Link, LLDP; Exclusive Owner / Input Only /
  Listen Only; Class 1 max 8 connections, Class 3 max 32; 1 ms cyclic;
  QuickConnect ~300 ms to Forward_Open; CIP Sync with PTP over UDP; ACD.
  The quickstart adds the gotchas "Concurrent connections are currently not
  supported" and "Get_Attribute_All returns only Attribute IDs 1 to 22 for
  the Identity Object," both things a client should tolerate.
- **Verdict:** Trash as a *source*, keep the datasheet as a *checklist*. Nothing
  here can be built on, linked, or redistributed. If a customer runs AM64x,
  this page tells you what their device supports.
- **How to cite:** `TI IND_COMMS_SDK AM64x, EtherNet/IP Adapter datasheet (eip_datasheet.html), accessed 2026-10-26`.

### ibexuk/C_Communications_Ethernet_TCP_IP_Driver

- **What:** "TCP/IP Ethernet stack for embedded microcontrollers" (repo
  description) for PIC18/24/32, LPC2000 and ENC28J60-class NICs. MIT. Last
  commit **2016-10-04**. Modules: ARP, DHCP, DNS, HTTP, ICMP, IP, NetBIOS,
  POP3, SMTP, SNTP, TCP, UDP.
- **Relevance:** none. It contains no CIP or encapsulation code; it's the
  layer *under* an adapter stack on a 2010-era 8/16-bit micro. Every target
  you'd plausibly run on (Linux SBC, STM32 with lwIP, AM64x) already has a
  TCP/IP stack.
- **Verdict:** Trash. Drop it from the reading list.

### Not on your list, and should be

Your stack note says Python or Rust, one language per project, and the role
you need is scanner/client. The Python originators are more mature than
anything above:

| Repo | Lang | License | Stars | Last push | Notes |
|---|---|---|---|---|---|
| ottowayi/pycomm3 | Python | MIT | 521 | 2025-12-22 | `CIPDriver` does generic CIP (List_Identity, generic messaging, Forward_Open); `LogixDriver` adds Rockwell tag services. |
| pjkundert/cpppo | Python | dual GPL/commercial | 391 | 2025-07-08 | Full originator *and* a simulated target; licensing is the problem. |
| libplctag/libplctag | C | MPL-2.0 | 904 | 2026-10-04 | PLC-tag focused (AB + Modbus), not generic CIP objects. |

pycomm3 is the one to study first for the Python shape of the problem. It
doesn't do implicit I/O, which is the gap EIPScanner's example fills.

### pycomm3 — ottowayi/pycomm3 (Logix tag services, prior art)

- **What:** Python client for Logix-family controllers over explicit
  messaging. MIT (Ian Ottoway, Agostino Ruscito). Read 2026-10-30 at
  `master` = `36aeb19b5cb47495df9fd1caa38ce9d2f6d12ea3`, for byte layouts
  only; no code copied.
- **Leaned on by** `crates/helmsman-proto/src/logix/` and the fixture in
  `crates/helmsman-adapter/src/logix.rs`:
  - `pycomm3/cip/data_types.py`: elementary type codes (BOOL `0xC1` ..
    LREAL `0xCB`, BYTE `0xD1` .. LWORD `0xD4`); `DataSegment` extended
    symbolic `0x91` with length byte and odd-length pad; `LogicalSegment`
    member type and 8/16-bit widths.
  - `pycomm3/const.py`: `STRUCTURE_READ_REPLY = b"\xa0\x02"`,
    `INSUFFICIENT_PACKETS = 6`, `PRIORITY = b"\x0a"`,
    `TIMEOUT_TICKS = b"\x05"`.
  - `pycomm3/packets/logix.py`: Read Tag request (service, path, element
    count `UINT`); Read Tag Fragmented adds a `UDINT` byte offset; reply
    type is 2 bytes for atomics, 4 for structures;
    `MultiServiceRequestPacket` (to Message Router instance 1, count,
    offsets from the count field, requests) and its response parser.
  - `pycomm3/packets/util.py`: `tag_request_path` (split on `.`, indexes
    as member segments), `wrap_unconnected_send` (priority, timeout ticks,
    length, message, pad if odd, route path), `parse_read_reply`.
  - `pycomm3/logix_driver.py`: `_get_instance_attribute_list_service`
    (Symbol object `0x6B`, Get Instance Attribute List, attribute ids),
    `_parse_instance_attribute_list` (instance `UDINT`, name as `UINT`
    length + bytes, then the requested attributes in order),
    continuation on status `0x06` from `instance + 1`, `_create_tag`
    (symbol type bits: 15 structure, 13-14 dimension count, 12 system,
    0-11 template id, 0-7 atomic code; bits 8-10 as a BOOL's bit position
    are read there but not used here),
    `_send_read_fragmented` (offset advances by bytes received).
  - `pycomm3/cip_driver.py`: the route path is encoded
    `length=True, pad_length=True` with `PortSegment("bp", slot)` for the
    backplane. The bytes (`words, 0x00, 0x01, slot`) are our reading of
    those flags (the encoder itself was not read); unverified on hardware.
  - `pycomm3/cip/status_info.py`: meanings of general status `0x04`,
    `0x05`, `0x06`, `0x13`, `0x1E`, `0x26` and extended `0x2104`,
    `0x2105`, `0x2107` under `0xFF` (the fixture's error choices).
- **How to cite:** `pycomm3 <file> <function or constant>`.

### libplctag — libplctag/libplctag (Logix tag paths, prior art)

- **What:** C library for PLC tag access. MPL-2.0 (the repo's SPDX id;
  older releases also offered LGPL). Read 2026-10-30 at `release` =
  `7712ed7f1ee05ba0ebfca641b3f5c43da52a66ed`, for layouts and parsing
  rules only; **no code copied** (copyleft).
- **Leaned on by** `crates/helmsman-proto/src/logix/path.rs`:
  `src/libplctag/protocols/ab/cip.c` `parse_numeric_segment` (element
  index segments `0x28` 8-bit, `0x29 0x00` + 16-bit, `0x2A 0x00` + 32-bit),
  `cip_encode_tag_name` (at most three dimensions; a `.N` bit suffix is
  kept out of the encoded path), `parse_symbolic_segment` (name
  characters: letters, digits, `_`, `:`; length byte, odd pad). This
  repo is stricter on the first character (letter or `_`).
- **How to cite:** `libplctag cip.c <function>`.

## Document sources

### ODVA PUB00213R0, EtherNet/IP Quick Start for Vendors Handbook (2008, 41 pp)

**Read this one.** It's the only source that explains what the pieces are and
what a vendor is obliged to do. Dated (it predates CIP Security, DLR as a
mandatory feature, and the current spec volume numbering), but the core model
hasn't moved.

What to pull from it, by page:

- **p.11, Table 1 and text:** explicit = TCP, request/reply, connected or
  unconnected; implicit = UDP, cyclic/CoS/application trigger, unicast or
  multicast. "Connections are established using the ForwardOpen Request
  service of the Connection Manager Object ... Connection clean-up takes place
  when a ForwardClose Request service request is issued or when either
  connection end point times out."
- **p.12, device taxonomy:** Explicit Message Server, Explicit Message Client,
  I/O Adapter, I/O Scanner. "All EtherNet/IP devices are required to have
  minimal explicit server capability." Your box is an Explicit Message Client
  first and an I/O Scanner second.
- **p.13, Security:** minimum robustness checklist for *devices* (nmap,
  Nessus, broadcast flood, malformed packets). Invert it: those are the
  adversarial inputs your client's parser must survive from a hostile or
  broken device.
- **p.17, essentials:** Run/Idle notification ("Controllers/scanners
  typically use it to notify their devices that they have gone into program
  (idle) mode. It is important that target devices react to this notification
  appropriately."), the Assembly object as the I/O data carrier, TCP/IP
  Interface + Ethernet Link as mandatory for embedded devices.
- **p.18, EDS:** "An EDS file can be placed in a device using the File Object"
  — that's why EIPScanner has a File Object with upload only. A config tool
  should try to pull the EDS from the device before asking the user for it.
- **p.23–26, Glossary:** port 44818 for encapsulation and explicit, UDP 2222
  for I/O; Class 1 I/O carries a 16-bit sequence count, Class 0 doesn't, Class
  3 is explicit; Exclusive Owner / Input Only / Listen Only definitions
  (Listen Only "can only be attached to an existing Exclusive Owner or Input
  Only Connection"). This glossary is your type vocabulary.
- **p.28–29, Appendix A (EDS):** which sections matter ([Device] matching on
  the first four Identity attributes excluding minor rev; [Connection
  Manager] `ConnectionN` entries; the three application paths config /
  consumed / produced). This is the spec for an EDS parser.
- **p.37–39, Appendix C:** sequence diagrams for unconnected explicit,
  connected explicit, and I/O. The text survives extraction; the figures
  don't, open the PDF for those.
- **p.40, Appendix D:** Wireshark has a built-in CIP decoder. Use it as the
  oracle in tests.

How to cite: `ODVA PUB00213R0 p.<n>` or `PUB00213R0 Appendix A`.

### ODVA PUB00138R8, EtherNet/IP Technology Overview (Feb 2024, 7 pp)

A brochure. Current (2024), so it's the right citation for the *feature
landscape*: Ethernet-APL, PRP, QuickConnect, CIP Safety/Sync/Motion/
Security/Energy (p.1), DLR (p.2), and the Messaging / Adapter / Scanner class
definitions (pp.4–5). The one genuinely useful paragraph is p.5 on
producer/consumer: "If a second (third, fourth, etc.) node wants the same
data, when it asks for it, the device will provide the multicast address and
Connection ID of the message already being produced." Everything else is in
PUB00213 with more detail.

Verdict: cite it for feature names and the three device classes; don't read
it for implementation.

### Rockwell ENET-WP001A-EN-P, "EtherNet/IP: Industrial Protocol" (Paul Brooks, EFTA 2001, 12 pp)

A 2001 conference paper reprinted as a Rockwell white paper. Historical
context only:

- It argues that switched full-duplex Ethernet is deterministic *enough* for
  control (§V), which was the live debate in 2001 and isn't now.
- §IV.D–E and §V.D are a decent prose description of UCMM vs Connection
  Manager, explicit vs implicit, and the multicast-plus-heartbeat mechanism.
  §V.D is the clearest statement anywhere in your sources of *why* the
  consumer heartbeat exists: "As UDP and IP are unacknowledged transmission
  mechanisms, the producer has no way of knowing if the consumer is online."
- It calls CIP "Control an Information Protocol" (sic, p.1) — that expansion
  was retired for "Common Industrial Protocol." The Identity Object attribute
  table (p.6) is still accurate.
- Three of its trigger types (Polled, Strobed) are DeviceNet-isms that don't
  apply to EtherNet/IP; PUB00213 p.11 gives the real three (Cyclic, CoS,
  Application).

Verdict: trash as a reference. Cite only §V.D if you want a quotable
explanation of the heartbeat connection. Don't let its 2001 vocabulary leak
into the code.

### Modbus Organization, MODBUS Application Protocol Specification V1.1b3 (2012-04-26, 50 pp)

- **Where:** https://www.modbus.org/file/secure/modbusprotocolspecification.pdf
  (linked from https://www.modbus.org/modbus-specifications as "Modbus
  Protocol Specification"). Fetched 2026-10-29, sha256
  `f80d0d7132fd035104c3f21b01026470e3310ca3d8d493d9a9737d0d32ad2eb1`. Not
  vendored: Modbus Organization copyright.
- **Leaned on by** `crates/helmsman-io/src/modbus.rs` and `sim.rs`: PDU
  limit of 253 bytes (§4.1 p.3), 0-based PDU addressing (§4.4), function codes
  0x01-0x06 (§6.1-6.6 pp.11-19) with their quantity limits (2000 bits, 125
  registers) and coil encoding (0xFF00/0x0000, §6.5 p.17), LSB-first bit
  packing with zero padding (§6.1 p.11), 0x10 Write Multiple Registers (§6.12
  p.30, 1-123 registers), exception replies and codes (§7 p.47). The same
  application layer runs over RTU (`rtu.rs`), which also takes reply lengths
  from it.
- **How to cite:** `App §<x> p.<n>` in the module, after the module docs name
  the document once.

### Modbus Organization, MODBUS Messaging on TCP/IP Implementation Guide V1.0b (2006-10-24, 46 pp)

- **Where:** https://www.modbus.org/file/secure/messagingimplementationguide.pdf
  (linked as "Modbus TCP/IP"). Fetched 2026-10-29, sha256
  `065d71170475642e82f370c648bf5135263cb20f61ce92c921add975fab6f669`. Not
  vendored.
- **Leaned on by** `modbus.rs` and `sim.rs`: the 7-byte MBAP header
  (transaction id, protocol id 0, length counting unit id + PDU, unit id;
  §3.1.3 p.5) and port 502 (p.5).
- **How to cite:** `TCP §<x> p.<n>`.

### Modbus Organization, MODBUS over Serial Line Specification and Implementation Guide V1.02 (2006-12-20, 44 pp)

- **Where:** https://www.modbus.org/file/secure/modbusoverserial.pdf (linked
  from https://www.modbus.org/modbus-specifications). Fetched 2026-10-30 with
  WebFetch (curl gets 403), read with `pdftotext -layout`; sha256
  `85a91e278b26d68c94a2f61a5300cd39c9e4c855034833f391abc6541fbdf7e5`. Not
  vendored: Modbus Organization copyright.
- **Leaned on by** `crates/helmsman-io/src/rtu.rs` and `rtu/sim.rs`:
  - unicast and broadcast; broadcast requests are writes and get no reply
    (§2.1 p.7); address 0 broadcast, 1-247 units, 248-255 reserved (§2.2 p.8);
  - the master's response time-out, which keeps running past a reply from an
    unexpected slave, and the turnaround delay after a broadcast, typically
    100-200 ms (§2.4.1 pp.9-10); a slave sends nothing for a frame in error
    or not addressed to it (§2.4.2 p.10);
  - RTU characters of 11 bits: start, 8 data LSB first, parity, stop; even
    parity the required default; no parity takes 2 stop bits (§2.5.1 p.12);
  - the RTU frame (address, function, data, CRC low byte then high byte) and
    its 256-byte maximum (§2.5.1 p.13);
  - frames separated by at least t3.5 of silence, a frame with a silence over
    t1.5 inside it discarded, and fixed 750 µs / 1.750 ms above 19200 baud
    (§2.5.1.1 p.13);
  - the CRC field and its byte order (§2.5.1.2 pp.14-15); the CRC procedure,
    preset 0xFFFF and polynomial 0xA001 (§6.2.2 pp.39-40) and the worked
    example (frame 02 07 gives register 0x1241, sent as 41 12, p.41);
  - 9600 and 19200 baud required, 19200 the default (§3.2 p.20).
- **Where we differ, and why:** the receiver does not apply the t1.5 rule
  unless the device says `timing=strict`. A host behind a USB serial bridge
  timestamps bytes in bursts several milliseconds apart (the bridge's latency
  timer), which the rule would throw away; the CRC and the hunt for the
  expected unit and function do the framing instead. The transmit side
  always keeps t3.5 of silence before a frame.
- **How to cite:** `Serial §<x> p.<n>`.

### Linux GPIO character device, uAPI v2 (kernel documentation)

- **Where:** https://docs.kernel.org/userspace-api/gpio/chardev.html, read
  2026-10-30. GPL-2.0 kernel documentation; nothing copied.
- **Leaned on by** `crates/helmsman-io/src/board/linux.rs` through the
  `gpiocdev` crate: chips are `/dev/gpiochipX` ("Chip"), lines are requested
  with `GPIO_V2_GET_LINE_IOCTL` and read and driven through the request's
  file descriptor ("Line Request", "Get Line Values", "Set Line Values").
  First added in Linux 5.10.
- **How to cite:** the section name, e.g. `chardev "Line Request"`.

### Linux I2C device interface, `/dev/i2c-N` (kernel documentation)

- **Where:** https://docs.kernel.org/i2c/dev-interface.html, read 2026-10-30.
- **Leaned on by** `board.rs` and `board/linux.rs` (through `i2cdev`):
  adapters are named `i2c-%d`, all 256 minor numbers reserved for I2C
  ("Implementing I2C device drivers in userspace"); `I2C_RDWR` does a combined
  write-then-read "without stop in between", which plain read()/write()
  cannot ("Full interface description", "C example"). The register read is
  that combined transfer.
- **How to cite:** the section name.

### i2cdetect(8), i2c-tools (Debian trixie manual page)

- **Where:** https://manpages.debian.org/trixie/i2c-tools/i2cdetect.8.en.html,
  read 2026-10-30.
- **Leaned on by** `board.rs`: the 7-bit addresses an `i2c:` point accepts,
  0x08-0x77, are the range i2cdetect scans by default ("default: from 0x08 to
  0x77"); the others it calls "non-regular addresses".

### Logix 5000 Controllers Data Access, programming manual (publication 1756-PM020)

- **What:** Rockwell, *Logix 5000 Controllers Data Access*, publication
  1756-PM020I-EN-P, September 2025, 92 pp. A copy Nick downloaded
  (2026-10-31) lives outside the repo in `~/github/helmsman-refs/`
  (vendor copyright: never committed). Cite as `PM020 p.<n>` (printed
  page numbers).
- **Checked against it (2026-10-31), byte-exact in
  `helmsman-proto/tests/logix_pm020.rs`:** Read Tag request and DINT reply
  (p.18); Read Tag Fragmented requests at byte offsets 0 and 490, the
  offset in bytes whatever the type (pp.20-21); Multiple Service Packet
  request and reply to the Message Router (class 2, instance 1), offsets
  counted from the start of the request/reply data (pp.38-39); the
  Symbol object (class 0x6B, attribute 1 name, attribute 2 type,
  Get_Instance_Attribute_List 0x55, status 0x06 = more instances, instance
  ID u32 then name as UINT length + bytes then the type WORD; pp.44-49);
  the Symbol Type word: bit 15 structure (bits 0-11 the template
  instance), bit 12 system, bits 13-14 dimensions (p.50); the data type
  codes (BOOL 0x0nC1 ... LINT 0x00C5; p.12); the error codes 0x04, 0x05,
  0x06, 0x13 and 0xFF/0x2105 (pp.19-20, 26). BOOL read as 0 / 0xFF on
  5570/5370 firmware v29 and earlier (p.18) is handled (any non-zero is
  true).
- **Not in PM020, still from pycomm3:** attribute 8 (array dimensions) of
  the Symbol object (PM020's examples ask for 1 and 2 only); the
  Unconnected_Send wrapper and the backplane route bytes: PM020 p.15 sends
  network-specific delivery to the ODVA CIP Networks Library Vol. 1 ch. 3
  and Vol. 2, which are not in hand. Real-controller verification waits for
  a CompactLogix on the bench (TODO `logix-tag-read`).

### Logix 5000 Controllers Import/Export, reference manual (publication 1756-RM014)

- **What:** Rockwell, *Logix 5000 Controllers Import/Export*, publication
  1756-RM014D-EN-P, September 2025, 385 pp. Same location as PM020, not
  committed. Cite as `RM014 p.<n>`.
- **Checked against it (2026-10-31):** what `helmsman_iec::l5x` reads is
  the documented shape: the `RSLogix5000Content` root holding version and
  export information around one `Controller` (pp.37-38, 55);
  `SoftwareRevision` (p.99); `ProcessorType` (p.57); tag `TagType` with
  `Base` / `Alias` and `AliasFor` (p.106); `Usage` (`Input`, `Output`,
  `InOut`, `Public`) and `ExternalAccess` (`Read/Write`, `Read Only`,
  `None`), `Constant`, `Dimensions` (p.107); `DataTypes` (p.37ff.);
  `MainRoutineName` (p.196); `RLLContent` / `Rung` (pp.206-208);
  `STContent` / `Line` (p.264ff.); tasks and `ScheduledProgram` (p.313).
- **Not in RM014's text:** the root's `TargetName` and `TargetType`
  attributes (they are in real exports and in the `l5x` package; RM014's
  root example is not in the extracted text). Still to do: run the reader
  over a real Studio 5000 export (TODO `l5x-real-export`).
- **Leaned on by `helmsman_iec::ladder` (2026-10-31):** decorated tag
  data, `DataValue`, `Array`/`Element Index`, `Structure`/
  `DataValueMember` and `ForceValue` (pp.43-46); rung neutral text, a rung
  ends with `;`, branches `[leg,leg]` with empty legs allowed (pp.209-211);
  the neutral text of every converted instruction, e.g.
  `TON(timer,preset,accum)`, `CPT(destination,expression)`,
  `COP(source,destination,length)`,
  `JSR(routine_name,input_1,...,return_n)` (pp.211-221). The order a rung
  runs in (left to right; a branch's legs top to bottom, ORed) is not
  stated in RM014 or RM003; it is Logix's documented ladder model in the
  Ladder Diagram programming manual (1756-PM008), not in hand.

### Logix 5000 Controllers General Instructions, reference manual (publication 1756-RM003)

- **What:** Rockwell, *Logix 5000 Controllers General Instructions*,
  publication 1756-RM003Z-EN-P, September 2024, 905 pp. Rockwell's
  literature site answers 403 to scripts; the copy read is the official
  PDF as Rockwell's distributor NHP hosts it
  (https://www.nhp.com.au/public/assets/pim/Original/10045/Allen-Bradley-Logix5000-Controllers-General-Instructions-User-Manual.pdf,
  fetched 2026-10-31, sha256
  f7016d7c2d9cdbf8c7910935db3b16b61d6f5ce82181ea9462ec74dd9bc7c409), kept
  with PM020 and RM014 in `~/github/helmsman-refs/1756-rm003_-en-p.pdf`,
  not committed. Printed page numbers equal PDF page numbers. Cite as
  `RM003 p.<n>`. The flow charts are images: read rendered, not as text.
- **Leaned on by `helmsman_iec::ladder` (its `lib.st` and the converter),
  and by the reference evaluator in `helmsman-iec/tests/ladder_equiv.rs`:**
  - XIC, XIO (pp.80-85), OTE (pp.76-77; prescan and postscan clear the
    bit), OTL (pp.77-78), OTU (pp.79-80). The OTU execution table on p.80
    says "the data bit is set to true", contradicting its own text on
    p.79 ("clears (unlatches)"); the text is taken.
  - ONS (pp.65-67 and its flow chart p.67): rung false clears the
    storage bit and the rung; rung true with the bit clear sets it and
    passes the rung, with it set makes the rung false; prescan sets it.
    OSR (pp.72-73, flow chart p.73) and OSF (pp.67-69, flow chart p.69):
    the rung passes through; storage and output bits as charted; prescan
    sets OSR's storage bit, clears OSF's, clears both outputs.
  - TON (pp.119-122, flow chart p.122), TOF (pp.111-114, flow chart
    p.114), RTO (pp.103-106): the TIMER members (pp.104-105,
    111-112, 119-120); time base 1 ms (p.87); `ACC = ACC +
    (current_time - last_time_scanned)` (pp.105, 112-113, 120); a
    negative PRE or ACC is major fault 4/34 (pp.105, 113, 121); overflow
    pins ACC at 2147483647; the rung-false and prescan rows. ACC is not
    clamped to PRE: the flow chart sets DN on the scan ACC reaches PRE and
    stops timing, so ACC may pass PRE by part of a scan. **Not verified:**
    RTO's flow chart is missing from p.106 (the heading has no figure);
    its rung-true path is taken from TON's chart, which RTO's description
    (p.105) matches.
  - CTU (pp.92-96, flow charts pp.95-96), CTD (pp.87-91, flow charts
    pp.90-91): the COUNTER members (pp.88-89, 93); counting on the
    false-to-true edge held in CU/CD; on wrap, OV (CTU) or UN (CTD) is
    set unless the opposite flag was set, in which case both clear; DN =
    ACC >= PRE, updated only while OV and UN are clear (both charts; CTD's
    member table on p.88 describes DN as "when clear indicates ...
    complete", which its own chart does not do; the chart is taken).
    Prescan sets CU/CD (pp.89, 94).
  - RES (pp.101-102): ACC to 0 and the status bits false (all of a
    COUNTER's: CU, CD, DN, OV, UN; p.97's example says "clears the status
    bits").
  - Compares (pp.319-388): the v36 renames EQU->EQ, GRT->GT, GEQ->GE,
    LES->LT, NEQ->NE, LIM->LIMIT, MOV->MOVE (pp.320, 331, 338, 348, 364,
    379, 482; p.356 says "from LES to LE", a typo for LEQ); EQ is false
    and NE true on a NaN (pp.324, 383); LIMIT's circular range when
    Low > High and false on a NaN (pp.364, 368).
  - Math: ADD (pp.395-400), SUB (pp.433-438), MUL (pp.417-421), DIV
    (pp.405-410: integer results truncated; by zero, Source A for
    integers and infinity ("1.$") for floats, a minor fault), MOD
    (pp.411-416: `Dest = Source A - (truncate(Source A / Source B) *
    Source B)`, by zero 0 (p.416, stated under the FBD function and taken
    for all)), CPT (pp.401-404: REAL when any operand or the destination
    is REAL; operator order, equal order left to right).
  - Conversions (pp.849-856): integer immediates are DINT, non-decimal
    radixes zero-filled to 32 bits (pp.850-851, 855); operands rank SINT <
    INT < DINT < REAL (p.851); SINT/INT sign-extend (pp.851-852); REAL to
    integer rounds to the nearest, ties to even, and keeps the low bits
    (pp.853, 855-856); narrowing keeps the low bits (pp.853, 855); integer
    overflow wraps (S:V example, p.849). Pseudo-operands (PRE, ACC of
    timers and counters) are initial values written at download (pp.856-
    858). MOVE copies with these conversions (pp.482-485). CLR sets 0
    (pp.473-475). COP is a byte copy of Length destination elements,
    clamped to the destination tag's end, and on 5x80 controllers also to
    the source's (p.510; the converter refuses a length past the source
    so the controller family does not matter); an overlapping forward copy
    replicates (p.512, example 5).
  - Array subscripts out of range are major fault 4/20 (p.864); bit
    addressing `tag.n` (p.864).
  - JSR (pp.622-627): `JSR(RoutineName,InputCount,...)` (p.623, so
    `JSR(Name,0)` is a JSR without parameters); a JSR may not
    call the main routine (p.626); at most 25 nested subroutines (p.627);
    rung false does nothing, rung true runs the routine (p.627); prescan
    runs every called routine once, ignoring RET (p.627).
- **Not in RM003 / not verified:** what `/` and `MOD` by zero do inside a
  CPT expression (the converter applies DIV's and MOD's rules); the sign
  of a float division by zero and 0/0 (IEEE is used: ±inf, NaN); a REAL
  too large for any integer (the converter stores 0, the low bits of its
  integer value); whether ADD/SUB/MUL with integer sources and a REAL
  destination compute in REAL (the converter does, by CPT's rule on
  p.401 and DIV's footnote on p.410); the rung condition after a JSR
  (refused unless JSR ends the rung); p.853's note that Logix Designer
  reads literal 2 as a REAL on 5x70 controllers (the converter follows
  p.855: immediates are DINT). Real-controller checks wait for the bench
  CompactLogix and a real export (`l5x-real-export`).

### Model Context Protocol specification, revisions 2026-07-28 and 2025-11-25

- **Where:** https://modelcontextprotocol.io/specification/2026-07-28 (the
  `latest` redirect on 2026-10-30) and
  https://modelcontextprotocol.io/specification/2025-11-25, read as the
  site's Markdown (`<page>.md`) on 2026-10-30. Pages read: `changelog`,
  `basic/index` (messages, `resultType`, error codes, `_meta`
  per-request fields, statelessness), `basic/versioning` (dual-era
  servers, compatibility matrix), `basic/transports` and
  `basic/transports/streamable-http` (POST-only endpoint, `Origin` check,
  `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` headers and the
  Base64 sentinel, `-32020` header mismatch, 405 for GET and DELETE,
  backward compatibility), `server/discover`, `server/tools` (list and
  call shapes, tool errors vs protocol errors, stateful tools by handle),
  `server/utilities/caching` (`ttlMs`, `cacheScope`), `deprecated`, and
  for the legacy era 2025-11-25 `basic/lifecycle` (`initialize`) and
  `basic/transports` (sessions optional, `MCP-Protocol-Version` after
  initialization). The `claude-api` skill's MCP material covers the
  Messages API's MCP *client* side only, so the spec was the source.
- **Leaned on by** `crates/helmsman-plc/src/api/mcp.rs` (`box-mcp`):
  served revisions `2026-07-28` (stateless, per-request `_meta`) and
  `2025-11-25` / `2025-06-18` (after `initialize`, no session minted);
  JSON responses only (no SSE), no batches, no `subscriptions/listen`,
  no resources or prompts.
- **How to cite:** `MCP 2026-07-28 <page> § <heading>`, e.g.
  `MCP 2026-07-28 basic/transports/streamable-http § Server Validation`.

### PKWARE, APPNOTE.TXT, .ZIP File Format Specification, version 6.3.10 (2022-11-01)

- **Where:** https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT,
  read 2026-10-31. Sections leaned on: §4.3.7 local file header, §4.3.12
  central directory file header, §4.3.16 end of central directory record,
  §4.4.2 / §4.4.2.2 version made by (upper byte 3 = UNIX, so the
  external attributes' high 16 bits are a Unix mode), §4.4.4 general
  purpose flag (bit 0 encrypted, bit 11 the UTF-8 language encoding
  flag), §4.4.5 compression method (0 stored, 8 deflate), §4.4.15
  external file attributes.
- **Leaned on by** `crates/helmsman-plc/src/project/zip.rs` (project
  download and upload): writes stored entries only; reads stored and
  deflate; refuses ZIP64, multi-disk, encrypted, symlink and non-UTF-8
  entries rather than interpret them.
- **How to cite:** `APPNOTE 6.3.10 §<n>`.

## Device sources

### Moxa ioLogik E1213 EDS, v2.0 (EtherNet/IP Electronic Data Sheet)

- **What:** `moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds`,
  647 lines, 42 906 bytes, SHA-256
  `3895fbca8bb036c1b31a9d6b68dea69eca6892fe0a6bce7409ab76311cfd470d`.
  EZ-EDS 3.27 generated; `[File]` Revision 2.0, ModDate 03-16-2026; device
  revision 4.1, "corresponds to firmware v4.0" (l.20).
- **Where:** Moxa's "EDS file software package" for the E1213/E1213-T, which
  Nick downloaded (2026-10-30) and keeps untracked at the repo root.
- **License:** none stated. The file has no header beyond the generator's
  line and no terms; Moxa publishes it for download, but nothing says it
  may be redistributed. **Not committed** (TODO.md `e1213-eds-license`):
  `scripts/eds-corpus.sh` checks the hash and runs
  `crates/helmsman-io/tests/eds_corpus.rs` against it, which asserts that
  it resolves to the built-in profile. The committed fixture
  `crates/helmsman-proto/tests/fixtures/e1213-facts.eds` is our own
  restatement of its facts (identity, params, assemblies, connections),
  written for this repository.
- **Leaned on by:** `helmsman_proto::eds` (the lexical form and the field
  order of `ParamN`, `AssemN`, `ConnectionN`, with `OpENer`'s EDS below),
  `DeviceConfig::moxa_e1213`, `enip::Spec::moxa_e1213`, and every EDS fact
  in `docs/devices/moxa-iologik-e1213.md`.
- **How to cite:** `E1213 EDS v2.0 l.N` (line of the file above).

### Moxa, ioLogik E1200 Series User's Manual, Version 15.10 (May 2021)

- **What:** the E1200 family's manual: models and channels (pp. 1-2, 2-4 to
  2-6), the EtherNet/IP setting and its connection limit (pp. 3-26, 3-27),
  activation (ch. 5), RPI advice (p. 6-10), the Modbus/TCP map per model
  (Appendix A, the E1213 on p. A-7), the EtherNet/IP assembly data map per
  model (Appendix B, pp. B-1 to B-2), port numbers (Appendix D), factory
  defaults (Appendix E). 10 137 362 bytes, SHA-256
  `f8788816b2e0062a68f9b7f1ef9850a2d57f92946ac8b64077c4e77097744454`.
- **Where:** fetched 2026-10-30 from
  `https://pct.wiki.uib.no/images/2/29/Moxa-iologik-e1200-series-manual-v15.10.pdf`
  (a university wiki's copy); Moxa's own links to v15.8 and v15.2 answered
  with an HTML page, not the PDF. Not committed (© Moxa).
- **Leaned on by:** what the E1213's 51 input bytes and 1 output byte mean
  (`helmsman_adapter::e1213`, the device doc), the profile's channel counts
  and connection limit, the Modbus example map. Bit positions within the
  status words and the counters' byte order are inferred, and say so.
- **How to cite:** `E1200 manual v15.10 p. X-N`.

### OpENer sample application EDS (`data/opener_sample_app.eds`)

- **What:** the EDS for `OpENer`'s POSIX sample, the layout
  `DeviceConfig::opener_sample` copies. 359 lines, SHA-256
  `0e0da5ddd62f571ea319dc29254be08cac5134236b389174983d80f0e370760f`.
- **Where:** EIPStackGroup/OpENer at
  `0e1ab7ee5c272950c0dc8daf7793008d7a518912` (the commit
  `docker/opener/Dockerfile` builds). License: OpENer's adapted BSD
  (above). Fetched by `scripts/eds-corpus.sh`, not committed.
- **Leaned on by:** `helmsman_proto::eds`: a second generator's output
  (EZ-EDS 3.23) for the lexical form; blank sizes taken from the assembly;
  configuration assemblies; listen-only connections; and the meaning of
  real-time formats 0, 3 and 4 (its connection words are the E1213's, and
  `OpENer`'s code shows what each direction carries).
- **How to cite:** `OpENer opener_sample_app.eds@0e1ab7e l.N`.

## Test corpora (fetched, not vendored)

### Beremiz and OpenPLC Editor example projects (PLCopen TC6 XML)

- **What:** `plc.xml` files saved by the editors themselves: Beremiz's
  `exemples/` and `tests/projects/` (44 files) and the OpenPLC Editor's
  `examples/`, `exemples/` and `tests/projects/` (43 files). Ladder (Blink,
  Arduino_*, MQTT_*, TCP_socket), FBD (Traffic_Light_FBD, STM32CAN_*), ST,
  SFC and IL, as real users draw them.
- **Where, pinned:**
  - https://github.com/beremiz/beremiz at
    `d7ddfb7523a53b153d74950c06c0447118f434c0`. License: GPL-2.0 (IDE,
    `COPYING_for_IDE`) / GPL-3.0 (GitHub's reading of the repo; C runtime
    `COPYING_for_C_Runtime`). The project files carry no header of their own.
  - https://github.com/thiagoralves/editor at
    `6a8e930a914e4421e8fa05966b31eb371b439243`, the `editor` submodule of
    https://github.com/thiagoralves/OpenPLC_Editor at
    `2b4add40ec9bdb84d0768503ce26a6b7517268ba`. License: GPL-2.0.
  - Fetched 2026-10-30; every file's SHA-256 is in
    `scripts/plcopen-corpus.sh`.
- **Not vendored:** GPL data in an MIT repository is a question for Nick
  (TODO.md `plcopen-fixture-license`). `scripts/plcopen-corpus.sh` fetches
  the files into `target/plcopen-corpus/`, checks their hashes, and runs
  `crates/helmsman-iec/tests/plcopen_corpus.rs` against them. Nothing in
  them is executed.
- **Leaned on by** the PLCopen importer and the ST compiler: 2026-10-30 they
  exposed a nondeterministic lowering order, `BOOL := 0` refused, feedback
  through function block instances refused, located initial values refused,
  and stack overflows on hostile nesting; see the PR that added the script.
- **How to cite:** `Beremiz <path>@d7ddfb7` or `OpenPLC Editor <path>@6a8e930`.

## What this means for the project

1. **Role:** build the originator (explicit client + Class 1 scanner), not
   another adapter. The market already has two BSD adapters and a dozen
   commercial ones.
2. **Reference material for the wire format:** OpENer `cipconnectionmanager.c`
   and `cipepath.c` for parsing, EIPScanner's implicit example for what to
   send, PUB00213 glossary and Appendix A for vocabulary and EDS.
3. **Test targets:** OpENer POSIX sample in Docker (adapter), EIPScanner
   (second originator for interop), Wireshark CIP dissector as the oracle.
4. **Adversarial list, free of charge:** CIPster's July 2026 security PRs
   (#50, #54, #55, #57, #58) and PUB00213 p.13. Every one of those is a
   property test on the parser.
5. **Naming and legal:** no "EtherNet/IP" or "CIP" in the project name or
   artefacts; ODVA's okay in writing; README disclaimer that users shipping
   devices need their own ODVA Terms of Usage and Vendor ID (PUB00213 p.27).

Language choice and whether the first deliverable is a library, a CLI, or a
daemon on a Pi/Jetson are open questions on the TODO board.
