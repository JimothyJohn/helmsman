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
 │  ┌─ agent loop (planned) ─┐   tools   ┌─ box API (built) ───────────────────┐  │
 │  │ draft → check → sim →  ├──────────►│ signals · modules · proposals ·     │◄─┼── canvas in any
 │  │ propose                │           │ snapshots · outputs · audit         │  │   browser on the
 │  └────────────────────────┘           └──┬────────────┬───────────────┬─────┘  │   LAN (planned:
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
| `helmsman-canvas` | egui canvas, native and wasm: the live ST view (a module running in-process, traced every frame) and the lab editor over `helmsman-graph`. | built |

Planned owners, not yet crates: the relay (separate stack, private per
`open-core-split`), the agent loop (behind the box API; the tool surface is the
box API, so it can run on the box or the relay), the WASM host (`wasm-tier`).

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
  `in NAME TYPE module MODULE` is another module's output. Older maps read
  unchanged; `box` and `module` are reserved device ids.

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
  device's rate. Any error marks the link down: its inputs go missing, and it
  forgets what the device holds, so every output is written again on
  reconnect.
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
  wrong content. A tree plus its blobs is what `bundle-format` will carry.
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

Not in the binary yet: the box API (below) stores proposals but applying
one waits for `approval-policy`, which will call the swap.

## Box API (`box-api`, built)

`helmsman-plc --api ADDR` (default `127.0.0.1:8080`, `0.0.0.0:8080` for the
LAN, `off`): one HTTP/1.1 + JSON surface for the canvas, the agent's tools
and later MCP (`api/`; contract in `tests/api.rs`). Endpoints are listed at
`GET /api` and in `api/mod.rs`.

- **Reads** (`GET`) are open: signals with live values and quality, modules
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
  commit, its snapshot id and its diff. `apply` and snapshot `rollback`
  answer 501 until `approval-policy` exists.
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
  arrives, 2 s for any write before a client is dropped. Audit log and
  proposals are bounded and refuse (507) rather than forget.

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
- **Canvas**: "Live ST view" in the top bar. A demo module runs in a
  `helmsman_plc::Runtime` on the page clock with a `TraceTap` asked for
  the last scan of every frame; scripted or hand-switched inputs. Pan,
  zoom, click a block for its source line, click an output for why.
  Native and wasm alike.

## Trust boundaries

| boundary | what crosses | enforced by | status |
| --- | --- | --- | --- |
| module ↔ module | nothing directly | per-module windows, compile-time bounds, `%Q` overlap rejected, per-module fault stop | built |
| module ↔ CPU | one scan's work | instruction budget per scan (`Watchdog` fault) | built |
| module ↔ network | nothing | ST has no network primitives; sinks (webhook, OPC UA) are core-owned | built by construction; sinks planned |
| module ↔ devices | signal values by name | the I/O map (only bound signals, one writer per point, writable points only), binding at load (one writer per output signal: a module or the calendar), hold-on-missing-input | built |
| driver ↔ box | strings and values | the `Driver` contract; WASM drivers get one capability, a socket to their own device's endpoint, plus fuel and memory limits | contract built; sandbox `wasm-driver` |
| box ↔ observed PLCs | tag reads | `helmsman-proto` without its `unrestricted` feature can only send reads: `Session::get_attribute_single` and `logix::TagReader`, whose `ReadRequest` has no constructor that takes a service code (no Write Tag, no `Set_Attribute`); `Session::request`/`request_raw` and the Class 1 scanner exist only with the feature. The `logix-tag` driver's points are not writable and its `write` is refused. `helmsman-plc/tests/box_build.rs` fails if the box build's resolved features ever include `unrestricted`. | built (`logix-tag-read`); the CLI, a bench tool, enables `unrestricted`; real-controller check waits for hardware |
| box ↔ scanner | assemblies 150/100 | the adapter's connection rules | built |
| agent ↔ box | proposals only | box API + approval policy: the agent drafts, a human with the right PIN applies; anything that can move an output needs an engineer PIN and a sim replay | box API built (no write route; mutations need the token and are audited; apply/rollback 501); `approval-policy` planned |
| text ↔ agent | tag names, comments, bundle text, manuals | treated as data, never instructions, in the agent prompt; the host rules above hold regardless | planned (`agent-loop`) |
| box ↔ Claude | prompts, tool calls | the relay holds the key; boxes hold only a per-box token | planned (`relay-v0`) |
| bundle ↔ box | modules, params, maps | signature + manifest checked by the host; AI review can only refuse | planned (`bundle-format`, `marketplace`) |

The rule under all of these: limits are enforced by the host, never by the
prompt. The agent never touches the Rust core.

## Where the planned pieces attach

- `box-api` (built) → `approval-policy`: the policy is the only thing that
  will answer `POST /api/proposals/{id}/apply` and
  `/api/snapshots/{id}/rollback` with anything but 501; the stored proposal
  already holds the change set to commit. `box-mcp` wraps the same routes.
- `st-graph-view` → `canvas-web`: the trace tap's samples (built, below)
  rendered on the ST diagram, served by the box.
- `snapshots` (built, above) → `approval-policy` / `box-api`: apply =
  compile, check, then `Store::commit` with the approver; the box API
  serves `list`, `diff` and `rollback`. `bundle-format` exports a tree and
  its blobs.
- `sim-replay`: a RAM ring buffer of signal values that a proposed module is
  replayed against before approval.
