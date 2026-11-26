# How big a machine can one Skipper run?

> **Saturday 2026-11-28, overnight: a local Fable session on the Studio,
> with the bench.** It has to measure on the Skipper itself, so it is not a
> cloud run. Nick starts it around 21:00 CT (a reminder fires then). In
> tmux on the Studio: `cd ~/github/helmsman && claude --model
> claude-fable-5-2`, then paste: *Work the TODO card `node-capacity`: read
> PERFORMANCE.md and follow its section "Brief for the Saturday run".* It
> appends "The plan" to this file, adds the build cards under
> `node-capacity` in TODO.md, and opens a DRAFT pull request into `dev` by
> Sunday morning.

A problem statement, written 2026-11-26 from a question of Nick's: "How many
devices could this unit realistically run? Are we talking about 16 servo
axes with 32 sensors and 25 valves? Where's the realistic ceiling? Is it the
program?" It is meant for a strategist (Fable) to read cold and propose a
solution; the TODO card is `node-capacity`. Everything below is cited to
the code at 429a0fcc (dev) or to docs/EDGE.md, and the last section names
what nobody has measured.

## The question to answer

1. **Where is the ceiling of one Skipper today**, in the terms a machine
   builder uses (axes, sensors, valves, program size), and which limit
   binds first for a realistic machine?
2. **What should the ceiling be**, and what is the cheapest sequence of
   changes that gets there without breaking the node's guarantees (below)?
3. **When the answer is "more than one node" or "a box"**, what is the
   rule a builder follows to split a machine, and what is missing for that
   split to work (cross-node signals, handoffs, the canvas)?

A good answer is a short ranked plan: each change, what it buys in axes or
points or program lines, what it costs (RAM, flash, compatibility, safety),
how it is proven on the bench, and the order. It may argue the target
machine below is the wrong target.

## The target machine (Nick's example)

16 servo axes, 32 discrete sensors, 25 valves, one program sequencing them.
Assume what the bench has: each axis is a networked integrated drive like
the Novanta MDrive (one EtherNet/IP device an axis), the sensors on remote
I/O blocks, the valves on one fieldbus valve terminal. The MDrive's
library profile maps `position`, `error-code` and the moving bit in, and
`max-velocity`, `enable`, `run-current` and `slew` out
(docs/devices/novanta-mdrive.md): about 7 points an axis. Its assemblies are
16 bytes in and 36 out (4 of them the run/idle header).

## What a Skipper is

The node board (Waveshare ESP32-P4-ETH on the bench; node-p4-poe in
the `skipper` repo, our own board) has **no field I/O**: no expansion
header, every spare GPIO unconnected. Every device is on Ethernet, so
capacity is a firmware and network question. The node powers nothing in
the field. It runs one ST program on a fixed scan (10 ms on the bench) and
moves a process image to and from its devices over Modbus TCP and
EtherNet/IP Class 1 (docs/EDGE.md, "Sixteen devices on one node").

## The limits as they stand

Constants, all chosen, none hardware:

| Limit | Value | Where |
|---|---|---|
| Devices | 16 | `crates/helmsman-nodecore/src/iomap.rs:140` `MAX_DEVICES` |
| Points, one device | 32 | `iomap.rs:142` `MAX_POINTS_PER_DEVICE` |
| Points, whole map | 96 | `iomap.rs:144` `MAX_POINTS` |
| Bool numbers, one device | 0 to 31 | `iomap.rs` `BOOL_WINDOW_BITS` |
| Numeric image, each direction | 128 bytes | `iomap.rs:154` `NUM_BYTES` |
| EtherNet/IP assembly | 128 bytes | `iomap.rs:158` `MAX_ASSEMBLY` |
| Shortest RPI | 10 ms | `iomap.rs:160` `MIN_RPI_MS` |
| Map text | 4096 bytes | `iomap.rs:138` `MAX_TEXT` |
| Program + map, together | 8140 bytes | `crates/helmsman-nodecore/src/program.rs:41` `MAX_SOURCE` (an 8 KiB slot) |
| Shared values | 32 | `iomap.rs:168` `MAX_SHARES` |
| Internal heap | 288 KiB, PSRAM as a second region | `crates/helmsman-node-p4/src/bin/node.rs:55` `HEAP_SIZE` |
| Settings + both program slots | 24 KiB `nvs` partition | `crates/helmsman-node-p4/partitions.csv:15` |

The target machine against them:

| Limit | Target machine | |
|---|---|---|
| 16 devices | 16 drives + at least one I/O block + a valve terminal = 18 or more | over |
| 96 points | 16 × ~7 + 32 + 25 ≈ 170; at a bare 3 an axis still 105 | over |
| 128 B numbers out | target and velocity as 32-bit values: 8 B × 16 = 128 B, full with nothing else | at the edge |
| 32 bools a device | 32 sensors on one block: exactly full; 25 valves fit | fits |
| 4 KiB map | ~190 lines at ~35 B ≈ 6.5 KB | over |
| 8140 B program + map | the map alone fills it | over |

Measured on the P4 (docs/EDGE.md, Phase 3 "On the board"): the 431-line
batch-mixer cell (12 KB of source) compiled on the node in under 80 ms,
ran in 0.95 to 1.05 ms of a 10 ms scan, held 253 KB of heap and peaked at
377 KB compiling (spilling into PSRAM). It **ran but could not be kept**:
it is bigger than its flash slot. Host figures for the compiled form
(docs/EDGE.md, "Numbers measured on the Studio"): about 200 KB fixed plus
about 0.8 KB a source statement, 1.5× that while compiling; `Value` is 32
bytes (an `i128` and a tag).

Not limits: the wire (16 Class 1 connections at 10 ms with full 128-byte
assemblies both ways is about 5 Mb/s of 100), and scan lateness (94 to
141 us at most, measured with two and three devices).

## The order the limits bite in (one session's reading, not decided)

1. **The map caps and the flash slot.** They refuse the target machine
   before anything runs. The slot already refuses a real 431-line program.
2. **RAM for the compiled program.** Once the caps are raised, a program
   for 16 axes is plausibly 2 to 4 times the cell, so it lives mostly in
   PSRAM.
3. **Scan time.** The cell used a tenth of the scan; what PSRAM does to
   that is unmeasured.
4. **The kind of motion, which no constant moves.** Cyclic EtherNet/IP at
   10 ms or more is indexing: the node sends "go to X at V" and each drive
   runs its own profile. Coordinated motion (gearing, camming, interpolated
   paths across axes) needs CIP Motion or EtherCAT-class time sync, which
   neither the node nor the box does. If the 16 axes must move together,
   that decides the architecture, not the counts.

The same session's suggestion, offered to be argued with: treat a Skipper
as one station of a machine (about 4 to 6 indexing axes, a valve terminal,
an I/O block, a few hundred lines of program); raise the slot first (the
partition table), then the point and numeric caps; leave 16 devices; split
large machines by station and hand off through `share`.

## Guarantees a solution must keep

- **Safety does not ride the network.** E-stop and safe torque off are
  hardwired; a device's own comms-loss state is the machine's protection
  when a node goes silent (docs/EDGE.md, "The short answer").
- **The scan stays bounded**: the instruction budget and a wall-time
  watchdog; a late scan skips, it never bursts.
- **Old maps and programs still load.** A one-device bool map keeps its
  addresses (`iomap/tests.rs` pins them); a firmware rolled back past a
  change must not misread what a newer one stored (the 2048 to 4096
  `MAX_TEXT` change already costs that, docs/EDGE.md).
- **One map text means the same on a box and on a node**, in the box's
  driver names.
- **A device that is down costs the others nothing** (one task and socket
  a device; one failed dial every 2 s in the whole node, `dial.rs`).
- The node firmware is `no_std` Rust on esp-hal; nothing in this repo may
  take a dependency without the review in ~/.claude/rules/dependency-review.md.

## Levers already known (costs as far as anyone knows them)

- **A larger program slot**: a new partition table moves the settings, so
  every node is commissioned again (TODO.md `node-p4-followups`, notes).
- **Raising the point and numeric caps**: the image is bytes, so the RAM is
  small; the signal list is copied on each API request (about 60 B a
  point); bool and numeric address rules must keep existing addresses.
- **More devices**: about 1.46 KB of stack resources a socket and a
  1 080-byte task each (docs/EDGE.md, "RAM"), plus the neighbour-lookup
  rule with many absent devices.
- **A smaller compiled form**: boxed tree nodes, `String` names and 32-byte
  values; "room to shrink this by several times, but that is a
  representation change in the compiler" (docs/EDGE.md). It would help the
  box too. Compiling on the programming side and shipping a compact image
  is the other road; neither exists.
- **More than one node**: `share` (Modbus TCP tables, 32 values a node) is
  host-tested only, not run on a board. A cross-node signal needs an age, a
  timeout and a safe value when stale.
- **A box instead of a node** for a big machine: the box's own limits on
  devices and points were not checked for this statement.

## What nobody has measured

- 16 devices on the board at once, half of them absent (`node-multi-device`
  bench runs 4 and 5).
- Heap and scan time with the 12 KB cell and 16 devices together, and what
  running a program from PSRAM costs a scan.
- `share` between two boards.
- A real MDrive and E1213 on EtherNet/IP from one node together.
- The per-axis point count above is the MDrive profile's; another drive
  will differ.

## Brief for the Saturday run

You are the strategist for this one question, in a local session on the
Studio with the bench, while Nick sleeps. Answer the three questions at the
top with a ranked plan and the measurements that settle it.

1. **Setup.** A worktree from `origin/dev` under `.claude/worktrees/`, on
   a branch `strategy/node-capacity-<date>`. Read the main checkout's
   `.claude/handoff.md` first: it holds the bench as it stands (the
   Skipper's address and token, the image and map on it, the serial port,
   what the box is doing) and whether another session holds the bench. If
   one is mid-run on it, do the host work only and say so.
2. **Read**, in this order: this file; `CLAUDE.md`; `docs/EDGE.md`; the
   TODO.md cards `node-capacity`, `node-multi-device` and
   `node-p4-followups`; then the code the limits live in
   (`crates/helmsman-nodecore` iomap.rs, program.rs, dial.rs;
   `crates/helmsman-node-p4` bin/node.rs, io.rs, partitions.csv; the
   compiled form in `crates/helmsman-iec`). Check every limit and figure
   above against the code before you build on it, and correct what is
   wrong, saying what changed. Argue with the earlier reading where the
   evidence is against it.
3. **Measure on the board** what "What nobody has measured" lists and the
   plan depends on: 16 devices with half absent (simulated Modbus servers on
   Studio address aliases, never a stranger's address), heap, peak, PSRAM
   and scan time with the 12 KB cell and 16 devices, and what running from
   PSRAM costs a scan. Flash with `scripts/node-flash.sh` as the cards'
   notes say; a cargo-built binary cannot reach the LAN from a session, so
   talk to the board and the devices with `curl` or `/usr/bin/python3`.
   Before you change what is on the Skipper, record its image, map and
   program from its API, and put them back before you stop; say what it
   holds at the end. Host measurements are welcome too (`cargo run
   --release -p helmsman-iec --example trace_bench`, a realistic 16-axis
   program compiled on the host). Label every number measured (with the
   command) or estimated (with the reasoning).
4. **No motion and no new outputs.** Read the MDrive; never enable it or
   start a move (that waits for Nick). Drive no physical output beyond what
   the program already on the Skipper drives.
5. **Write it down.** Append `## The plan (strategist, <date>)` below this
   brief: each change, what it buys in axes, points or program lines, its
   cost (RAM, flash, compatibility, safety), how the bench proves it, the
   order; the rule for splitting a big machine across Skippers or onto a
   box, and what that split still lacks. Under `node-capacity` in TODO.md,
   one card per build step in the board's format (`result:`, `needs:`,
   `depends:`, `est:`, `difficulty:`, `id:`, `asked:`, `by: strategist
   node-capacity`). What only Nick can decide (money, hardware, a changed
   public interface or a removal, breaking stored maps or programs) goes
   under "Open questions for Nick" as a question card with the options and
   your pick. Do not tick `node-capacity` and never write an `answer:`.
6. **Limits.** Change only `PERFORMANCE.md` and `TODO.md`; measurement
   scratch stays under `scratch/`, uncommitted. No code, test, CI or
   dependency change. Run `./scripts/lint.sh` and `cargo test --workspace
   --locked` before the push and read their exit status (a timing test that
   fails under load and passes alone is load: say so). One DRAFT pull
   request into `dev`, `docs: Skipper capacity plan (strategist)`. Never
   push to `dev`, `staging` or `master`.
7. **End.** One push notification when done or blocked. In the PR and the
   session: the ranked steps a line each, one Skipper's ceiling today and
   after the plan (axes, points, program lines), measured versus
   estimated, the questions for Nick, and what the Skipper holds now.
