# Architecture

Helmsman Factory Automation is a small soft PLC for a factory edge box (a Pi 5
is the reference), headless, with a browser UI. The agent writes Structured
Text modules, an engineer approves them, the box runs them, and the operator
watches them run as a live diagram. This page is the map every roadmap card
points at: what the parts are, which crate owns each, and where the trust
boundaries sit. Product intent lives in `TODO.md` ("Product vision",
"Roadmap M1..M4"); this page is how the code is shaped to deliver it.

Every part below is tagged **built** (on `dev`, tested) or **planned** (a
roadmap card, by id).

## The picture

```
                       ┌──────────────────────────── relay (planned: relay-v0) ─┐
                       │ per-box token · Anthropic key · metering · prompts     │
                       └──────────────▲─────────────────────────────────────────┘
                                      │ HTTPS, box-initiated
 ┌──────────────────────────── the box (Pi 5) ───────┼────────────────────────────┐
 │                                                   │                            │
 │  ┌─ agent loop (built) ───┐   tools   ┌─ box API (built) ───────────────────┐  │
 │  │ draft → check → sim →  ├──────────►│ signals · modules · proposals ·     │◄─┼── canvas in any
 │  │ propose                │           │ snapshots · outputs · audit         │  │   browser on the
 │  └────────────────────────┘           └──┬────────────┬───────────────┬─────┘  │   LAN (built:
 │                                 policy + │ PIN        │ read-only     │        │   canvas-web)
 │                                          ▼            ▼               ▼        │
 │  ┌─ runtime: helmsman-plc (built: N modules, one scheduler thread) ─────────┐  │
 │  │  module A (ST, 10 ms)   module B (ST, 100 ms)   module C (ST, 1 s)       │  │
 │  │     own %I/%Q/%M window, own scan rate, faults stop only itself          │  │
 │  │  ── interpreter + checker: helmsman-iec (built) ─────────────────────────│  │
 │  └───────┬───────────────────────────────┬──────────────────────────┬───────┘  │
 │          │ process image (built)         │ signals by name          │ trace    │
 │          ▼                               ▼ (built)                  ▼ (built)  │
 │  ┌─ CIP adapter ───────────┐   ┌─ I/O map + drivers: helmsman-io ────┐         │
 │  │ helmsman-adapter        │   │ IoMap · Driver · modbus-tcp (built) │         │
 │  │ assembly 150 in, 100 out│   │ WASM drivers (planned: wasm-driver) │         │
 │  └───────▲─────────────────┘   └───────┬──────────────┬──────────────┘         │
 └──────────┼─────────────────────────────┼──────────────┼────────────────────────┘
            │ EtherNet/IP, box is target  │ box is master│ box GPIO (do:N/di:N)
     a scanner (CODESYS,               remote I/O,      stack light, relays
     a Logix PLC, our CLI)             drives, sensors

  observed controllers (a CompactLogix): read-only, via helmsman-proto (originator)
  (built: logix-tag driver; no Write Tag, no unrestricted session in the box build)
```

Three directions of traffic, three different rules:

1. **Toward devices the box masters** (remote I/O blocks, drives, its own
   GPIO): the box reads and writes, but only through the I/O map. Logic names
   signals; only the map knows addresses.
2. **Toward controllers the box only observes** (a customer's CompactLogix):
   read-only. The box reads tags, it never writes them.
3. **From a scanner that owns the box** (the box as a CIP adapter on someone
   else's network): the scanner writes the box's input assembly and reads its
   output assembly, as with any adapter.

## Crates

| crate | owns | status |
| --- | --- | --- |
| `helmsman-proto` | CIP originator: encapsulation, CPF, discovery (`ListIdentity`), explicit messaging (`RegisterSession`, `SendRRData`, `Get_Attribute_Single`), Logix tag reads (`logix`: Read Tag, Multiple Service Packet, Read Tag Fragmented, Symbol object browse, backplane routing), Class 1 scanner. Byte codecs plus socket-level session, discovery and scanner. Any-service requests and the scanner sit behind the `unrestricted` feature. | built |
| `helmsman-adapter` | CIP target: a sans-IO `Device` state machine (every call takes `now`) and a std-thread server. Identity, assemblies, `Forward_Open`, cyclic I/O; a Logix tag-service fixture (`logix::Controller`) for tests. | built |
| `helmsman-cli` | `helmsman discover / get / io`: the originator from a shell. | built |
| `helmsman-iec` | IEC 61131-3: ST lexer, parser, checker, interpreter; `PLCopen` TC6 XML (LD/FBD/ST) lowered to ST; the standard FB library; the ruSTy differential oracle (dev only). One `Program` scans one `Image` (`%I`, `%Q`, `%M` byte areas) with an instruction budget. | built |
| `helmsman-plc` | The runtime: a library (`runtime::Runtime`, the multi-module scheduler; `spec`, the `--module` syntax; `store`, snapshots in SQLite) and the binary that puts it behind the adapter. Grows into the box core (scheduler, signals, snapshots, policy, box API) unless a card splits it. | built |
| `helmsman-io` | The I/O layer: `IoMap` (devices, signals, bindings) and its validation, the `Driver` contract, the Modbus TCP driver, the box's channels (in memory until GPIO), a device simulator, the conformance harness, the map file format (`text`). | built |
| `helmsman-graph` | Node-graph model and scan simulator behind the canvas's lab editor. | built (lab tool) |
| `helmsman-diagram` | A module's ST lowered to a read-only FBD-like diagram (`st-graph-view`): nodes, wires, gated regions, each tagged with its `StmtId`; deterministic layout; a trace's values and gates on it; "on because …" as data. Depends only on `helmsman-iec`. | built |
| `helmsman-agent` | The chat panel's agent loop (`agent-loop`): the relay client (protocol v1, HTTP/1.1 on std plus rustls), a bounded SSE reader, assistant-turn assembly from Claude's stream events, the fixed tool set run against the box API, budgets. No dependency on the runtime: its tools are HTTP calls to the box API. | built |
| `helmsman-canvas` | egui canvas, native and wasm: the live ST view (in a browser served by the box, that box's modules over its API; else a demo module running in-process, traced every frame) and the lab editor over `helmsman-graph`. | built |

Planned owners, not yet crates: the relay (separate stack, private per
`relay-repo`, in `../helmsman-relay`), the WASM host (`wasm-tier`).

## One scan, as built

With a single program (the default deploy) each scan, every `--scan-ms`
(default 10):

1. copy adapter assembly 150 (32 B, written by the scanner) into `%I`;
2. the runtime runs the program once (`Program::scan_at`), under an instruction budget
   (`DEFAULT_BUDGET`, 1 000 000; an endless loop is a `Watchdog` fault, not a
   hang);
3. copy `%Q` into assembly 100 (32 B, read by the scanner);
4. sleep to the next deadline; a late scan skips the periods it missed
   (counted as overruns) rather than bursting.

A fault (division by zero, index out of range, watchdog) zeroes that
module's outputs and stops it; the adapter stays on the network so the PLC
can still be found and diagnosed, even once every module has stopped.

## Many modules in one runtime (`multi-module`)

Built in `helmsman-plc` (`runtime.rs`; contract in `tests/runtime.rs`).
On the command line a module is `--module PATH[,name=N][,scan=MS][,i=START+LEN][,q=START+LEN][,m=BYTES]`;
a bare program argument is a module with every default (whole image), so
existing deploys run unchanged. A module can also be a directory
(`--module-dir DIR`, below), which is what the agent writes.

A module is one compiled ST `PROGRAM` plus a schedule and three windows:

- **`%I` window** into the box's input image. Windows may overlap: reading is
  free.
- **`%Q` window** into the box's output image. Windows may **not** overlap:
  two modules driving the same byte is rejected when the runtime is built, the
  same rule `IoMap::validate` applies to signals (`TwoWriters`). Output bytes
  no module owns stay zero.
- **`%M`**, private to the module.

The module is compiled against its windows' sizes and addresses relative to
them, so `%QX0.0` is bit 0 of *its* output window and the checker rejects an
access outside the window at compile time. A module cannot name another
module's memory.

One scheduler thread runs every module whose deadline is due, in a fixed
order, each against a snapshot of its `%I` window taken at the start of its
scan. Its `%Q` window is published to the output image only when its scan
completes. A module that faults has its `%Q` window zeroed and is stopped;
the others keep their schedule. A module whose scan runs past its next
deadline skips the missed periods and counts an overrun, so a slow module
cannot starve the others into a burst. The instruction budget bounds each
scan; the scheduler bounds how often.

## The I/O layer, as built (`helmsman-io`)

- **`Device`**: an endpoint the box is master of: `id`, `driver` name
  (`modbus-tcp`), `endpoint` (`host:port`), driver params (`unit`,
  `timeout_ms`).
- **`Signal`**: what logic names: an identifier, a type (`Bool`, `Int`,
  `Real`), a direction (`In`, `Out`), an optional engineering `unit`
  (metadata, never converted), an optional per-input `poll` period, and a
  **`Binding`**: a point on a device in that driver's own address syntax
  (`coil:0`, `hr:100:f32`), one of the box's own channels (`do:N`,
  `di:N`), or **another module's output** (`Binding::Module`, below). A
  CIP controller's tags and attributes (`logix-tag-read`) arrive as one
  more driver behind `Binding::Device`, with its own point syntax, so
  polling, links and validation apply unchanged.
- **`IoMap::validate`** checks everything that needs no device: unique
  names, identifiers, known devices, parseable points, signal type matches
  point type, outputs only on writable points, one writer per point, units
  of 1..=16 printable ASCII characters, poll periods of 1 ms ..= 60 s and
  only on polled inputs, module outputs as `in` signals naming a valid
  module. It returns every problem, not the first.
- **`Driver`**: `parse(addr)`, `read(&[addr]) -> Vec<Value>` (all or
  nothing, batching is the driver's business), `write(addr, value)`. Strings
  and plain values only, so the same contract crosses a WASM boundary
  unchanged.
- **`modbus`**: the Modbus TCP client and driver, from the spec (citations
  in `docs/SOURCES.md`).
- **`sim`**: a Modbus device with cross-wires (a coil driving a discrete
  input, like a wire on the bench) and injectable faults.
- **`harness`**: driver conformance, reported as `PASS`/`FAIL`/`SKIP` lines.
  Writes are proven through loops (output wired to input), so a driver that
  only pretends to write fails. `cargo run -p helmsman-io --example
  conformance -- --sim`.

- **`logix`**: `logix-tag`, read-only signals from an observed Logix
  controller by tag name (`in temp real plc Program:Main.Temp@real`); the
  `@TYPE` is the tag's Logix type and is checked on every reply. Reads are
  batched into one Multiple Service Packet per cycle.

- **`local`**: the box's own channels (`do:N`, `di:N`) behind the same
  contract; in memory until `gpio-outputs` adds the Pi backend.
- **`text`**: the map as a file, one `device` / `in` / `out` line each, with
  optional `unit=`, `poll_ms=` (inputs) and `safe=` (outputs) in any order;
  `in NAME TYPE module MODULE` is another module's output, `out NAME TYPE
  screen TILE` a floor screen tile (below). Older maps read unchanged;
  `box`, `module` and `screen` are reserved device ids.

## Signals in the scan (`io-runtime`, built)

`helmsman-plc --io-map FILE` loads the map; a module names signals by
declaring them as its PROGRAM's `VAR_INPUT`s and `VAR_OUTPUT`s (matched by
name, case-insensitively, as ST is). That interface is what a module
manifest declares, and is checked against.

- **`SignalTable`** (`signals.rs`) holds each input's last good read and each
  output's *command*. Outputs start at their safe value.
- **Binding** happens when the runtime is built: an unknown name, a
  `VAR_OUTPUT` on an input signal, a type the port can't carry (integers
  cross as `i64`, so `ULINT`/`LWORD` outputs are refused), and two modules
  driving one signal are all refused before the adapter starts.
- **Links** (`link.rs`), one thread per device plus `box`: each cycle writes
  the commands that changed since the device last took them, then reads
  every input that is due. A link cycles at the fastest of its device's
  `poll_ms` and its inputs' own; an input without one is read at the
  device's rate. A *link* error (I/O, an invalid reply) marks the link down:
  its inputs go missing, and it forgets what the device holds, so every
  output is written again on reconnect. A *point* error (a value the point
  can't hold, a device exception, an unknown tag) stays on that signal: it
  is recorded (`SignalTable::point_error`), a refused output is retried each
  cycle, a refused input goes missing alone, and the link stays up.
- **A module never computes on missing data.** If any signal it reads has no
  value (device down, or a value that doesn't fit the port), it is held: not
  scanned, `%Q` window zeroed, output signals sent to their safe values. It
  runs again once every input is back. An output value its signal can't
  carry (a non-finite `REAL`) faults the module (`OutputRange`).
- **Module outputs as signals** (`signal-model`). `in rate real module
  conveyor` is the `VAR_OUTPUT rate` of module `conveyor`, which other
  modules read as `VAR_INPUT rate`. It has a value only once `conveyor`
  has scanned, and none while it is held or faulted, so its readers are
  held too and their outputs go safe. Nothing polls it, and only
  `conveyor` may drive it. Refused at load: a module reading its own
  output, a reader whose source isn't loaded or doesn't drive the signal,
  and modules reading each other in a loop (which would hold them all
  forever). A reader sees the source's latest completed scan, from the
  same tick if the source comes first in scan order and from the previous
  tick otherwise. An `out` signal is still readable through its command,
  as before.

## Module directories (`module-format`, built)

`helmsman-plc --module-dir DIR` (repeatable, alongside `--module`) loads a
module from a directory: `module.st` and `module.manifest`
(`manifest.rs`; contract in `tests/manifest.rs`). The manifest is a small
line format, hand-parsed like the I/O map:

```text
name     conveyor
author   Nick <nick@example.com>
prompt   Run the belt while start is held; stop on the jam sensor.
scan_ms  50
q        0+2
reads    start jam
drives   motor lamp
param    speed   real 1.5 min=0 max=10
param    beacon  bool true
```

- **Checked against the program, not trusted.** Every `VAR_INPUT` is
  under `reads` or `param`, every `VAR_OUTPUT` under `drives`, and every
  declared name is a port facing that way (case-insensitive). A manifest
  that disagrees is refused before the adapter starts, so policy and the
  box API can read the manifest instead of the ST.
- **Params are `VAR_INPUT`s bound to a value instead of a signal.** Typed
  (`bool`, `int`, `real`), ranged (`min`/`max`, required for numbers, and
  the whole range must fit the port's ST type), set before every scan. An
  engineer or the agent changes a param by changing the manifest; the
  logic is untouched, and the range is the policy line.
- **Windows** (`i`, `q`, `START+LEN`) are module-relative and only for
  modules that use `%I`/`%Q`; left out, a module has none. `%M` is private.
- `author` and `prompt` (one line each, repeatable) are free text: any
  Unicode except control and invisible formatting characters (no escape
  sequences or bidi overrides in a log or a review). Files over 1 MiB are
  refused.

## The trace tap (`trace-tap`, built)

What the live canvas draws: one scan of one module, as it ran.

- **`helmsman_iec::trace`**. `Program::scan_traced` runs a scan and returns
  a `Trace`: per statement (a `StmtId`, with POU, line, column, shape and
  parent arm in the shared `TraceMap`) how many times it started; for each
  IF/CASE the arm taken (last) and per-arm counts; for each loop its
  passes; the fault and the innermost statement running when it struck;
  every scalar of the program at the end of the scan, FB internals
  included (`s.hold.ET`), plus the module image. Statements of FB and
  function bodies count over every instance and call; each instance's state
  is in the values.
- **Cost.** A plain `scan_at` pays one predictable branch per statement
  (about 2% on `examples/trace_bench.rs`); a traced scan about 1% more.
- **`helmsman_plc::TraceTap`** (`Runtime::tap(name)`): sampling on demand.
  A viewer calls `request()`, that module's next scan is traced, the
  `Sample { scan, fault, trace }` lands in a one-slot mailbox that `take()`
  empties. The scheduler thread never waits on it (one atomic load per
  untraced scan; a `try_lock` to post). A UI at 30 Hz traces at most 30
  scans a second however fast the module runs. The tap survives a swap.

## Shift calendar (`shift-calendar`, built)

`helmsman-plc --calendar FILE` loads shifts, breaks and planned downtime
(`calendar/`; file format in `calendar/text.rs`, contract in
`tests/calendar.rs`). Weekly patterns plus dated exceptions (`holiday`,
a dated `shift` that replaces that day's occurrence), in local time at a
fixed UTC offset, with daylight saving written as dated offset changes: no
time zone database (`calendar-timezones` asks about one). Shifts may cross
midnight; overlapping shifts and breaks outside their shift are refused at
load.

- **Evaluation** (`Calendar::at`) is a pure, total function of wall-clock
  seconds: the runtime still has no clock, the binary reads `SystemTime`
  each loop and wakes at least once a second when a calendar is loaded.
- **As signals**: `calendar::Source` adds `in_shift`, `on_break`,
  `planned_down`, `working` (`BOOL`) and `shift_id`, `shift_minute`,
  `shift_left` (`INT`) to the `SignalTable` as box-provided inputs. A
  module asks "in shift?" by declaring `VAR_INPUT in_shift : BOOL;`, the
  same binding as a device signal; a map signal with one of those names is
  refused.
- **Scheduled outputs**: `drive SIGNAL [!]COND` makes the calendar the
  writer of a `bool` output signal. The table records it, and a module
  binding a `VAR_OUTPUT` to that signal is refused at load
  (`TwoWriters`, writer `calendar`). Links write it like any command.
- **Untrusted clock**: before 2024-01-01 (a Pi with no RTC before NTP) the
  calendar's signals are missing, so modules reading them are held, and
  driven outputs go to their safe values.

## Floor screen (`andon-screen`, built)

A box-driven full-screen andon view, an output kind next to GPIO lights
(`screen.rs` in `helmsman-plc`, `andon.rs` in the canvas; contracts in
`tests/screen.rs`, `andon_tests.rs`).

- **Tiles are outputs.** `out line1_state int screen line1` in the I/O map
  (`Binding::Screen`): an `out` signal no device takes, so no link polls or
  writes it. A module drives it through a `VAR_OUTPUT` of that name, so the
  one-writer rule, safe values (a held or faulted module's tile goes to its
  `safe=` state), the approval tiers (a map change is output-affecting) and
  the audit apply as to a stack light. Several signals may feed one tile.
- **Messages are state codes, not strings.** A module drives an `INT` (or a
  `BOOL`, read as 0/1); the layout maps each code to a fixed colour and a
  fixed message. Signals stay scalar (no string type in the scan, recorder,
  replay or sinks), and the words in front of the line are reviewed box
  configuration, never text a module computes. A code the layout does not
  name shows as "UNKNOWN STATE n".
- **Layout file** (`--screen FILE`, `helmsman-screen 1`, hand-parsed, 64 KiB
  cap): `title`, `grid COLS ROWS` (1..=8), `code SET N COLOUR TEXT`, `tile
  ID COL,ROW[,W,H] SET SIGNAL TITLE`, `value ID SIGNAL LABEL` (a number
  under the message). Colours are a fixed palette (IEC 60073 stack-light
  colours plus the brand's safety yellow and graphite; no user theming).
  Checked at startup against the map: each tile's signals are `screen`
  outputs on that tile, of a type it can show, and every `screen` output is
  on a tile; a map with tiles and no layout is refused while the API runs.
  Changing it is a box config change (a restart), like the sinks file; it
  is not in snapshots (`screen-layout-snapshot` asks).
- **Served read-only** at `GET /api/screen`: the file byte for byte (the
  canvas parses it with the same parser), the parsed layout and the palette.
- **The view** is the served canvas at `/?andon`: a header (title, and the
  shift from the calendar's `in_shift`/`shift_id`/`on_break`/`planned_down`
  when the box has them), big tiles filling the window, type sized to fit
  each tile. Fed by `/api/events`. **Stale data never shows as a colour**:
  when the stream fails the values are dropped, and when no event arrives
  for 3 s every tile shows "NO DATA" on black under a red banner ("BOX
  UNREACHABLE", "NO DATA FROM THE BOX FOR n s"); a stream quiet for 10 s is
  reopened. Full screen is the browser's (F11, or `chromium --kiosk
  http://BOX:8080/?andon`).

## Sim replay (`sim-replay`, built)

The evidence an engineer approves an output change on: what a proposed
module would have done over the last few minutes, beside what the running
one did. `record.rs` and `replay.rs` in `helmsman-plc`; contract in
`tests/replay.rs`.

- **Recorder** (`record::Recorder`). Just before each tick the scheduler
  calls `observe(now, &table)`: every signal a module reads that the
  runtime does not compute (device and box inputs, the calendar's
  signals, outputs the calendar drives) is compared with the last tick
  and each change, value or missing, stamped with the tick's `now`. So
  the recording is exactly what the modules scanned against, on the
  runtime's clock; the links are untouched. Module outputs are not
  recorded: replay recomputes them.
- **Bounded**: `RecordConfig { window, max_bytes }` (default 10 min,
  4 MiB, about 100 000 changes), allocated up front, never grown.
  Evicted changes fold into a per-signal base value, so a recording is
  exact from its `start` however much was evicted.
- **Never blocks the scan**: the ring is behind a mutex the scheduler
  only `try_lock`s; changes it cannot hand over wait in a preallocated
  backlog (part of the same byte budget). A reader that holds the ring
  past the backlog restarts the recording from the scheduler's view:
  history is lost, never falsified. Measured
  (`examples/record_bench.rs`, release, M-series Mac): `observe` costs
  about 65 ns with 16 signals, 190 ns with 64, 700 ns with 256 when
  nothing changed (2.4 µs when all 256 did), against a 10 ms scan; a
  snapshot of a full 4 MiB ring holds it about 0.35 ms.
- **Recording** (`record::Recording`): channels, base values at
  `start`, changes to `end`. Exported as versioned text
  (`helmsman-recording 1`, one `signal` / `base` / `at` line each,
  reals in shortest round-trip form so it reads back bit for bit),
  hand-parsed, 64 MiB cap, every problem reported with its line. This
  is what a proposal carries and what tests use as fixtures.
- **Replay** (`replay::replay(table, i_len, q_len, modules, &rec)`):
  a fresh `Runtime` (programs cold-restarted) over a copy of the table,
  fed the recorded values and ticked at its own deadlines from `start`
  to `end`. It *is* the live runtime, so hold-on-missing, faults and
  scan periods behave as live. Refused: a recorded signal whose type or
  role the map changed (`Changed`), a module reading a signal the
  recording lacks (`NotRecorded`), modules that don't fit (`Config`),
  over 5 000 000 scans (`TooLong`).
- **Output**: a `Timeline`: per driven signal a `Track` (the value after
  each tick at which it changed: a step function, plus `rises()` and
  `on_time()`), per module its `Running` / `Held` / `Faulted` spans, and
  each fault with its time. `propose(.., running, proposal, &rec)`
  replays the running set and the set with the proposal swapped in (or
  added) and `compare`s them per signal: `extra_rises()` is "would have
  fired 3 more times".
- **Faithfulness**: replaying the running modules over a recording taken
  from the runtime's start reproduces the live outputs exactly when live
  scans were on time (property-tested against the Modbus simulator over
  TCP); a late live scan saw a slightly later `now`. A rolling window
  starts timers and counters cold at its `start`. The CIP scanner's `%I`
  image is not recorded and replays as zero. A proposal faster than every
  running module sees data at the running tick rate
  (`replay-granularity` asks).

## Snapshots (`snapshots`, built)

`helmsman_plc::store` (`store.rs`; contract in `tests/store.rs`) keeps
every applied change set in one SQLite file, `DIR/snapshots.db`;
`helmsman-plc --store DIR` runs whatever head is (and takes no other
module, map or calendar source).

- **A change set is files, byte for byte**: each module's
  `module.manifest` and `module.st` (params live in the manifest), the I/O
  map (the bindings) and the calendar. Commit checks that each manifest
  parses and names its module and that the map parses; it does not compile
  (whoever applies compiles first).
- **Content-addressed**: blobs are keyed by SHA-256; a snapshot's id is the
  SHA-256 of a canonical text tree (`helmsman-snapshot 1`, then one
  `module NAME MANIFEST SOURCE` line per module by name, `io-map H`,
  `calendar H`). Same content, same id, in any store. Every read re-hashes
  the tree and each blob, so a corrupted or edited row is an error, never
  wrong content. A tree plus its blobs is what a bundle carries (below).
- **History is an append-only log**: one entry per apply or rollback, with
  author, prompt, approver and time (caller's clock), each hashed over its
  fields and the previous entry's hash, so edited history fails `list`.
  Rollback is a new entry pointing at old content. Committing what head
  already is writes nothing. `diff(a, b)` names modules added, removed or
  changed (source, manifest, which params), devices and signal bindings
  changed, and whether the calendar changed.
- **Guarded**: triggers refuse `UPDATE` and `DELETE` on every table; the
  file carries an application id and `user_version` (schema 1), and a
  newer schema is refused rather than misread. Writers take SQLite's write
  lock up front (`BEGIN IMMEDIATE`, WAL), so concurrent commits never lose
  an entry. Author and approver are one line, prompt may span lines; none
  may carry controls or invisible formatting characters (as the manifest).

## Hot swap (`hot-swap`, built)

`Runtime::swap(module, &mut table)` replaces the module of the same name
between its scans (contract in `tests/swap.rs`). The adapter, the links and
the other modules never notice.

- **Checked like a fresh load**, by the same code, against the other modules
  as they run: windows, sizes, period, ports against the `SignalTable`, one
  writer per `%Q` byte and per output signal. A refused swap changes nothing.
- **State carries by name and type** (`Program::state` / `Program::restore`
  in `helmsman-iec`): top-level `VAR`, `VAR_INPUT`, `VAR_OUTPUT`, not
  `VAR_TEMP`, constants or located variables. A variable carries whole when
  its type has the same shape in both programs (same elementary type; same
  enumeration; same array bounds; same struct or function block, by name and
  members), so a `TON` keeps its elapsed time and a `CTU` its count; a new or
  retyped variable starts at its initial value. Every copied slot is also
  checked against the new slot's type, so nothing ill-typed lands in memory.
  `%M` keeps its bytes over the length both share.
- **No glitch**: the `%Q` bytes and output signals the module keeps hold their
  last values until the new program's first scan; bytes it gives up go to
  zero and signals it stops driving go safe at once.
- A **faulted or held** module restarts Running, its state carried; a faulted
  one is next due on its old phase after the last tick (open question
  `hot-swap-faulted-state`).

In the binary, an approved change reaches the runtime as
`Runtime::reconfigure` (the whole module set in one step: modules kept are
carried as `swap` carries one, new ones start cold, ones dropped have their
bytes zeroed and signals made safe), run by the scan thread between ticks;
see the approval policy below.

## Box API (`box-api`, built)

`helmsman-plc --api ADDR` (default `127.0.0.1:8080`, `0.0.0.0:8080` for the
LAN, `off`): one HTTP/1.1 + JSON surface for the canvas, the agent's tools
and later MCP (`api/`; contract in `tests/api.rs`). Endpoints are listed at
`GET /api` and in `api/mod.rs`.

- **Reads** (`GET`) are open: signals with live values, quality and point errors, modules
  (source, manifest, ports, status, stats), a module's latest trace (a
  `GET` requests the next traced scan; 202 until one lands), output commands
  and safe values, device links, proposals, the audit log, and snapshots
  (`--store`: history, a snapshot's files, a diff). Two event streams (SSE:
  one-way is all the canvas needs, and it is plain HTTP, so the same bounded
  parser serves it): live values and status, and a module's traces.
- **Everything else** needs `Authorization: Bearer <token>` (64 hex
  digits, made on first start in `api-token`, mode 0600, under
  `--state-dir` or systemd's `StateDirectory=`, printed once to the
  journal). A token in a query string is refused on any request. Every
  authenticated call is appended to the audit log whatever its outcome,
  with the snapshot id it concerns.
- **No write path.** There is no route that writes a signal, an output, a
  param or a module. `POST /api/proposals` stores a checked draft (a module
  directory's two files, or one param's value): parsed, compiled against its
  windows, held to its manifest, its signals checked against the box, and,
  when the box runs a change set, carrying the change set applying it would
  commit, its snapshot id, its diff and its tier. The only way a draft runs
  is `apply`, the only way back `rollback`, both through the approval
  policy (below).
- **The scan never waits on it.** The scheduler publishes a view (signal
  values, module status and stats, `%Q`) at most every 100 ms with a
  `try_lock`, dropping it if a reader holds the slot; the API reads only
  that, the catalog fixed at startup, the trace taps, and the snapshot
  store (its own SQLite connection, on API threads only). While the API
  runs the scan loop wakes at least every 100 ms to publish.
- **Bounded.** Hand-rolled on `std::net` (no new crates; `serde_json`, already
  in the workspace, for JSON): one request per connection, 32 at once (503
  beyond), 8 event streams, 16 KiB head, 64 headers, 1 MiB body,
  `Content-Length` only, 5 s for the whole request however slowly it
  arrives, 2 s for any write before a client is dropped, 10 min for an
  event stream (the browser reconnects). Audit log and
  proposals are bounded and refuse (507) rather than forget. A browser
  watching the box holds two event streams, so the 8-stream cap is four
  screens at once.
- **The canvas** (`--ui DIR`, `api/ui.rs`; contract in `tests/ui.rs`):
  `GET /` and the files beside it are the wasm canvas `make canvas-web`
  builds (`/?andon` is the floor screen, above), on the API's origin (no CORS). Only `GET`; a path is refused
  (404) before the file system is asked unless every segment is
  `[A-Za-z0-9._-]`, not empty, not dot-led (after percent-decoding, so
  `%2e%2e`, `%2f`, `%5c`, `%00` and any non-ASCII never pass), at most 8
  segments of 128 bytes, with a served extension (html, js, wasm, css,
  json, svg, png, ico, woff2, txt); what passes must resolve to exactly
  itself (no symlink below the directory, no case-folded match) and be a
  regular file of at most 64 MiB. Files stream in 64 KiB chunks within the
  write timeout per chunk and 120 s per file. Trunk's content-hashed names
  are cached a year (`immutable`), everything else revalidates
  (`no-cache`); HTML carries a CSP of `'self'`, `'wasm-unsafe-eval'` and its
  own inline scripts by SHA-256.

## Approval policy (`approval-policy`, built)

The safety story as code: `helmsman_plc::policy` (tiers, policy file, PINs)
and `api/approve.rs` (the apply path); contracts in `tests/policy.rs` and
`tests/approval.rs`.

- **Tiers, from the diff alone** (`policy::classify(running, target)`):
  `param` when only param values changed, each within the range the
  *running* manifest declares (the manifest may be re-rendered, the program
  byte for byte the same); `logic` when programs or manifests changed but
  every module drives the same signals and `%Q` bytes, the map is the same,
  and the calendar's `drive` lines are the same; `output-affecting` for
  anything else that can move an output: the I/O map changed at all
  (bindings, devices, safe values), a module's `drives` or `%Q` window
  changed, a module that drives something was added or removed, the
  calendar's `drive` lines changed, or a manifest or calendar does not
  parse. When in doubt, up. What a proposal says about itself is never
  read. Property-tested against an independent "what can drive an output"
  oracle: nothing that alters it is ever below output-affecting.
- **Policy, with a floor in code.** Per tier, the least role whose PIN
  applies it and whether a sim replay must be attached. The floor:
  `param` operator, no replay (applies live; undo is one rollback);
  `logic` engineer + replay (the preview); `output-affecting` engineer +
  replay; at most 5 wrong PINs before a lockout of at least 30 s that
  doubles per further wrong PIN up to 1 h. `--policy FILE` (`helmsman-policy
  1`, one `TIER ROLE replay=yes|no` line per tier, `lockout N SECONDS`,
  `min-replay SECONDS`, hand-parsed) can only be stricter: a file that
  loosens anything, or asks less of a higher tier than a lower one, stops
  the box at startup naming the line. `GET /api/policy` serves it.
- **Roles and PINs.** operator < engineer < integrator, each with its own
  PIN, set with `helmsman-plc --set-pin ROLE` (PIN on stdin) into `pins`
  in the state directory (mode 0600, refused if readable by others):
  PBKDF2-HMAC-SHA256, 600 000 iterations, 16-byte random salt each, over
  the `sha2` the store already uses (HMAC and PBKDF2 written out and
  checked against RFC 4231 / RFC 7914 vectors; no new crate). One lockout
  for the whole box across roles and connections; checks are serialized,
  so parallel guessing is no faster. While locked no PIN is checked
  (429, `Retry-After`). A PIN is only ever in a request body: never logged,
  audited or echoed (malformed bodies get a fixed message, not serde's).
- **Evidence bound to content.** `POST /api/proposals/{id}/replay` (or
  `/api/snapshots/{id}/replay` for a rollback) replays what runs and the
  target over the recorder's window (`replay::replay` twice, then
  `compare`) and keeps the result with both snapshot ids. A target with
  another map or calendar runs against its own (`replay::propose_change`):
  a signal the recording lacks is missing there, its readers held, and
  the evidence's `reload` says which (`unrecorded`, `complete`, `note`).
  The ids hash the map and calendar, so evidence binds to them too. An apply that
  needs one names it (`"replay": N`) and is refused (428) unless its base
  is exactly what runs and its content exactly the target, over at least
  `min-replay`: an old replay, or one of other content, never counts.
- **Apply = check, swap, commit, as one.** `POST .../apply` and
  `POST /api/snapshots/{id}/rollback` take the token and `{role, pin,
  replay?}`; in order: target known; draft not stale (its base is what
  runs, else 409); tier and rule; role at least the rule's (403, no PIN
  spent); evidence (428); PIN (403/429); every module compiles (422); another map or calendar
  loads (422) and the modules bind to it (409). Then the scan thread runs
  `Runtime::reconfigure` between ticks (or, with another map or calendar,
  the reload below) (refused: nothing changed, 409; not
  taken within 5 s: the job is withdrawn and can never land later), the
  store commits the target with author, prompt, approver (`ROLE (PIN)`) and
  time, and the API's view of the box moves to it; if the commit fails the
  scan thread is swapped back (500). Applies are serialized, so of two
  racing conflicting proposals one wins and the other is stale. Needs
  `--store`. The response names the previous snapshot as the one-call
  undo.
- **Audited, every path.** Every apply, rollback and replay, refused or
  not, is an audit entry with time, status, role (once the body parsed),
  target and base snapshot ids; an apply that acts writes one entry before
  (so a full log stops it) and one after.

## Map and calendar reload (`map-reload`, built)

An apply or rollback whose I/O map or calendar differs from what runs
reloads them on the running box, through the same approval path
(always `output-affecting` for a map, `logic` for a calendar with the same
`drive` lines). `POST /api/proposals` takes `{kind: io_map|calendar, text}`;
any snapshot can be rolled back to. Contract in `tests/reload.rs` (real
threads, the Modbus simulator over TCP).

- **Sequence** (`api/swap.rs`): load the new `Layout` (table + calendar)
  and bind the modules to it, both before anything moves; `LinkSet::plan`
  builds the links the new map needs with their drivers opened (one that
  will not open: 409, nothing changed); stop and join the links of
  removed or changed devices; the scan thread, between two ticks with the
  table locked, carries every unchanged signal into the new table
  (`SignalTable::carry_from`), rebinds the modules (`Runtime::rebind`),
  evaluates the new calendar, swaps the table and restarts the recorder
  on it, as one step; then (refused: the stopped links resume as they
  were) every output point the new map dropped from its device is written
  to its safe value through the stopped link's own driver, and the new
  links start. Then the store commits; if that fails the old change set
  is reloaded the same way (500).
- **No glitch.** A signal whose name, type, direction and binding are
  unchanged keeps its value, command and point error; a link whose device
  line and points are unchanged never stops (links find their points by
  name, device and address whenever the table's epoch changes, and never
  touch a point the table no longer binds to them). A restarted link
  rewrites the carried command, the value the device already holds; a
  driver with the same device line is carried into it (connection, `box`
  channels). An unchanged signal's point is never sent to safe on the way
  (pace params `poll_ms`/`timeout_ms` do not make it another device).
  Property-checked over 50 reloads across four map/calendar variants.
- **Safe.** What no healthy module drives takes the new map's safe value
  in the reload step: undriven outputs, and outputs of held or faulted
  modules (so a new safe value reaches the device at once). Added outputs
  start safe. Removed outputs are written safe before their link is
  dropped; if the device does not answer that cannot happen (the box only
  reaches a device through its link), and the apply says so in
  `reload.notes`: the device's own comm watchdog is the backstop.
- **Around it.** Sinks rebind their emits by name; the API's catalog,
  device list and replay table follow. The recording restarts, so the
  next replay's window starts at the reload. A published live view from
  just before the catalog moves can be read against the other one for a
  moment (display only).

## Agent loop (`agent-loop`, built)

`helmsman-plc --relay URL --relay-token FILE [--relay-agent v1]` turns on
`POST /api/chat` (`helmsman-agent`, `api/chat.rs`; contract in
`helmsman-plc/tests/agent.rs`). The box holds a per-box relay token, never
an Anthropic key, and names no model, prompt or tool schema: the relay
serves those per agent version (relay protocol v1).

- **The loop.** One user message: send the conversation to
  `POST /v1/agents/{version}/messages`, assemble the streamed assistant
  turn (text, `thinking` with its signature, `tool_use` from its
  `input_json_delta` fragments), run its tool calls in order, send every
  result back in one user message, repeat until `end_turn`. History is
  append-only and assistant turns go back exactly as streamed. A turn that
  fails (`relay_error`, the model API's `error`, a stream that ends without
  `relay_usage`, malformed events, a timeout) is discarded whole and none
  of its tools run; `refusal` runs nothing; a `max_tokens` turn's tool
  calls are answered unrun. Retryable failures are retried (2 s doubling,
  jittered). Budgets per message: 24 model turns, 200k output tokens, 16
  tool calls per turn, 15 minutes; a spent budget leaves the tool results
  last, so the next message continues.
- **Tools** (the relay's `tools.json` names): `list_signals`,
  `read_signals` (`GET /api/signals`), `list_modules` (`GET /api/modules`),
  `read_module` (`GET /api/modules/{name}` and its `trace`),
  `draft_module` (kept in the conversation, the manifest object rendered to
  manifest text with every value checked so none can start a line),
  `check_module` (`POST /api/proposals/check`), `simulate_module`
  (`POST /api/proposals/replay`), `propose_change` (`POST /api/proposals`,
  only for a draft that checks clean and has been simulated; param changes
  as param proposals). A draft reaches the proposal list only when
  proposed, so the agent iterates on checker and simulator results before
  a human sees anything. Every input is parsed strictly and validated
  (unknown fields refused); a bad one is answered `INVALID_JSON` unrun.
- **Data, not instructions.** Every tool result is plant data (tag names,
  ST comments, manifest prompt lines, diagnostics quoting them) and goes
  back inside a `<box-data>` envelope that says so, as JSON with `<`, `>`
  and `&` escaped, so nothing inside can close it. A tool name the model
  makes up is answered "no such tool", unrun. No tool reaches `apply`,
  `rollback` or any write, and the box API enforces token, limits and
  audit on every call the tools make.
- **Chat route.** `POST /api/chat` (token, audited) takes
  `{message, session?}` and streams `session`, `text`, `tool_call`,
  `tool_result`, `proposal`, `retrying`, then `done` or `error` (relay
  unreachable: "the agent is offline, installed modules keep running").
  Conversations are in memory: 16 at most (idle ones dropped after an
  hour, oldest first when full), one message at a time each (409), two
  running at once (503 beyond), 900 KiB of history.
- **Draft routes.** `POST /api/proposals/check` (the checks
  `POST /api/proposals` runs, nothing stored) and
  `POST /api/proposals/replay` (`{draft, minutes}`: what runs and the draft's
  change set replayed over the last 1..=1440 minutes of the recorder's tap,
  on the approval path's plant, in the same JSON shape as the evidence
  replay): read-only, token-gated and audited like every other POST. A
  draft replay is never evidence; an apply cites a replay of the stored
  proposal (`POST /api/proposals/{id}/replay`), and only a role PIN applies.
  The agent holds the API token and nothing else: it has no tool that
  reaches `apply`, `rollback` or a PIN, and the token alone cannot apply.

## Bundles (`bundle-format`, built)

`helmsman_plc::bundle` (format, manifest, signatures, CLI) and
`api/bundle.rs` (export, import); contract in `tests/bundle.rs`. The trust
model is `market-floor`'s floor: signing, a capability manifest and
host-enforced checks; an AI review (`marketplace`) can only refuse on top.

- **One file, hand-framed**: `helmsman-bundle 1\n`, then records of a type
  byte, a big-endian `u32` length and the bytes: the snapshot tree (`T`),
  the bundle manifest (`M`), each blob the tree names (`B`, ascending by
  SHA-256), detached signatures (`S`, 32-byte key + 64-byte Ed25519
  signature, ascending by key). No archive crate: four record types do not
  need one, and every byte is accounted for. The tree is the store's, so a
  bundle's id **is** the snapshot id of the same content.
- **Refused, with the reason**: over 16 MiB; a record claiming more than
  its type may hold (tree 256 KiB, manifest 16 KiB, blob 1 MiB, signature
  96 B) or more than the file has left, checked before any allocation;
  records missing, repeated, out of order, of an unknown type, or bytes
  after the last; a blob twice, a blob the tree does not name or a name
  with no blob (a changed file); a module name that is not a module name
  (`../`, `/`, non-ASCII lookalikes, invisible characters); a tree or
  manifest not in canonical form.
- **Bundle manifest** (`helmsman-bundle-manifest 1`, canonical): name,
  version, author, description, and the **capability manifest**: modules,
  signals read, signals driven (modules and calendar `drive` lines), `%Q`
  bytes, devices and drivers the signal-map template names. It is computed
  from the content (each module compiled and held to its own manifest, as
  at load) and must match **exactly**: understated or overstated, refused.
- **Signatures**: Ed25519 over `helmsman-bundle-signature 1\n` + bundle id
  + SHA-256 of the manifest, via `ring` (already in the lockfile through
  rustls; now also under `box`). Every signature on a bundle must verify;
  at least one must be by a publisher in the box's trust store
  (`trusted-publishers` in the state directory, mode 0600, refused if
  others can read it, checked before the box is on the network). With
  `dev-bundles allow` there (open question `dev-bundles-switch`), unsigned
  and untrusted bundles import too, marked `DEV BUNDLE` everywhere.
- **Publishers** use `helmsman-plc bundle keygen | pack | sign | verify`
  off the box (PKCS#8 keys, mode 0600, never overwritten); integrators use
  `helmsman-plc bundle trust add NAME KEY`. No key is ever on a box.
- **Export**: `GET /api/snapshots/{id}/bundle`, unsigned, capability
  manifest computed, placeholder name and version (`sign` can rename it
  while unsigned).
- **Import**: `POST /api/bundles` (token, `application/octet-stream`):
  verify; bind every signal the bundle reads or drives to this box's by
  name (missing, a type or direction the template disagrees on, an output
  another module or the calendar already drives: refused with the whole
  report); then a **proposal** adding or replacing the bundle's modules in
  what runs. The bundle's map is a template only (the box's map is never
  replaced) and its calendar is not imported. From there the approval
  policy classifies it from the diff like any proposal: a bundle that drives
  anything is output-affecting (engineer PIN + replay). Every import,
  refused or not, is audited.

## Webhook sinks (`sink-webhook`, built)

`helmsman-plc --sinks FILE --sink-dir DIR` publishes signal events over
HTTP (`sink/`; contract in `tests/sink.rs`, `tests/sink_queue.rs`,
`tests/sink_bin.rs`). Modules never see the network: a module *emits* by
driving a signal (a `VAR_OUTPUT` bound to an `out` signal or to an
`in NAME TYPE module MODULE` signal), and the sinks file, which the core
owns, says which signals go where and when.

```text
sink   mes  https://mes.example.com/hooks/line3  timeout_ms=5000 queue_bytes=1048576
header mes  Authorization  file:/etc/helmsman-plc/mes.token
emit   mes  parts  change
emit   mes  jam    rise
emit   mes  level  every_ms=60000
```

- **Events.** Sampled once per runtime tick, after the scan. `change`
  (any new value; reals bitwise, no deadband), `rise`/`fall` (`bool`
  only), `every_ms=N` (late ticks skip missed periods). The first value
  after start is a baseline, not an event; a missing value never fires and
  keeps the baseline (false → missing → true is a rise). A change undone
  within one tick is not seen: count pulses in ST and publish the count.
- **Body.** One flat JSON object per event: `key`, `boot`, `seq`,
  `signal`, `value`, `trigger`, `ts_ms` (`null` while the clock is
  untrusted, as for the calendar). `key` (`SINK-BOOT-SEQ`) is also the
  `Idempotency-Key` header; `boot` is a per-sink counter persisted next to
  the queue, so a key is never reused across restarts, and a `seq` gap is
  a dropped event.
- **Never blocks the scan.** One thread per sink. The scan thread
  `try_send`s into a channel of 1024 events; a full channel drops and
  counts (`dropped_channel`).
- **Queue.** Bounded (`queue_bytes`), on disk, append-only records with a
  CRC-32 each; replay stops at the first torn or damaged record and cuts
  the file there, so a half-written append is never sent. Full → the
  *oldest* events go (counted): after an outage the receiver gets current
  state, not a stale morning. Compacted past twice the bound.
- **Delivery.** At-least-once, in order, one event at a time: queued and
  synced before the first attempt, removed only on a 2xx. 400/413/415/422
  drop that event (the receiver will never take it); anything else retries
  with exponential backoff (`retry_min_ms`..`retry_max_ms`, equal jitter
  from a seeded generator).
- **SSRF.** `http`/`https` only; no user info or fragment in the URL; the
  host is resolved once, every address is checked and the socket connects
  to a checked address. Refused always: link-local (`169.254/16`, where
  cloud metadata lives; `fe80::/10`), known metadata addresses, unspecified,
  multicast, broadcast, reserved, and IPv4 embedded in IPv6 that maps to
  any of those. Loopback and `localhost` only with `allow_local=true`.
  Plant-LAN addresses are allowed. Literal addresses are refused at load;
  names at each send. Redirects are never followed; the response head is
  capped at 16 KiB and the body never read; one deadline covers connect,
  TLS, write and read.
- **Secrets.** Credential headers (`Authorization`, `Cookie`, anything
  named like a token, key, secret or password) must come from `env:` or
  `file:`. Header values never print (`Secret` has no `Display`), and
  logs show only the URL's origin, never its path or query (a webhook path
  is often the credential).
- **TLS.** `https` is the `https` cargo feature (rustls with the ring
  provider and compiled-in Mozilla roots), on by default;
  `--no-default-features` builds an http-only box with no third-party
  crates.

## The live ST view (`st-graph-view`, built)

The operator's read-only view of what a module's code is doing
(`canvas-edit-scope`: ST is the source of truth, changes go through the
agent as ST; nothing here writes ST back).

- **Lowering** (`helmsman_diagram::lower`, from `helmsman_iec::ast`, the
  parser's tree exposed read-only). Per statement: reads and literals on the
  left, operators, functions and FB calls as blocks, the assignment on the
  right. A read of a path the POU already wrote (same region or an enclosing
  one) is wired from that write, so data flow crosses statements; an FB
  instance's outputs (`t.Q`, `Q => x`) are ports of its call block. Each
  IF/ELSIF/ELSE and CASE arm is a region gated by its condition (or the
  selector); each loop is a region gated by its head or condition. Every
  node, region and statement carries its `StmtId`, found by POU, line and
  column in the `TraceMap`, so a trace maps straight onto it.
- **Layout**: statements stack in source order, each a band laid out as a
  tidy tree layered by longest path (inputs left, assignment right); arms
  are boxes to the right of their conditions. Sizes come from label
  lengths, not fonts. Editing a statement moves nothing above it; the same
  source always lays out the same. No layout dependency.
- **Live**: `annotate` puts one `Trace` on a diagram (FB bodies viewed
  through one instance's prefix): variable values at scan end, logic and
  comparisons re-evaluated from them, arithmetic left blank; gates open,
  closed or not reached from arm hits and loop passes.
- **"On because"** (`explain`): the last assignment to the variable that
  ran, the gates around it with the conditions they needed TRUE or FALSE
  and their values, and the same question for every variable read, to a
  depth. FB outputs are followed into user FB bodies (and FB inputs back
  out to the caller's argument); library FBs report their inputs. No
  assignment ran: "held", with the closed gate that stopped each. Data, not
  prose: explain-mode phrases it.
- **Canvas**: "Live ST view" in the top bar, with two sources. **Box**
  (`canvas-web`; the default in a browser the box served): the module list
  and a module's source from the box API, its traces from
  `/api/modules/{name}/trace/events` and live values and module status
  from `/api/events`, over `fetch` and `EventSource`. The source is
  lowered against the statement table each trace carries
  (`lower_table`); a trace from the wire is a `BoxScan`, and the diagram
  reads any `live::Scan` (an in-process `Trace` or a `BoxScan`) alike,
  proven by rendering every scan of a run with the API's own renderer and
  comparing annotations and explanations. Connection status: connecting,
  connected, reconnecting, unreachable after 3 failures; a failed stream
  is closed and reopened after 2 s doubling to 30 s (the canvas's backoff,
  not the browser's). The decisions (`remote::BoxState`) are free of I/O
  and tested natively against the real API. **Demo** (`?demo`, native,
  or the button when the box is unreachable): a module runs in a
  `helmsman_plc::Runtime` on the page clock with a `TraceTap` asked for
  the last scan of every frame; scripted or hand-switched inputs. Pan,
  zoom, click a block for its source line, click an output for why.

## Proposal review (`proposal-ui`, built)

The screen an engineer approves on: "Proposals" in the canvas top bar
(`review.rs` decides, `review_view.rs` draws, `web::Http` fetches; contract
in `review.rs`'s tests and `review_tests.rs`, which runs the screen's own
requests against a real box: runtime, recorder, store, floor policy, PIN
file and token in a temporary state directory).

- **What it shows.** The drafts, newest first, each with the box's tier
  badge (`param`, `logic`, `output-affecting`: read from the draft's
  `approval`, never worked out in the browser), its reasons and what the
  policy needs. For a module draft: the manifest old against new (driven
  signals and the `%Q` window first, anything new that can move an output
  in the hazard colours), the source as a line diff with context, and the
  diagram diff. For a param draft: old → new and the declared range.
- **Diffs** (`helmsman_diagram::diff`, pure and deterministic). Lines: a
  longest common subsequence after trimming the common head and tail
  (middles past 4 M cells fall back to all-removed-then-added). Blocks: both
  versions are lowered; a block's identity is its POU, the kind and arm of
  each region around it, its kind and header; its signature adds the text
  it computes and what feeds each input. Equal signatures pair first, then
  equal identity and text, then equal identity (`changed`); the rest are
  added or removed. Regions pair the same way on their label, so a changed
  IF condition is one changed region, not a new body. Property-tested: a
  module against itself differs nowhere, and the old blocks less the
  removed and changed plus the added and the changed ones' new versions
  are exactly the new blocks. Removed blocks are drawn as dashed ghosts in
  their old layout below the proposed diagram.
- **Sim replay chart.** "Run the sim replay" posts `.../replay`; the
  answer now carries, per driven signal, both step tracks
  (`current_track`, `proposed_track`: type and `[at_ms, value]` steps, at
  most 2000 each, `truncated` past that; the summaries stay exact). Drawn
  in egui, no charting crate: what runs in steel, the proposal in yellow,
  rises marked, and the box's `extra_rises` as "would have fired N more /
  fewer times"; differences first.
- **Approve and undo.** Role picker, a masked PIN field, and the API
  token typed once per page load (memory only; `canvas-token` asks about a
  login exchange). Submitting moves the PIN into the request body and
  empties the field whatever happens; neither secret shows in a `Debug`
  output or a log. The box's refusals are shown verbatim with what to do:
  401 token (and the token is dropped), 403 role or wrong PIN, 428 replay
  missing or stale, 429 locked with a countdown from `Retry-After` (nothing
  is sent until it ends), 409 stale draft. After an apply: the new snapshot
  and the history head, and an undo (`/api/snapshots/{previous}/rollback`)
  through the same replay and PIN flow.

## Trust boundaries

| boundary | what crosses | enforced by | status |
| --- | --- | --- | --- |
| module ↔ module | nothing directly | per-module windows, compile-time bounds, `%Q` overlap rejected, per-module fault stop | built |
| module ↔ CPU | one scan's work | instruction budget per scan (`Watchdog` fault) | built |
| module ↔ network | nothing | ST has no network primitives; a module drives a signal and core-owned sinks publish it (address checks, no redirects, bounded queue, never blocking the scan) | built: webhook sink; OPC UA planned (`sink-opcua`) |
| module ↔ devices | signal values by name | the I/O map (only bound signals, one writer per point, writable points only), binding at load (one writer per output signal: a module or the calendar), hold-on-missing-input | built |
| driver ↔ box | strings and values | the `Driver` contract; WASM drivers get one capability, a socket to their own device's endpoint, plus fuel and memory limits | contract built; sandbox `wasm-driver` |
| box ↔ observed PLCs | tag reads | `helmsman-proto` without its `unrestricted` feature can only send reads: `Session::get_attribute_single` and `logix::TagReader`, whose `ReadRequest` has no constructor that takes a service code (no Write Tag, no `Set_Attribute`); `Session::request`/`request_raw` and the Class 1 scanner exist only with the feature. The `logix-tag` driver's points are not writable and its `write` is refused. `helmsman-plc/tests/box_build.rs` fails if the box build's resolved features ever include `unrestricted`. | built (`logix-tag-read`); the CLI, a bench tool, enables `unrestricted`; real-controller check waits for hardware |
| box ↔ scanner | assemblies 150/100 | the adapter's connection rules | built |
| agent ↔ box | proposals only | box API + approval policy: the agent drafts, a human with the right PIN applies; anything that can move an output needs an engineer PIN and a sim replay | built: box API (no write route; mutations need the token and are audited) and `approval-policy` (tier from the diff, policy floor in code, role PINs with lockout, replay bound to content hashes) |
| text ↔ agent | tag names, comments, bundle text, manuals | treated as data, never instructions: the relay's system prompt says so, and every tool result wraps plant text in a `<box-data>` envelope it cannot close; the tool set is fixed and has no apply; the host rules above hold regardless | built (`agent-loop`, `helmsman-plc/tests/agent.rs`) |
| box ↔ Claude | prompts, tool calls | the relay holds the key; boxes hold only a per-box token, sent only to the relay; the box API token never leaves the box | box side built (`agent-loop`); relay `relay-v0` |
| bundle ↔ box | modules, params, maps | signature + manifest checked by the host; AI review can only refuse | built: `bundle-format` (Ed25519 signatures against the box's trusted publishers, capability manifest held to the compiled modules, import only as a proposal through the approval policy); AI review planned (`marketplace`) |

The rule under all of these: limits are enforced by the host, never by the
prompt. The agent never touches the Rust core.

## Where the planned pieces attach

- `approval-policy` (built, above) → `proposal-ui` (built, above): the
  canvas shows a draft's `approval` (tier, reasons, what it needs), charts
  its replay, and posts the PIN to `apply`. `box-mcp` wraps
  the same routes; `local-accounts` replaces role PINs with people, keeping
  the role check and the gate.
- `canvas-web` (built, above) → `andon-screen`: another page the box
  serves from the same directory, on the same streams.
- `snapshots` (built, above) → `approval-policy` / `box-api`: apply =
  compile, check, swap, then `Store::commit` with the approver; the box API
  serves `list`, `diff` and `rollback`. `bundle-format` (built) exports a
  tree and its blobs; `marketplace` and `fleet-console` move bundles
  between boxes and put the AI review on top of `bundle::verify`.
- `sim-replay` (built, above) → `box-api` serves the recorder's tap and
  runs the replay; `proposal-ui` charts it; `approval-policy` requires one
  for logic and output-affecting changes.
- `agent-loop` (built, above) → `local-fallback` answers the chat route
  when the relay is unreachable (today: an `error` event); `explain-mode`
  and `dropin-prose` are prompts over the same tools, plus a trace tool
  for explain mode.
