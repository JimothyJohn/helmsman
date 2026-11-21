# Novanta IMS Liberty MDrive (Lexium MDrive Ethernet)

An integrated stepper motor, drive and controller with one Ethernet
interface. A unit is on the bench since 2026-11-16 in EtherNet/IP mode
("On the bench" below is what it did); the rest is read from Novanta's
files and pages, cited in docs/SOURCES.md ("Novanta IMS MDrive files").

## What it speaks

- One Ethernet interface, four protocols, one at a time: Modbus TCP (the
  factory default), EtherNet/IP, PROFINET IO and MCode/TCP ("the device
  cannot be used in multiple modes simultaneously", LMDxE product manual
  V1.00 §1.2.1). The protocol is a setting made with Novanta's TCP/IP
  Configuration Utility (product manual ch. 7).
- Default IP address 192.168.33.1 (product manual §6; the same manual
  gives two different default masks).
- Firmware: the PROFINET application's release notes name a "bug
  impacting Profinet I/O task timing" and a timer rollover fixed in
  2.4.0.7; the newest on Novanta's page on 2026-11-10 is 2.4.0.9.

## PROFINET, from its GSDML

`GSDML-V2.2-mdrive-SchneiderElectic-20130530.xml` (in the firmware zip):

| | |
| --- | --- |
| Vendor / device id | 0x0261 / 0x0001 ("Schneider Electric Motion") |
| Default station name | `mdrive` (`DNS_CompatibleName`) |
| Access point | slot 0, module 0x00000001, one submodule 0x00000001 without data; no interface or port submodules ("Standard, no PDev") |
| Slot 1 | module 0x00000030, submodule 0x0001: 128 bytes of input |
| Slot 2 | module 0x00000031, submodule 0x0001: 128 bytes of output |
| Fastest cycle | `MinDeviceInterval` 256 x 31.25 us = 8 ms |
| Parameter records | none |

The two 128-byte blocks are MCode registers ("uses 128 bytes of I/O data
to execute MCode commands and set parameters in the drive", Novanta's
PROFINET page). **The byte map is not read yet**: it is in Novanta's
PROFINET manual and its S7 workbook, and the endianness of the registers
is not known, so docs/devices/novanta-mdrive-profinet.map names no
signal beyond the raw first bytes.

The device line in docs/devices/novanta-mdrive-profinet.map is what
`scripts/gsdml-slots.py` prints for this file.

## EtherNet/IP, from its files

`SEM_MDrivever1_2.eds` (revision 1.2 of 2012, in the firmware zip) and
the fieldbus manual LMD-EIP-REV-F (02/2022):

| | |
| --- | --- |
| Identity | vendor 243, device type 43, product code 15362, revision 2.22, `MDrive` |
| Input (T->O) | assembly 100, 16 bytes |
| Output (O->T) | assembly 112, 36 bytes |
| Exclusive owner | configuration instance 1, O->T 112, T->O 100, both declared modeless, point-to-point, no RPI given |
| Input only / listen only | heartbeat points 128 / 129 |

The EDS gives its assemblies no size, only members; `helmsman_proto::eds`
takes the size from the members' bits (fixture
`tests/fixtures/mdrive-facts.eds`, and the vendor's file in
`scripts/eds-corpus.sh`).

The manual's layout (p. 20), which the manual also says "may be changed
to any MCode mnemonic of the same data type using the TCP/IP
Configuration Utility":

| assembly | bytes | |
| --- | --- | --- |
| 100 | 0 | bit 0 EF error flag, bit 1 MV moving |
| | 1 | IN, the inputs as a group |
| | 4..7 | C1, position counter |
| | 8..11 | C2, encoder counter |
| | 12..15 | LL lead/lag (the EDS says V, velocity) |
| 112 | 0..3, 4..7 | A, D: acceleration, deceleration |
| | 8..11, 12..15 | MA, MR: move absolute, move relative |
| | 16, 17, 18 | RC, HC run and hold current, OT outputs |
| | 20..23 | SL, slew |
| | 24..27, 28..31 | VI, VM: initial and maximum velocity |
| | 32.. | TQ and EF in the manual (34 bytes in all), TE in the EDS (36) |

## On the bench (2026-11-16)

One LMDCE853C at its default address, 192.168.33.1, DC supply on, shaft
free, set to EtherNet/IP by Nick. `PR VR` answers `LMDCM 6.013, Hw:
2.10`. The Studio reaches it through an alias on en0 (`sudo ifconfig en0
alias 192.168.33.10 255.255.255.0`, gone at a reboot).

**The connection**

- **Identity.** Vendor 243, device type 43, product code 15362, product
  name `MDrive`, revision **2.36** (the EDS and the manual say 2.22).
- **Assemblies** that answer: 100 (16 bytes) and 112 (36 bytes), nothing
  else. Attribute 4 (the size) is not supported.
- **It takes a run/idle header O->T, which its EDS does not declare.** A
  `Forward_Open` at the EDS's size (38 with the sequence count) is
  refused 0x0127 with the size it wants, 42. `enip-io` reads assembly 112
  to see that it is the 36 bytes the EDS says, concludes the four bytes
  are the header, and opens again with it: `eds=` with the vendor's file
  connects. T->O is 16 bytes without a header.
- Any configuration instance is taken (1, 100, 112, 128 tried). The
  input-only and listen-only points of the EDS are refused, 0x012A.
- **It sends less often than it agrees to.** Its reply grants the RPI
  asked for, and its packets come at about 1.3 to 1.5 times that: every
  26 ms at 20, 72 ms at 50, 147 ms at 100, 750 ms at 500, and every
  2 ms at 1. Inside the connection's timeout at every multiplier.
- **It resets the TCP session 2 to 3 s after the `Forward_Open`** and
  goes on exchanging I/O. A `Forward_Close` over a new session is
  accepted. `enip-io` holds the link through it and closes that way
  when the driver goes.
- **It sends its inputs to UDP 2222 only**; a T->O socket-address item
  is ignored.
- MCode/TCP on port 503 answers beside EtherNet/IP (`PR P`, `PR ER`,
  `SL 0`), which is how the numbers below were read back.
- **A hold of 20 minutes** at 20 ms, outputs at zero (`enip_soak`):
  the first 13 s lost the link over and over (191 connections opened that brought no data, after 40 minutes in which nothing had talked to the drive), then one connection held for the remaining 19 minutes with no loss. It happened once and is not explained. Ruled out afterwards, each with the link held and watched: a start after a short pause (clean three times), a start after 40 minutes of silence (first packet after 18 ms, no gap over 42 ms in 30 s), a change of an output field mid-connection, and load on the controller's machine (a build on every core, a test suite). A failed connect is now tried again after a wait, so such a stretch costs a handful of attempts, not 191.

**What the bytes did on this unit**

Input 100: byte 0 bit 1 is MV (moving); bit 0 followed the drive enable
(1 with DE=1, 0 with DE=0) while EF read 0; byte 1 is IN (0x0F);
bytes 4..7 are the position (`PR P`); bytes 8..9 held the error code
(`PR ER`: 30, 94, 107, 25 were seen there), not an encoder count.

Output 112 does **not** follow the manual's layout on this unit: its
mapping had been changed before it reached the bench (Nick,
2026-11-17), and nothing the drive says over EtherNet/IP shows it. A
field is acted on when it changes from the last image the drive saw,
even across connections.

The mapping as the E.I.P. page of Novanta's TCP/IP Configuration
Utility shows it (Nick's photo of 2026-11-17), with what the bench saw
of each field the night before, one field at a time, read back over
MCode:

| assembly | bytes | mnemonic | on the bench |
| --- | --- | --- | --- |
| 100 | 0 bit 0 | DE drive enable | followed `PR DE` |
| | 0 bit 1 | MV moving | followed `PR MV` |
| | 1 | IN inputs | 0x0F, as `PR IN` |
| | 4..7 | P position | equal to `PR P` |
| | 8..11 | ER error code | equal to `PR ER` |
| | 12..15 | LL lead/lag | 0 throughout |
| 112 | 0..3 | P position | set the counter (`PR P` read the value) |
| | 4..7 | MF make-up frequency | not seen to act |
| | 8..11 | VM maximum velocity | not tried with a valid value |
| | 12..15 | ER error code | not tried |
| | 16 | CF clear locked rotor | not tried |
| | 17 | DE drive enable | non-zero enabled, zero disabled |
| | 18 | RC run current | 44 gave `PR RC` 44 |
| | 20..23 | SL slew | 5000 gave 5000 steps/s, 0 stopped |
| | 24..27 | LG lag limit | not tried |
| | 28..31 | MA move absolute | see below |
| | 32, 33 | RD reverse direction, AS hMT mode | not tried |
| | 34..35 | toggle bits (off) | not tried |

The first image sent to the drive was built from the manual's table,
which put 2 500 000 (meant as the maximum velocity) into bytes 28..31
and an enable into byte 17. The drive started a move at the
`Forward_Open` that reached about 1.1 million steps/s, roughly
1300 rpm, before SL stopped it: on this mapping that image reads as
"enable, move to 2 500 000". One thing does not fit yet: a small MA
through the same bytes on 2026-11-17 (4746 steps away, drive enabled)
did not move the drive and left error 25, then 30, and `MA` typed at
the MCode port did the same. Not explained.

The same page shows three settings, as the photo has them (whether
they had been read from the drive with the page's Get buttons is not
known): "I/O Implicit connection tests" with Halt Motion **On** and
Imp. Writes On, a TCP socket inactivity timeout of 0 ("indefinitely"),
and toggle bits mode Off.

**When the controller goes away**

Slewing at 5000 steps/s over the cyclic connection (RPI 20 ms):

| the controller | the drive |
| --- | --- |
| sends `Forward_Close` | stops |
| sets the run/idle header to idle | stops |
| goes silent (no close, no more packets) | **keeps slewing**: still 5000 steps/s after 10 s, with timeout multipliers 0 and 1, until `SL 0` arrived over MCode |

A pulled cable or a controller that dies did not stop this drive,
although the Configuration Utility's page shows Halt Motion on for its
implicit connection tests (above). Whether that setting is really on
in the drive, and what it covers, is not known. Nothing in `enip-io`
can supply that stop. Accepted on the bench for now (Nick,
2026-11-17): before the drive is shown or left running unattended it
needs a stop of its own (TODO.md `mdrive-bringup`). A box that stops normally closes its connections, which does stop
it.

## PROFINET: first run on the bench

Parked (Nick, 2026-11-13: EtherNet/IP and Modbus TCP first). When it is
wanted:

1. `~/github/helmsman/scratch/27-bpf-access.sh` (once per boot, sudo),
   then `~/github/helmsman/scratch/28-profinet-scan.sh`: the drive
   should answer with its name, MAC and ids. No answer: the protocol
   setting is still Modbus TCP.
2. Bring the connection up and watch the input block, outputs at zero:

   ```sh
   cargo run -p helmsman-io --features profinet-raw --example profinet_watch -- \
       mdrive if=en0 ip=192.168.3.70/24 vendor=0x0261 device=0x0001 \
       slots=0:0x1/1:0x1,1:0x30/1:0x1:i128,2:0x31/1:0x1:o128
   ```

   The line it prints while it is not running says what the startup is
   waiting for or what the drive refused.
3. Capture the startup (`sudo tcpdump -i en0 -w mdrive.pcap ether proto
   0x8892 or udp port 34964`, a hand-off script) and keep it: the first
   exchange with a device this project did not write is the regression
   test for everything in helmsman-profinet.

Nothing here moves the motor: outputs stay zero until a program writes
them, and motion asks "cell clear?" first.
