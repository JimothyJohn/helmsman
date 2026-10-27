# Interop targets

What the originator has to talk to, what each end supports, and what has
been verified on the bench. "Verified" means a packet capture or a passing
test in this repo, nothing less.

| Target | Role for us | Status |
|---|---|---|
| OpENer POSIX sample in Docker | Adapter under CI | **Verified 2026-10-27**: `make opener-up && make test-interop`, 5/5 (ListIdentity, Identity attributes 1–7 and Get_Attributes_All, error codes 0x14/0x08/0x05, encapsulation 0x64). Pinned to OpENer 0e1ab7e. |
| Rockwell CompactLogix (model TBD) | Explicit-message server; Logix tag services; Class 1 target via produced tags | Not reachable from the Studio on 2026-10-26 |
| Panasonic FP0H AFP0HC32ET/EP | Explicit UCMM client; Class 1 originator and target | Not reachable from the Studio on 2026-10-26 |

## Bench sweep, 2026-10-26

`cargo run --example discover <broadcast>` with a 3 s window on both Studio
subnets:

| Broadcast | Found | Rejected |
|---|---|---|
| 255.255.255.255 | 0 | 0 |
| 192.168.3.255 | 0 | 0 |
| 10.0.0.255 | 0 | 0 |

Neither device is on a Studio-reachable subnet right now, or neither is
powered. First bench task is to get one of them answering `ListIdentity`.

## Rockwell CompactLogix

Model, firmware, and IP are unknown. Fill in when it's on the bench.

What is standard CIP and what is Rockwell-specific matters here, because the
project's scope is the ODVA-published protocol:

- **Standard:** Identity object over UCMM (`Get_Attribute_Single`,
  `Get_Attributes_All`), `ListIdentity`, Forward_Open to a produced tag.
- **Rockwell-specific:** symbolic tag read/write (services 0x4C/0x4D/0x4E,
  symbol segments in the EPATH), the Template object, and the Logix
  connection path through the backplane (port 1, slot 0). pycomm3
  `LogixDriver` is the reference for these and documents them as Logix
  extensions; they are *not* in PUB00213R0. They belong behind a feature
  flag or a separate crate, not in the core.

## Panasonic FP0H

Source: Panasonic *FP0H User's Manual (EtherNet/IP)*, WUME-FP0HEIP-07
(2024.4), covering AFP0HC32ET and AFP0HC32EP. Section numbers from the manual.

- **Roles (§1.1):** "The PLC (FP0H) can be set as the both originator and
  target." PLC-to-PLC connections are Input Only in each direction; against
  I/O devices the FP0H is the originator and may use Exclusive Owner.
- **Connection budget (§3.2):** total of plain Ethernet user connections
  plus EtherNet/IP connections is 9 or fewer. With the default 5 user
  connections, 4 are left for EtherNet/IP. Our originator consumes one of
  those four per Forward_Open.
- **Ports:** cyclic data on UDP 2222; connection setup and messaging on
  44818. Both match PUB00213R0 glossary.
- **Heartbeat:** for Input Only / Listen Only the FP0H sends a zero-length
  heartbeat at 16 × the target's RPI. If we are the target, expect that.
- **Timeout:** 4 to 512 × RPI, selectable; "Connection Timeout" setting
  needs unit firmware 1.7 or later.
- **Data size:** max 504 bytes per connection.
- **Explicit messaging as a client (firmware 1.80+):** ladder instructions
  F495 EIPMSATT, F496 EIPMBODY, F497 EIPMSEND, F498 CIPMSET, F499 CIPMGET
  send UCMM requests; the manual's worked example reads the Identity
  object's product code from a target at 192.168.1.10. Whether the FP0H
  *answers* UCMM requests from us as an explicit-message server is not
  stated in the sections read and must be verified on the bench.
- **Error codes:** the manual's error table includes CIP general status
  0x0114 (vendor/product code mismatch), 0x0115 (device type mismatch), and
  0x0204 to 0x0207 (unconnected request timeout, parameter error, message
  too large, acknowledge without reply). Those map directly onto the
  electronic-key and Unconnected_Send paths our originator will exercise.
- **Model check:** the answer on the board was "FP0H or something I think."
  Only the Ethernet-type control units (AFP0HC32ET / AFP0HC32EP) have
  EtherNet/IP; a non-ET/EP FP0H does not.

## What each target proves

| Milestone | OpENer | CompactLogix | FP0H |
|---|---|---|---|
| ListIdentity discovery | **passes** | bench | bench |
| RegisterSession + UCMM Get_Attribute_Single(Identity) | **passes** | bench | bench, if it serves UCMM |
| Forward_Open Class 1, Input Only | CI | produced tag | PLC-to-PLC Input Only |
| Forward_Open Class 1, Exclusive Owner | CI | n/a | n/a (FP0H as target is Input Only) |
| Electronic key mismatch handling | CI | bench | error 0x0114/0x0115 |
