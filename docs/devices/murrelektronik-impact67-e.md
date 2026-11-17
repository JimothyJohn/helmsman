# Murrelektronik IMPACT67 E DIO8 (DIO8), art. no. 55089

An IP67 EtherNet/IP block: eight M12 ports, pin 4 and pin 2 of each an
input or an output (16 channels, 1.6 A an output), two Ethernet ports,
power over 7/8". EtherNet/IP only. The library entry, with every bit of
its assemblies and the handbook's page for each, is helmsman-devices
`devices/murrelektronik/impact67-e-dio8-dio8/`; this page is what the
unit on the bench showed and what `enip-io` learned from it. Sources:
docs/SOURCES.md, "Murrelektronik IMPACT67 E handbook" and "The bench
IMPACT67 E DIO8 and MDrive, 2026-11-16".

## The map line

```text
device i67 enip-io ADDR config=109 config_data=00ff00 o2t=35:2 t2o=117:8 rpi_ms=10
```

`config_data` is the channel setup, sent with every `Forward_Open`: byte
0 bit 0 picks pin mode (0) or port mode (1), byte 1 makes pin 4 of ports
0..7 an output where its bit is 1, byte 2 the same for pin 2 (handbook
pp. 62-63, 70). `00ff00` is pin mode, pin 4 outputs, pin 2 inputs. Left
out, the block keeps the setup it has.

## On the bench (2026-11-16)

One unit as it came out of its box: 24 V on US and UA, one Ethernet
cable, nothing on its ports, an address from DHCP. Revision 1.2.

- **Identity.** `ListIdentity`: vendor 640, device type 7, product code
  55089, revision 1.2, product name `IMPACT67 E DIO8 (DIO8)`, status
  0x0030, state 3. The library had the product code and the type as
  inferences from the EDS file's name; both are right.
- **Assemblies** that answer `Get_Attribute_Single` (attribute 3): 5,
  35, 100, 101, 105 and 135 with 2 bytes, 117 and 118 with 8, 110 with
  none, and 109 with **4** bytes where the handbook says 3 (p. 70). The
  fourth byte reads 0; three bytes of configuration, padded to four on
  the wire as a data segment is, are accepted and read back in place.
  Instances 100 and 101 are not in the handbook's table for this article.
- **Out of its box it is in "Over EDS File" mode** (handbook p. 43), so
  the configuration instance is 109. A `Forward_Open` naming 110, the
  instance for a block set up in its web server, is refused with general
  status 0x01 and extended status 0x0119, followed by sixteen more
  status words: two zeros and fourteen 0x5555. `enip-io` writes that run
  once (`[0000 x2, 5555 x14]`) and says what 0x0119 means here. With 109
  (data or none), or with no configuration segment at all, it opens. Its
  successful reply carries an application reply of 16 or 18 words of the
  same filler. Both replies are tests
  (`the_impact67s_refusal_and_its_padded_reply_read`).
- **Sizes and formats.** O->T is 2 bytes behind a run/idle header (a
  request without the header is refused 0x0127 with the size it wants),
  T->O is 8 bytes without one (0x0128 otherwise). Point-to-point both
  ways.
- **Timing.** Asked for 10 ms it sends every 10 ms: over 300 packets the
  median gap was 9.8 to 10.2 ms and the 99th percentile 12 to 14 ms,
  none lost, and the first packet came 10 to 11 ms after the
  `Forward_Open`.
- **It sends its inputs to UDP 2222 and nowhere else.** A T->O
  socket-address item naming another port is ignored, so a link with
  `io_port=` set gets an open connection and no data. On a box that is
  the shared endpoint on 2222, which is the default.
- **An output channel does not read back as an input.** With pin 4 of
  port 0 set to output and switched on, input bit 0.0 stays 0. A loop
  test needs a wire from an output channel to an input channel.
- **After a `Forward_Close` the output assembly still reads the last
  image** (0x8101 after a run that ended with three channels on), and
  zeros again after the next connection sent zeros. Whether the pins
  themselves stay on after a close or a timeout was not seen: nothing
  was wired and nobody was looking at the LEDs. `enip-io` now sends what
  was written last before it closes, so a box that writes its safe
  values and stops leaves them on the block.
- **A hold of 20 minutes** at 10 ms, toggling channel 00 twice a second
  (`enip_soak`): 120 000 reads, one connection, the link never lost.

## Its EDS (2026-11-17)

Nick downloaded the article's two files from Murrelektronik's shop page
(docs/SOURCES.md, "Murrelektronik IMPACT67 E DIO8 EDS"):
`55089_Impact67_E_DIO16.eds` (EDS revision 1.3, 2017) and
`55089_Impact67_E_DIO16_Omron.eds` (2019). They differ in one thing that
matters: configuration assembly 109 is 3 bytes in the first and 4 in
the second, the fourth reserved, which is the size the unit reports.

- Both agree with the unit: identity 640 / 7 / 55089 revision 1.2, the
  assembly sizes, O->T run/idle and T->O modeless. Neither gives an RPI.
- Six connections: an exclusive owner without and with diagnosis, each
  with configuration instance 109 ("Config over EDS") and with 110
  ("Config over Web Server"), and two listen-only ones.
- `eds=` with either file connects to the unit. The configuration sent
  is the file's parameter defaults (pin mode, every channel an input),
  or `config_data=` of the file's size: `00ff00` for the first,
  `00ff0000` for the second. Both read back in assembly 109.
- From an EDS `enip-io` opens the first exclusive owner, which here is
  the one **without** diagnosis (T->O 5, 2 bytes). The map line at the
  top of this page gives the sizes itself and gets the 8 bytes.
- `scripts/eds-corpus.sh` checks both files when they are at hand.

## Open

- What the outputs do when the controller goes away: the handbook only
  says the network LED flashes red (p. 83). Needs eyes on the block.
- Diagnosis bytes (input bytes 2..7) were zero throughout: no load, no
  short. Untested.
