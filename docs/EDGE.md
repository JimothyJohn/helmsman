# Edge node: Helmsman on an ESP32-POE2

An evaluation, written 2026-11-01, of running the IEC runtime bare-metal on
an Olimex ESP32-POE2 (no Linux, no Pi) as a small soft PLC driving remote
I/O over Modbus TCP, with the editor and the agent on a laptop. A roadmap
item, not scheduled; the cards are at the end of `TODO.md` (ids
`iec-no-std`, `node-scan-bench`, `node-modbus`). Everything measured here
was measured on `dev` at 37bdbc2; everything about the board comes from
the Olimex product page and from supervictor's `HARDWARE.md`, both cited
in the last section.

> **Unplug Ethernet before you plug in USB.** The ESP32-POE2 has no
> galvanic isolation. PoE and USB at the same time put the PoE switch's
> ground and the laptop's ground on one board and can damage the board,
> the laptop port or the switch port. Flash over USB, unplug USB, then
> plug in Ethernet. Never both. A USB isolator is the only exception.
> (Olimex's own page says the same; supervictor `HARDWARE.md` §4.2 and
> its `rack-flash.sh` enforce the order.)

## The short answer

Yes, with one hard constraint and two decisions.

- **The interpreter ports.** `helmsman-iec` has no dependencies and
  touches `std` in five places that all have `core`/`alloc` equivalents,
  plus one that does not: the REAL math builtins (`SQRT`, `LN`, `SIN`, ...)
  and the `round`/`trunc` used by REAL to integer conversion are `std`
  inherent methods with no `core` counterpart on the current toolchain
  (checked below). That is a one-day port plus one dependency decision.
- **Compute is probably fine, not certainly.** The 4 000-statement bench
  scan takes 58 µs on the Studio. A 240 MHz Xtensa with software 128-bit
  integers and software double-precision floats is somewhere between 40
  and 100 times slower on this kind of code, so 2.5 to 6 ms per scan of
  that bench, against a 5 ms target. A real cell program is a tenth of
  that bench's statement count. The bench on the board decides.
- **Memory is the real gate, and it decides the module.** The compiled
  form of a program is big: about 200 KB fixed (the parsed and checked
  standard library) plus roughly 0.8 KB per source statement, and compiling
  peaks 50 % above that. The batch-mixer starter with its shared blocks
  (534 lines) is 412 KB resident and 618 KB at compile. The ESP32-WROOM
  has 520 KB of SRAM in total, of which supervictor's firmware can afford
  176 KB of heap. **A WROOM node cannot compile or hold a realistic program
  as the crate stands.** The WROVER-E, which is what Olimex ships on the
  POE2 today (8 MB PSRAM), can, with the program in PSRAM and the scan
  paying PSRAM latency. Shrinking the compiled form, or compiling on the
  laptop and shipping a compact image, is the follow-on that would bring a
  WROOM into range; neither exists today.
- **The I/O side is a smaller port than it looks.** The Modbus application
  layer in `helmsman-io` is already generic over a one-method `Transport`
  trait and is pure byte work. The TCP client, the error type's
  `std::io::Error` and the synchronous `Driver` contract are the parts
  that do not survive, and the last one is a design point (below), not a
  rewrite.
- **The box's safety story does not move.** A node loses its outputs the
  moment it loses power or Ethernet. The remote I/O block's own comms-loss
  safe state is what protects the machine, as it already is for the Pi
  box. Nothing in this plan puts an interlock on the network.

Cost, for the record: the POE2 is 20.95 EUR at one, 16.76 EUR at fifty,
PoE powered, in a ready-made enclosure-free board. A Pi 5 is a multiple of
that before its PSU, storage, PoE HAT and case. The saving is real; what it
buys is a node that runs one program against a few Modbus blocks, with
everything the box does beyond the scan (store, API, policy, agent, OPC UA,
WASM drivers, the adapter) staying on a laptop or a box.

## What a node is, and is not

```
 laptop (or a Pi box)                         node (ESP32-POE2)
 ┌───────────────────────────┐    deploy     ┌──────────────────────────┐
 │ canvas · editor · agent · ├──────────────►│ helmsman-iec runtime     │
 │ approvals · project store │    (later)    │ fixed scan, watchdog     │
 └───────────────────────────┘               │ Modbus TCP client        │
                                             │ serial / UDP log         │
                                             └───────────┬──────────────┘
                                                         │ Modbus TCP, node is master
                                             ┌───────────▼──────────────┐
                                             │ remote I/O (E1213 or the │
                                             │ simulator), own 24 V,    │
                                             │ own comms-loss safe state│
                                             └──────────────────────────┘
```

- **Node:** one compiled program, the fixed scan with the instruction
  budget, a Modbus TCP client to one or a few remote blocks, a log. For
  the spike the program is embedded in the firmware; a deploy endpoint,
  signed images and A/B slots are the next spike and reuse supervictor's
  OTA and mTLS identity.
- **Not on the node:** the box API, SQLite store, policy and audit, the
  agent loop, OPC UA, the CIP adapter, WASM drivers, bundles, the canvas.
  The node is a scan with a network cable.
- **Field power:** the node powers nothing. Blocks sit on their own 24 V.
  See "Which board" for the one case where the POE2's rail matters.

## Which board

The POE2's distinguishing feature is its 802.3at class 4 budget and the
12 V / 24 V output rail (0.75 A at 24 V, 1.5 A at 12 V, jumper selected,
25 W total to external circuits). The node as scoped here does not use it.
Supervictor chose the ESP32-POE-ISO over the POE2 for four reasons
(`HARDWARE.md` §3): 3000 VDC isolation so USB and PoE can coexist while
debugging, no PSRAM pin remap (GPIO16/17 free, RMII clock on GPIO17 as
hardware-verified since 2026-07), 802.3af so any switch or injector powers
it, and inventory. The same MCU, the same firmware with one feature flag
less.

So the honest recommendation is:

- **Spike on whichever board is on the bench**, with the unplug rule if it
  is a POE2.
- **The POE2 earns its place in the product only if its 24 V rail powers
  the remote I/O block**, making a cell one PoE cable: node, block, and a
  few 24 V sensors on 18 W. That is a real and attractive install story and
  it is worth a card of its own once the spike passes. Until then, prefer
  the POE-ISO for development.
- **Check the module before anything else.** The Olimex page lists the
  POE2 with an ESP32-WROVER-E-N4R8 (4 MB flash, 8 MB PSRAM). Supervictor's
  inventory answer of 2026-10-27 records both of its rack POE2s
  (`poe2-rack-01`, `poe2-wifi-01`) as WROOM, no PSRAM. Read the can label
  or `espflash board-info`. Per the memory numbers above, a WROOM is a
  no-go for anything but a toy program until the compiled form shrinks.

Board facts used here (Olimex page, read 2026-11-01): ESP32 dual-core
Xtensa LX6 at 240 MHz, 100 Mb Ethernet (LAN8720 PHY), PoE class 4
(12.95 to 25.5 W, needs at least 37 V on the line), no galvanic isolation,
single-precision FPU only (so `f64` is software), and the RMII reference
clock on GPIO0 because the WROVER's PSRAM owns GPIO16/17
(supervictor `src/board/poe2.rs`: APLL through clock-output channel 1,
GPIO0 is also the boot strap and the PHY is held unpowered until bring-up).

## What the runtime needs from `std` today

`crates/helmsman-iec` has an empty `[dependencies]` table (proptest is
dev-only). Every `std` use, by grep on 2026-11-01:

| where | what | no_std answer |
| --- | --- | --- |
| `lib.rs`, `trace.rs` | `std::sync::Arc` | `alloc::sync::Arc` |
| `lib.rs` | `std::sync::OnceLock` for the parsed standard library and the per-program `TraceMap` | no `core` equivalent that is `Sync`. Pass the parsed library in explicitly, or parse per compile (112 KB, below). The trace map can be an `Option` built on first traced scan. |
| `lib.rs` | `std::time::Instant` in `Program::scan` (wall clock since first scan) | gate `scan` behind `std`; the node calls `scan_at(image, now)` with its own clock, which is the API the box already uses |
| `state.rs` | `std::time::Instant` in `State::started` (warm restore keeps timers running) | carry the `Duration` the program was at instead of an `Instant`; the one real API change |
| `lib.rs`, `trace.rs` | `std::time::Duration` | `core::time::Duration` |
| `check.rs`, `plcopen.rs`, `ladder/mod.rs`, `trace.rs` | `HashMap`, `HashSet` | `alloc::collections::BTreeMap` / `BTreeSet`; no hashbrown, the crate stays dependency-free |
| `lib.rs` | `std::error::Error` | `core::error::Error` (stable since 1.81) |
| everywhere | `std::fmt`, `std::cmp::Ordering`, `std::str::from_utf8`, `std::mem::replace` | the same paths under `core` |
| `interp.rs` | `f64::{sqrt, ln, log10, exp, sin, cos, tan, asin, acos, atan}` (the REAL builtins), `f64::{round, trunc}` (REAL to integer) | **none in `core`**: a `#![no_std]` crate calling these twelve fails to compile on rustc 1.99.1 (checked). Options: `libm` (pure Rust, the implementation `compiler-builtins` itself carries) as an optional dependency only the `no_std` build enables, or gate the twelve builtins behind `std` and have the checker refuse them on a node. Recommendation: `libm`, optional, so the box build's tree stays unchanged and a node runs the same programs as a box. |

Not in the table because they are fine as they are: `Value::Int(i128)`
and `Value::Real(f64)` compile on Xtensa; they are just slow there (every
DINT add is a 128-bit add, every REAL op is a software double). That is a
G2 matter, not a port matter. `include_str!("library.st")` works on
no_std. `plcopen`, `l5x` and `ladder` are compile-time converters and can
sit behind `std` if their port drags; the node only needs `compile` and
`scan_at`.

`helmsman-io` is a bigger surface and is only partly needed:

| part | state | no_std answer |
| --- | --- | --- |
| `modbus::{pdu, reply, app}` and the `Transport` trait | pure byte work over `Vec<u8>`; `Transport::request(&mut self, pdu: &[u8]) -> Result<Option<Vec<u8>>, Error>` | ports as is |
| `modbus::Client` | `std::net::TcpStream`, `std::io::{Read, Write}`, a `Duration` timeout | a node transport over `embassy-net`'s TCP socket behind a feature |
| `driver::Error::Io(std::io::Error)` | std-only variant | a `Link(String)` or `Link(&'static str)` variant for the node; `Error::is_link` keeps its meaning |
| `driver::Driver` (`parse`, `read`, `write`) | synchronous, blocking | see "Sync scan, async network" |
| `ModbusDriver` | reads one point per request (its own doc comment) | the node batches: one Read Discrete Inputs for every DI, one write for the DOs, per scan. Two round trips per scan, not two per point. |
| `model`, `text`, `local`, `harness`, `wasm`, `rtu`, `enip*`, `logix`, `board` | the box's map, simulator, conformance harness, WASM drivers, serial, EtherNet/IP, Logix, GPIO | not on the node; either `std`-gated or left in a crate the node does not depend on |

Precedent next door: `supervictor-common` is a `no_std` crate with an
`alloc` feature in the same shape proposed here.

## Numbers measured on the Studio, 2026-11-01

Apple M1 Max, `cargo run --release`, `dev` at 37bdbc2. The ESP32 numbers
in the next section are extrapolations from these; the bench on the board
replaces them.

**Scan time** (`cargo run --release -p helmsman-iec --example trace_bench`,
about 4 000 statements per scan, 64 array elements, 256 function-block
calls, one TON):

| tracing | per scan |
| --- | --- |
| off | 57.8 µs |
| 1 scan in 100 | 58.7 µs (+1.5 %) |
| 1 scan in 10 | 57.9 µs (+0.2 %) |
| every scan | 59.2 µs (+2.4 %) |

The scope note this page grew from reported 168 µs for the same bench on a
2.8 GHz Xeon; that number was not reproduced here and is quoted only as a
second host data point.

**Heap** (a counting `GlobalAlloc` around `compile` and `scan_at`; the
harness is not in the repo, it is forty lines and worth adding as a second
example if the card is picked up):

| program | source | resident after compile | peak during compile | library cache | allocations per scan | growth over 10 000 scans |
| --- | --- | --- | --- | --- | --- | --- |
| one-statement `PROGRAM` | 6 lines | 202 KB | 207 KB | 111 KB | 0 | 0 |
| `trace_bench` | 45 lines, ~4 000 statements executed | 235 KB | 262 KB | 112 KB | 515 | 0 |
| batch-mixer starter + `starters/blocks/*.st` | 534 lines, ~350 statements | 412 KB | 618 KB | 126 KB | 12 | 0 |

What the table says:

- **The fixed cost is about 200 KB:** `library.st` (226 lines) parsed and
  kept in the `OnceLock` (111 KB) plus every library block checked into
  the program's code (the other 90 KB of the one-statement program). A
  node could drop the parsed library after compiling, which halves the
  resident fixed cost but not the compile peak.
- **The compiled form costs roughly 0.8 KB per source statement** on top
  of that, and the checker peaks at about 1.5 times the resident size
  while it runs. `Value` is 32 bytes (an `i128` with a tag), tree nodes
  are individually boxed (9 500 allocations to compile the starter), and
  names are `String`s. There is room to shrink this by several times, but
  that is a representation change in the compiler, not a port.
- **Scans allocate nothing net.** The bench's 515 allocations per scan are
  two per function-block call (the argument vector and the staged output
  moves in `interp.rs`), freed within the scan. Zero growth over 10 000
  scans means no leak and no fragmentation pressure from the scan itself.
  On a simple embedded allocator those 515 alloc/free pairs cost real
  time; a fixed call frame would remove them, and it is the first
  optimisation to reach for if G2 is close.

## Extrapolating to the board

Assumptions, stated so the bench can contradict them:

- **Clock and width:** 240 MHz against the M1 Max's performance cores, and
  a 32-bit core doing 128-bit integer arithmetic and software `f64`.
  Interpreter code is branchy, pointer-chasing and allocation-heavy, where
  the big core's advantage is well beyond the clock ratio. A factor of 40
  to 100 is the honest range. From 57.8 µs that is **2.3 to 5.8 ms per
  bench scan**, straddling the 5 ms gate; a 350-statement cell program
  with a 10 ms scan has a comfortable margin either way. With the program
  in PSRAM (the WROVER path) add the PSRAM cache-miss penalty on every
  tree node visit; the bench on both memories is the only way to size it.
- **Heap:** supervictor's classic-ESP32 build runs 80 KB of primary heap
  plus 96 KB in the otherwise unused `dram2` segment, and the comment in
  its `config.rs` explains why not more: the stack is what the linker
  leaves, and 144 KB of heap left 5 KB of stack, which overflowed inside
  the network stack. That firmware also carries TLS and OTA; a node
  without them has more room, but not 400 KB more. Internal SRAM is 520
  KB with about 320 KB addressable as data. **So on a WROOM the
  one-statement program fits and nothing realistic does**; on a WROVER
  the compiled program goes to a PSRAM heap region and the image, the
  variable frame and the network buffers stay internal.
- **The scan budget:** `DEFAULT_BUDGET` is 1 000 000 instructions per
  scan. At the extrapolated speeds that is seconds, not milliseconds; the
  node needs a watchdog on wall time as well (the RTC watchdog supervictor
  already arms before network init is the obvious one), with the
  instruction budget kept as the deterministic second line.
- **Modbus round trips dominate a 10 ms scan.** Two TCP transactions per
  scan to a remote block on a 100 Mb LAN cost low single-digit
  milliseconds each, device dependent. The scan loop must mirror the box's
  rule (`runtime.rs`: a late scan skips the periods it missed and counts
  them as overruns, never bursts), and G4 counts those overruns.

## Sync scan, async network

The box's `Driver` and `Transport` are synchronous: the scheduler thread
blocks in `TcpStream::read` with a timeout. Supervictor's firmware is
Embassy: `embassy-net` runs as a task on the executor, and a socket read is
an `async` call. Blocking inside a task that shares the executor with the
network runner starves the runner, so the node cannot simply wrap
`block_on` around the socket and call it a `Transport`.

Two clean shapes, in order of preference:

1. **An async twin of `Transport`** (`async fn request`) with the
   application layer written once over it, and the scan task awaiting the
   two Modbus round trips between `scan_at` calls. The `app` functions are
   small enough that a second, async copy is acceptable for the spike;
   making them generic over both is the follow-on.
2. **The node drives the network stack itself** from a plain loop, polling
   the stack between scans with no executor at all. Simpler to reason
   about for timing, but it gives up every piece of supervictor's
   bring-up that assumes Embassy.

Either way the scan itself stays a synchronous call into `helmsman-iec`,
which is the point: the interpreter does not learn about async.

## Firmware stack to borrow

All from `../supervictor/supervictor/edge` (read 2026-11-01), pinned as it
pins them so one toolchain serves both repos:

- Toolchain channel `esp` from `espup install --targets esp32`; target
  `xtensa-esp32-none-elf`. The Xtensa target is not in upstream rustc, so
  CI proves `no_std` on `thumbv7em-none-eabihf` instead and the Xtensa
  build is a local step.
- `esp-hal = "=1.0.0"` with `unstable`, `esp-alloc`, `esp-rtos 0.2`
  (`embassy`), `embassy-executor 0.9.0`, `embassy-time 0.5.0`,
  `embassy-net 0.9.1` (`dhcpv4`, `medium-ethernet`, `tcp`), `esp-println`.
- Ethernet: `ph-esp32-mac` pinned to a git commit (crates.io 0.1.1 cannot
  detect the PHY; the MDIO fix is merged upstream but unreleased), plus the
  `esp32` PAC 0.42 for the two registers esp-hal does not expose (APLL
  power-up, the RMII clock-out mux). `src/network/ethernet.rs` is the
  bring-up, `src/board/poe_iso.rs` and `src/board/poe2.rs` the pins, with
  `board-poe2` selecting the GPIO0 clock path.
- Heap layout from `src/config.rs` (80 KB + 96 KB `dram2`), to be re-sized
  for a node without TLS and OTA, and extended with a PSRAM region on a
  WROVER.

The spike copies the board module and the Ethernet bring-up into a new
`crates/helmsman-node` rather than depending on the supervictor crate; the
node binary is excluded from the default workspace members so the host
`cargo test --workspace` never needs the esp toolchain.

## Go / no-go

Answer these, then stop. A failed gate with numbers is a result.

| # | question | pass |
| --- | --- | --- |
| G1 | `helmsman-iec` builds `no_std + alloc` (`--no-default-features`) for `thumbv7em-none-eabihf` in CI, with every existing test, the ruSTy differential tests and the property tests green on `std` | yes |
| G2 | `trace_bench` per-scan time on the board, tracing off, program in internal SRAM if it fits, otherwise in PSRAM with both noted | < 5 ms (stretch < 3 ms) |
| G3 | peak heap over compile plus 10 000 scans for `trace_bench` and for the batch-mixer starter, on the node's actual heap layout | fits with ≥ 25 % headroom; record the SRAM-only figure separately, because today's numbers say it fails there |
| G4 | 10 ms scan with one Modbus TCP read and one write per scan against the E1213 (or the simulator), 10 minutes | overruns < 1 % of scans |
| G5 | pull the node's Ethernet; the block reaches its configured comms-loss safe state within its timeout | yes |

If G2 or G3 fails, record the numbers and stop. The follow-on (a compact
compiled form, or compile-on-laptop) is a different card.

## Phases

**Phase 0, host only (G1).** A `std` feature on `helmsman-iec`, default
on; `#![cfg_attr(not(feature = "std"), no_std)]` and `extern crate alloc`;
the table above, item by item; `libm` optional and only for `not(std)`;
`Program::scan` behind `std`; `State` carries a `Duration`. CI adds
`cargo build -p helmsman-iec --no-default-features --target
thumbv7em-none-eabihf` (a `rustup target add` in the workflow) next to the
existing checks; `scripts/lint.sh` gets the same line. Nothing else in the
workspace changes, and no test changes.

**Phase 1, the board, no network (G2, G3).** `crates/helmsman-node`, not a
default member. Embed the `trace_bench` source and the batch-mixer starter
(blocks concatenated), compile at boot, scan in a loop, print min / mean /
max / p99 per-scan time and `esp-alloc` stats after compile and after
10 000 scans over USB serial. Ethernet unplugged throughout; USB power.
Repeat with the program heap in PSRAM on a WROVER.

**Phase 2, one real block (G4, G5).** The async `Transport` over
`embassy-net`, DI read and DO write batched per scan, DO0 to DO3 looped to
DI0 to DI3 as in the tabletop bench card. Flash over USB with Ethernet
unplugged, unplug USB, plug PoE, read the log over UDP to the laptop. G4
over 10 minutes; then set the block's comms-loss safe state, yank the
node's cable, and time the outputs going safe.

## Out of scope for the spike

Deploy endpoint, signed images, A/B program slots and the power-cut-during-
deploy test (next spike, on supervictor's mTLS identity and OTA). The
EtherNet/IP scanner on the node. Node-to-node signals and output ownership.
A node kind in the canvas and a fleet view. RISC-V parts (C6, P4), the
Pico, enclosures, a PCB. Any field power from the node.

## Risks and open questions

- **WROOM or WROVER** decides G3 before any code runs. Supervictor's two
  POE2s are WROOM; new ones from Olimex are WROVER-E. Check the label.
- **The compiled form is heavy** (0.8 KB per statement, 200 KB fixed,
  1.5× at compile). PSRAM hides it on a WROVER at a speed cost; nothing
  hides it on a WROOM. A compact representation helps the box too and is
  the natural follow-on if the node is worth pursuing.
- **`libm` versus `std`-gated REAL builtins** is the one dependency
  decision; this page recommends `libm` optional under `not(std)`.
- **Sync `Driver` versus async network** is the one design decision; this
  page recommends an async `Transport` twin for the spike.
- **i128 and f64 on a 32-bit core without a double FPU** are the two
  likeliest reasons G2 misses. The bench reports them separately if it
  runs an integer-only and a REAL-heavy program.
- **Std/no_std divergence** is a correctness risk: the differential
  (ruSTy) and property tests must run on the `std` build of the same code
  path the node uses, which is why Phase 0 moves no logic, only imports.
- **GPIO0 is the boot strap and the RMII clock** on the POE2. Supervictor's
  bring-up handles the ordering; nothing else goes on that pin.
- **Distributing logic later** moves compute, it does not remove it. A
  cross-node signal needs an age, a timeout and a safe value when stale,
  and a hard interlock never rides the network.

## Sources

- Olimex, ESP32-POE2 product page
  (`olimex.com/Products/IoT/ESP32/ESP32-POE2/open-source-hardware`), read
  2026-11-01: price, module (WROVER-E-N4R8), PoE class 4, 12 V / 24 V
  rail, 100 Mb Ethernet, no isolation, the USB warning.
- Olimex, ESP32-POE2 user manual (GitHub `DOCUMENTS/ESP32-POE2-user-manual.pdf`),
  as read by supervictor on 2026-10-25: UEXT table, GPIO33 / GPIO0 remap,
  power rails.
- `../supervictor/HARDWARE.md` §2 (inventory, 2026-10-27), §3 (POE-ISO
  over POE2), §4.2 (POE2 differences); `supervictor/edge/Cargo.toml`,
  `src/config.rs`, `src/board/poe2.rs`, `src/network/ethernet.rs`.
- This repo: `crates/helmsman-iec` (`lib.rs`, `interp.rs`, `state.rs`,
  `trace.rs`, `check.rs`, `examples/trace_bench.rs`),
  `crates/helmsman-io/src/{modbus.rs, driver.rs}`,
  `crates/helmsman-plc/src/runtime.rs`, `crates/helmsman-project/starters/`.
- `core` float method availability: a `#![no_std]` crate calling the
  twelve `f64` methods named above, `cargo check` on rustc 1.99.1
  (2026-10-13), twelve `E0599` errors.
