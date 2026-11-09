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

*Done 2026-11-02 (feat/iec-no-std).* As planned, with three differences.
The `no_std` build is `--no-default-features --features libm`: cargo cannot
turn a dependency on when a feature is *absent*, so `libm` is an optional
dependency the node's build names; the std build still has none. The
wall-clock start (`started: Option<Instant>` on `Program` and `State`) is
behind `std` rather than turned into a `Duration`: it only serves
`Program::scan`, which a node never calls, and `scan_at`'s clock already
comes from the caller. `OnceLock` (the parsed standard library, a
program's trace map) is a small `Once<T>` of the crate's own: std's
`OnceLock` under `std`, a lock-free leak-once `AtomicPtr` cell without.
`HashMap`/`HashSet` became `BTreeMap`/`BTreeSet` throughout (no hashbrown),
which also makes every iteration order deterministic. `scripts/lint.sh`
runs clippy on the thumbv7em build; the CI step is a separate draft PR
(workflow edits go through a draft, per the cloud-security rules).

**Phase 1, the board, no network (G2, G3).** `crates/helmsman-node`, not a
default member. Embed the `trace_bench` source and the batch-mixer starter
(blocks concatenated), compile at boot, scan in a loop, print min / mean /
max / p99 per-scan time and `esp-alloc` stats after compile and after
10 000 scans over USB serial. Ethernet unplugged throughout; USB power.
Repeat with the program heap in PSRAM on a WROVER.

*Built 2026-11-03 (feat/node-scan-bench).* **Numbers, the board, 2026-11-03**
(Olimex ESP32-POE2, ESP32 rev 3.0 WROVER-E, 240 MHz, flash 40 MHz DIO as
the bootloader reports it; after `fix/node-stack`: 128 KiB + 96 KiB
internal heap, 60 784 B main stack):

| round | program | compile | heap for the program | 10 000 scans: min / mean / p99 / max | peak heap |
| --- | --- | --- | --- | --- | --- |
| 1, internal SRAM (229 376 B heap) | bench-4k | 40.9 ms | 158 216 B | 31.8 / 32.0 / 32.3 / 32.8 ms | 198 440 B (13.5 % headroom) |
| 1, internal SRAM | cell (batch-mixer) | out of heap during compile: the allocator's error handler aborts | | | |
| 1, internal SRAM, second flash | bench-4k | 38.7 ms | 158 216 B | 27.9 / 28.1 / 28.3 / 28.9 ms | 198 440 B |
| 2, + PSRAM (4 423 680 B heap) | bench-4k | 19.0 ms | 85 308 B | 27.9 / 28.1 / 28.3 / 28.9 ms | 198 440 B |
| 2, + PSRAM | cell (batch-mixer) | **panic during compile**: "memory allocation of 1061212033 bytes failed" in `check::Ck::coerce` (a `Vec::reserve`) | | | |

**G2 fails**: 32 ms against < 5 ms, about 520× the Studio's 61.8 µs
(a 240 MHz in-order core alone would explain 50–100×). Suspects, not yet
measured: flash at 40 MHz DIO (instruction fetch through the cache), the
interpreter's `i128` `Value::Int` on a 32-bit core, the BTreeMap lookups
the no_std port brought in, and esp-alloc's heap-stats. **G3 fails on
internal SRAM** (13.5 % headroom for bench-4k, the starter does not fit);
with PSRAM the scan time is unchanged (esp-alloc fills internal SRAM
first, so the program likely still sits there). The batch-mixer starter
**crashes compiling with PSRAM**: the size asked for, 1 061 212 033 =
0x3F40D781, is an address in the flash-mapped rodata, so a pointer is
being read as a length: memory corruption on the board, not an
out-of-memory (the same source compiles on the host and in every test).
Suspects, not yet tested: the ESP32's PSRAM cache behaviour and esp-alloc's
PSRAM region. Per the rule above: record and stop; the follow-on (speed,
and the PSRAM corruption) is a separate card.

`crates/helmsman-node` is excluded from the workspace (own Cargo.lock,
`rust-toolchain.toml` channel `esp`, `.cargo/config.toml` with the Xtensa
target and build-std), built by `make node-build` (`scripts/node-build.sh`;
`make node-clippy` for the lint). The firmware embeds the host bench's
workload, now the shared file `crates/helmsman-iec/examples/trace_bench.st`,
and the batch-mixer starter's `main.st` with its six blocks (the plant model
left out) as the "realistic cell"; compiles each at boot, scans it 10 000
times at a 10 ms clock, and prints compile time, heap for the program,
min / mean / max / p99 per scan and heap used and peak (esp-alloc's
`internal-heap-stats`, which costs a little per allocation and is named in
the output). The round runs twice: internal SRAM alone (160 kB in the main
DRAM segment plus 96 kB in `dram2`, supervictor's figures), then with the
PSRAM added as a second heap region (esp-alloc fills internal RAM first),
which gives G3 both figures; the interpreter is not told where to put
what, so "program image in SRAM, scratch in PSRAM" is what the fill order
makes of it, not a placement. The board, read over USB with `espflash
board-info` on 2026-11-02: chip esp32 revision v3.0, 40 MHz crystal, 4 MB
flash, MAC 10:97:bd:e4:1f:40; a WROVER-E module per Nick (board-info does
not report PSRAM; the firmware prints the heap size after the PSRAM region
is added, which is the check). Flashing is `scripts/node-flash.sh`, run by
Nick in his own shell: it prints the unplug rule, asks, builds, flashes
with espflash and tees the monitor into `scratch/node-bench-<utc>.log`.
Host baseline for the same workload on the Mac Studio (M-series, release):
61.8 µs per scan, tracing off (`cargo run --release -p helmsman-iec
--example trace_bench`, 2026-11-03).

**Phase 2, one real block (G4, G5).** The async `Transport` over
`embassy-net`, DI read and DO write batched per scan, DO0 to DO3 looped to
DI0 to DI3 as in the tabletop bench card. Flash over USB with Ethernet
unplugged, unplug USB, plug PoE, read the log over UDP to the laptop. G4
over 10 minutes; then set the block's comms-loss safe state, yank the
node's cable, and time the outputs going safe.

*Built 2026-11-03 (feat/node-modbus). Run on the boards 2026-11-03, then
**parked by Nick the same evening** (TODO.md `node-resume`).* What the runs
showed:

- **Stack.** The first flash panicked on its stack guard right after the PHY
  came up: the 160 KiB internal heap left the main stack 1 704 bytes. Fixed
  in #208 (heap 112 KiB, stack 50 856 B; `scripts/node-build.sh` refuses an
  image under 32 KiB). After it the POE2 boots, gets a DHCP lease (10.0.0.50)
  and runs the chase program against the E1213 over Modbus TCP: DO-00..02
  step every 500 ms, watched from the Studio. **The Modbus loop works on
  hardware.** No G4 number: with 28 ms scans (Phase 1) against a 10 ms
  period, G4 fails regardless.
- **POE2: frames from the Studio never register.** The UDP reports never
  reached the sink. tcpdump on the Studio: the node asks `who-has 10.0.0.41`
  about once a second, the Studio answers each time (`is-at
  7e:28:c7:a7:d6:f3`, its Wi-Fi address on en1), the node never takes the
  answer and never sends a datagram; it also answers no ping. Ruled out: the
  macOS firewall (the sink's python is permitted; a datagram from the Studio
  itself arrives), the UDP bind (embassy-net gives port 0 a free port), the
  report's size (about 250 B in a 512 B buffer), smoltcp's neighbour cache
  (8 entries). The E1213's frames do get in (Modbus works), so it is some
  frames, not all.
- **POE-ISO (WROOM, `NODE_BOARD=poe-iso`, #211): no lease.** The RMII clock
  on GPIO17 works: PHY id 0x0007c0f1, `link up Mbps100 Full`. Then one DHCP
  request (seen by tcpdump on the Studio's wired en0) in about 30 s, no
  retries, no lease. Whether the router's offer reached the board is not
  known (a unicast offer never passes the Studio's switch port).
- **Not the bring-up code.** helmsman-node's `ethernet.rs` matches
  supervictor's hardware-verified one line for line (buffer counts and
  sizes, promiscuous mode while DHCP runs, the ph-esp32-mac commit); the
  EMAC's DMA buffers sit in internal DRAM.
- **Where it points.** The receive path, or the network task stalling after
  its first send, in what surrounds the bring-up: the executor and interrupt
  setup (esp-rtos), or how the main task shares the CPU with the network
  task. Unproven.
- **Next step when resumed.** Flash supervictor's own known-good POE-ISO
  firmware on the same board and port: a lease there puts the bug in this
  firmware; none puts it in the board or the switch.

The
shared part is the codec: `crates/helmsman-modbus` (`no_std`, no deps; PR
#197) holds the PDUs, reply checks and MBAP framing that helmsman-io's
client and the node's client both use, since helmsman-io itself cannot
build without std (helmsman-proto's sockets). The node's client
(`crates/helmsman-node/src/modbus.rs`) is the async twin of helmsman-io's
`Transport`: one embassy-net TCP socket reused across reconnects, a
200 ms timeout on every step, any error dropping the connection so a late
reply is never taken for a newer request's answer. The `modbus` binary
(`net` feature; `make node-build net`, `scripts/node-build.sh clippy net`)
brings the POE2 up on Ethernet with supervictor's recipe copied in
(`board.rs`: the APLL at 50 MHz routed out of GPIO0 through clock-output
channel 1, PHY power on GPIO12; `ethernet.rs`: ph-esp32-mac's EMAC +
LAN8720A over RMII, DHCP after link-up), waits for a lease, then scans
every 10 ms: DI-00..07 in one FC 2 round trip, an embedded ST program (a
one-of-three chase on DO-00..02 every 500 ms, `seen` true when the three
DIs match the three DOs), DO-00..03 in one FC 15 round trip (0x0F is new
in the codec for exactly this). Counted and reported every 10 s over
serial and as a UDP line to the Studio (`scripts/node-log-sink.py`, port
9514; run it with Apple's python so the firewall lets it receive): scans,
late scans (started more than a period after their deadline: G4's
number), scans whose own work overran the period, cycle mean and max,
reads, writes, link errors and the last one, reconnects, matched and
mismatched scans, heap used and peak. `E1213_ADDR` and `LOG_SINK` at build
time change the defaults (10.0.0.57:502, 10.0.0.41:9514).

Run order (`scripts/node-flash.sh modbus` prints it): flash over USB with
Ethernet unplugged, Ctrl-C the monitor, unplug USB, start the sink, plug
PoE. G5 is the block's own comms-loss setting: set the E1213's Modbus
watchdog and DO safe mode on its console (manual pp. 3-4, 3-11, 3-21),
then pull the node's cable and watch the outputs go safe within the
timeout; the node cannot do that part without power or network, so the
test proves the block does it.


*Resumed 2026-11-08, the POE-ISO (poe-iso-rack-03, WROOM).* Split test
first: supervictor's known-good Ethernet firmware on the same board, cable
and port took a lease and uplinked in 11 s, so the fault was ours. Found
by diffing the two firmwares: this crate enabled esp-hal's `psram`
feature for every board, and `esp_hal::init` then brought up a PSRAM the
WROOM does not have over GPIO16/17, GPIO17 being the POE-ISO's RMII clock
pin. With `psram` off for that board (`--no-default-features --features
poe-iso`) the node took a lease at once. Then, on 10.0.0.x, its reports
reached the Studio for 50 s and stopped: the Studio is on that subnet
twice (a wired alias and Wi-Fi) and answered the node through the access
point, and the modbus image dialled a dead E1213 address every scan, each
dial spending the stack's one ARP request per second (smoltcp's neighbour
cache is silent for 1 s after any request, for every address), so the
node could never re-resolve the sink once that entry expired at 60 s.
Fixed: the Modbus client backs off 2 s after a failed connect. The bench
then moved to the Studio link with a **static address**
(`NODE_STATIC_IP=192.168.3.55/24`; most plants commission controllers on
static networks with no DHCP, Nick 2026-11-08): one Modbus TCP connection
to the E1213 (192.168.3.60) held, about 31 reads and 31 writes a second,
the chase on DO-00..02, reports every 10 s to the Studio's wired port,
the first end-to-end node run. The node answered ARP and TCP (a SYN to a
closed port got its RST in 1 s) but not ping: smoltcp answers ICMP echo
only with its `auto-icmp-echo-reply` feature, now on in both node crates.
Numbers to carry: the 10 ms scan ran at a 30 ms mean because the two
Modbus round trips are synchronous inside the scan loop; the node's I/O
moves to its own task with the API work (Phase 4).

**Phase 3, the ESP32-P4 and EtherNet/IP (planned 2026-11-08).** Nick
bought a Waveshare ESP32-P4-ETH with its PoE module (ESP32-P4NRW32: dual
RISC-V at 400 MHz with a double-precision FPU, 768 KB SRAM, 32 MB PSRAM,
32 MB flash, IP101GRI PHY, one Ethernet MAC). What was checked before it
arrived, so the first day on the board is spent on the two real unknowns:

- **Ethernet in Rust.** esp-hal merged basic EMAC support for the classic
  ESP32 (#5546) and the P4 (#5556) in May 2026, still marked work in
  progress: an embassy-net adapter, a `MdioBus` trait for external PHYs,
  and raw `transmit` / `receive` of whole frames. Community fallback:
  `esp-p4-eth`, tested on exactly this board and PHY, pre-1.0, DHCP not
  validated end to end. Unknown until the board runs: which of the two
  brings the IP101GRI up.
- **TLS.** supervictor's edge firmware runs mbedTLS (`mbedtls-rs`, the
  esp-rs fork) on the classic ESP32 in 112 KB of heap, so TLS on a node is
  proven on this chip family; the WROVER spike left it out for memory, not
  capability. The fork ships prebuilt libraries for riscv32imc and
  riscv32imac, not the P4's riscv32imafc, so it compiles mbedTLS with
  clang and cmake on the fly: feasible, untested on a P4. `embedded-tls`
  (pure Rust TLS 1.3 client, no_std, no allocator) is the fallback and
  still calls itself work in progress, with certificate verification on
  std only. TLS belongs on the uplink to the relay (a per-node client
  certificate) and the deploy channel (plus a signed image); the field bus
  stays plain Modbus and CIP, as on every PLC.
- **Protocols on one port.** Modbus TCP, EtherNet/IP (TCP 44818 explicit,
  UDP 2222 Class 1) and PROFINET RT class 1 are built to share a switched
  plant network and can share the node's one PoE port. PROFINET's
  EtherType 0x8892 frames never reach smoltcp (its raw socket is
  IP-level), so a frame splitter in front of embassy-net is needed, which
  esp-hal's raw `receive` allows. There is no usable open PROFINET IO
  controller (p-net is device-side; the PI community stack needs
  membership; `profinet-rs` is GPL-3.0-only and libpcap), so it would be
  our own implementation. **EtherCAT does not fit**: a master needs a
  dedicated port with the slave chain hanging directly off it (the
  segment reflects every frame back; a switch adds a store-and-forward
  hop per cycle), the P4 has one MAC, and on a PoE node that port is the
  uplink. EtherCAT stays a box feature on a second NIC (`ethercat-master`).
- **Verified on the host, 2026-11-08, before the board arrived.** The whole
  P4 firmware stack compiles on upstream *stable* Rust for
  `riscv32imafc-unknown-none-elf` with no build-std: esp-hal 1.2.2 (the
  first release with the P4's EMAC: `esp32p4`, `unstable`, `rt`), esp-rtos
  0.4, esp-bootloader-esp-idf 0.6, esp-alloc 0.11, esp-println 0.18,
  esp-backtrace 0.20, embassy-executor 0.10, embassy-net 0.9, together
  with helmsman-iec, helmsman-modbus and helmsman-proto without std. The
  esp-rs mbedTLS fork does **not** build for the P4 at the rev supervictor
  pins (`4329f17`: a bindgen field mismatch), and does at its current main
  (`07aeabb`), with Homebrew LLVM's clang first on `PATH`: Apple's clang
  has no RISC-V backend, and the esp toolchain's "clang" directory holds
  libraries only. So TLS on the P4 node is a known-good recipe, not a
  question.
- **Code side, done.** `helmsman-proto` builds `no_std + alloc` without its
  default `std` feature (#296): encapsulation, CPF, CIP, the identity
  item, Forward_Open / Forward_Close and the Class 1 packet, the Logix
  read layouts and the EDS parser, plus `class1` (the transport-free
  producer and consumer of a Class 1 connection, which the box's
  `scanner` threads now run too). `scripts/lint.sh` builds that half for
  thumbv7em. **`crates/helmsman-node-p4`** is the P4 firmware, its own
  package outside the workspace like helmsman-node, on stable: `board`
  (the Waveshare pin map, the efuse MAC), `ethernet` (esp-hal's EMAC
  driver under embassy-net with DHCP, the PHY polled every 500 ms),
  `modbus` (the POE2's client, unchanged), `enip` (an explicit session
  over embassy-net TCP for `RegisterSession` and `SendRRData`, and
  `IoConnection`: `Forward_Open` through it, `class1::{Producer,
  Consumer}` around the UDP socket, the E1213's O->T port quirk honoured)
  and two binaries. `modbus` is the POE2's Phase 2 firmware on the P4.
  `enip` opens an Exclusive Owner connection to the E1213 (configuration
  100, O->T 102 one byte under a run/idle header, T->O 101 51 bytes
  modeless, RPI 100 ms), runs the chase program at a 10 ms scan with the
  DI bits and DO status as its inputs, produces at the RPI, counts late
  scans, packets each way, ignored datagrams, losses and reopens, and
  reports every 10 s over serial and UDP; `ENIP_INPUT_ONLY=1` at build
  time asks for Input Only instead, which runs beside the try box's
  exclusive owner. Both link (`cargo build --release`), espflash builds a
  526 KB app image from the `enip` ELF, and the main stack is about
  130 KB with a 256 KB heap. `NODE_BOARD=p4-eth scripts/node-build.sh`
  and `NODE_BOARD=p4-eth scripts/node-flash.sh [enip|modbus]` are the
  entry points. Nothing has run on hardware yet.
- **First day on the board, in order.** (1) `espflash board-info` over USB,
  Ethernet unplugged, and the MAC noted. (2) Flash `modbus`, unplug USB,
  plug PoE: a lease and the chase on the E1213's outputs says the EMAC,
  the PHY and the stack work. (3) Flash `enip` with `ENIP_INPUT_ONLY=1`
  beside the try box: T->O packets arriving says the Class 1 path works.
  (4) Stop the try box's enip-io (or point it at Modbus) and flash the
  Exclusive Owner build: the chase over CIP. (5) Measure the PoE module's
  isolation before USB and PoE are ever connected together.

Sources for this phase: esp-hal issue #4163 and PRs #5546 / #5556
(`github.com/esp-rs/esp-hal`), `github.com/babasha/esp-p4-eth`,
`github.com/esp-rs/mbedtls-rs` (mbedtls-rs-sys README, "Precompilation"),
`docs.rs/embedded-tls` 0.19, `docs.rs/smoltcp` (socket::raw), the Zephyr
ESP32-P4 features page (one 10/100 Ethernet MAC, RMII), the Waveshare
ESP32-P4-ETH wiki, `github.com/pasrom/profinet-rs`,
`github.com/rtlabs-com/p-net`, the ETG EtherCAT FAQ, and
supervictor `supervictor/edge/{Cargo.toml,src/config.rs}`; all read
2026-11-08.

**Phase 4, a node you can commission, deploy to and watch (2026-11-08, on
the POE-ISO).** Nick's brief: set up a node's network like supervictor
now does, connect to it, update the runtime, and watch it from the host
canvas, so a user has one robust experience. The decisions (interview):
a fixed factory address like Moxa's (192.168.127.250/24), DHCP as an
option; program deploy before firmware OTA; the node serves the box API
subset so the existing canvas box view works unchanged; a bearer token
from the settings record guards writes; programs compile on the node;
status is scan health, memory and fieldbus links. What exists:

- **`crates/helmsman-nodecore`** (workspace member, `no_std + alloc`,
  host-tested): the settings record and the program slots in the `nvs`
  partition (two slots each, sequence-numbered, checksummed, 24 KiB in
  all, the partition espflash writes by default), a bounded HTTP/1.1
  reader and writer with SSE framing, the box API's JSON shapes
  (`render.rs`'s field names) plus `/api/node` and `/api/settings`, the
  node's own page with the settings form, and the router whose effects
  (deploy, control, save settings) the firmware carries out. The
  canvas's `node_api_tests.rs` parses the node's JSON with the same code
  that parses a box, so the two cannot drift apart unnoticed.
- **`helmsman-node`'s `node` binary** (`api` feature; `scripts/node-build.sh
  api`, `scripts/node-flash.sh node`): settings and program from flash
  (factory record written on an empty partition), the I/O task on its
  own (the scan never waits on Modbus), two HTTP connections on the
  settings' port, the site beacon, a report line every 10 s. Verified on
  poe-iso-rack-03 from the Studio over the wire: first boot at the
  factory address with the page and every read route answering; a save
  on the page (the first save needs no token and sets one) wrote the
  record and restarted the node on static 192.168.3.55; record and
  program came back from flash; ping; the values stream; `PUT
  /api/modules/blink` with the token compiled, ran and was kept (201),
  stop and run (200), a broken program refused with the checker's line
  (422) and the old program kept running, a tokenless write refused
  (401); a settings re-save restarted the node and the deployed program
  came back from flash. Scan: 10 ms, zero late scans in steady state,
  about 1 ms of work, the E1213 up at 192.168.3.60.
- **Two WROOM lessons.** (1) The main stack is what the linker leaves above
  `.bss`, and the task pools sit at the very top of `.bss`: at 112 KiB of
  heap the stack was 42 KiB, compiling the program at boot overran it by
  a few hundred bytes, the pools read as "spawned" and the first spawn
  failed Busy, which looked like a dead link; esp-hal's stack guard is a
  word 60 B below the stack base and a frame larger than that steps over
  it. 96 KiB of heap leaves 59 KiB of stack. (2) Memory: 153 KiB of
  196 KiB used with one small program resident, most of it the runtime's
  parsed standard library; compiling a second program beside the first
  ran the heap out and reset the node, so a deploy now drops the running
  program, compiles, and restores the old one from its source if the new
  one fails, and only a program that runs is written to flash. A real
  cell program is a P4 job.
- **The host canvas opens a node by address.** `?box=http://192.168.3.55:8080`
  on any canvas page (the dev server's, or a box page served with
  `--site`) sends every box read and the values stream to that origin;
  only an origin is accepted, never a path. Verified 2026-11-09 in a
  headless Chromium against the node: the canvas fetched `/api/modules`,
  `/api/devices`, `/api/io_map`, opened `/api/events`, selected the chase
  module and fetched its source, and showed "chase running" with its scan
  count and the twelve signals live on the I/O tab; the trace stream it
  also asks for answers 404 on a node, so the diagram pane says it is
  waiting for a traced scan. (From an agent session the browser itself
  cannot reach the LAN, macOS Local Network privacy; a TCP forwarder run
  by Apple's python on localhost stands in.) Writes too, since card
  `canvas-node-token` (2026-11-09): on such a page the sign-in panel asks
  for the node's token instead of a user, keeps it in that browser for
  that origin only (`localStorage`, never the URL), and the transport
  sends it as `Authorization: Bearer` on every call there (the node
  answers the preflight), so Deploy, Save, Stop and Run work from the UI.
  Forget drops it. A box watched by address still refuses the preflight,
  so that path is a node's today.
- **`helmsman-node-p4` has the same binary** (`node`, from the same
  modules; esp-storage 0.10 and esp-bootloader-esp-idf 0.6 for the P4),
  built and lint clean, waiting for the board.
- **Bench topology.** The Studio is on 10.0.0.0/24 twice (a wired alias and
  Wi-Fi) and answers wired nodes through the access point; nodes live on
  the 192.168.3.x link with the E1213 and the Studio's en0, static.
- **Left in this phase:** firmware OTA (supervictor's installer and
  rollback policy); an I/O map setting for the block's address and point
  names; writes from the canvas (deploy, controls) need a token field in
  the canvas, today they are `curl` with the bearer.

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
  `crates/helmsman-plc/src/runtime.rs`, `crates/helmsman-project/starters/`
  (as of 2026-10-25; the batch mixer now lives in `crates/helmsman-node/bench/`).
- `core` float method availability: a `#![no_std]` crate calling the
  twelve `f64` methods named above, `cargo check` on rustc 1.99.1
  (2026-10-13), twelve `E0599` errors.
