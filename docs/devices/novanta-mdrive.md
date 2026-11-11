# Novanta IMS Liberty MDrive (Lexium MDrive Ethernet)

An integrated stepper motor, drive and controller with one Ethernet
interface. Not on the bench yet (due the week of 2026-11-10, TODO.md
`mdrive-bringup`): everything here is read from Novanta's files and
pages, cited in docs/SOURCES.md ("Novanta IMS MDrive files"), and nothing
has been tried against a unit.

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

## First run on the bench

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
