# Architecture

Helmsman Factory Automation is a small soft PLC for a factory edge box (a Pi 5
is the reference), headless, with a browser UI. The agent writes Structured
Text modules, an engineer approves them, the box runs them, and the operator
watches them run as a live diagram. This page is the map every roadmap card
points at: what the parts are, which crate owns each, and where the trust
boundaries sit. Product intent lives in `TODO.md` ("Product vision",
"Roadmap M1..M4"); this page is how the code is shaped to deliver it.

Every part below is tagged **built** (on `dev`, tested) or **planned** (a
roadmap card, by id). Crates still carry the `tcpethernet-*` names until
`helmsman-rename` lands.

## The picture

```
                       ┌──────────────────────────── relay (planned: relay-v0) ─┐
                       │ per-box token · Anthropic key · metering · prompts     │
                       └──────────────▲─────────────────────────────────────────┘
                                      │ HTTPS, box-initiated
 ┌──────────────────────────── the box (Pi 5) ───────┼────────────────────────────┐
 │                                                   │                            │
 │  ┌─ agent loop (planned) ─┐   tools   ┌─ box API (planned) ─────────────────┐  │
 │  │ draft → check → sim →  ├──────────►│ signals · modules · proposals ·     │◄─┼── canvas in any
 │  │ propose                │           │ snapshots · outputs · audit         │  │   browser on the
 │  └────────────────────────┘           └──┬────────────┬───────────────┬─────┘  │   LAN (planned:
 │                                 policy + │ PIN        │ read-only     │        │   canvas-web)
 │                                          ▼            ▼               ▼        │
 │  ┌─ runtime: tcpethernet-plc (built: N modules, one scheduler thread) ──────┐  │
 │  │  module A (ST, 10 ms)   module B (ST, 100 ms)   module C (ST, 1 s)       │  │
 │  │     own %I/%Q/%M window, own scan rate, faults stop only itself          │  │
 │  │  ── interpreter + checker: tcpethernet-iec (built) ──────────────────────│  │
 │  └───────┬───────────────────────────────┬──────────────────────────┬───────┘  │
 │          │ process image (built)         │ signals by name          │ trace    │
 │          ▼                               ▼ (planned: io-runtime)    ▼ (plan-   │
 │  ┌─ CIP adapter ───────────┐   ┌─ I/O map + drivers: tcpethernet-io ─┐  ned)   │
 │  │ tcpethernet-adapter     │   │ IoMap · Driver · modbus-tcp (built) │         │
 │  │ assembly 150 in, 100 out│   │ WASM drivers (planned: wasm-driver) │         │
 │  └───────▲─────────────────┘   └───────┬──────────────┬──────────────┘         │
 └──────────┼─────────────────────────────┼──────────────┼────────────────────────┘
            │ EtherNet/IP, box is target  │ box is master│ box GPIO (do:N/di:N)
     a scanner (CODESYS,               remote I/O,      stack light, relays
     a Logix PLC, our CLI)             drives, sensors

  observed controllers (a CompactLogix): read-only, via tcpethernet (originator)
  (planned: logix-tag-read; no Write Tag in the box build)
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
| `tcpethernet` | CIP originator: encapsulation, CPF, discovery (`ListIdentity`), explicit messaging (`RegisterSession`, `SendRRData`, `Get_Attribute_Single`), Class 1 scanner. Byte codecs plus socket-level session, discovery and scanner. | built |
| `tcpethernet-adapter` | CIP target: a sans-IO `Device` state machine (every call takes `now`) and a std-thread server. Identity, assemblies, `Forward_Open`, cyclic I/O. | built |
| `tcpethernet-cli` | `tcpethernet discover / get / io`: the originator from a shell. | built |
| `tcpethernet-iec` | IEC 61131-3: ST lexer, parser, checker, interpreter; `PLCopen` TC6 XML (LD/FBD/ST) lowered to ST; the standard FB library; the ruSTy differential oracle (dev only). One `Program` scans one `Image` (`%I`, `%Q`, `%M` byte areas) with an instruction budget. | built |
| `tcpethernet-plc` | The runtime: a library (`runtime::Runtime`, the multi-module scheduler; `spec`, the `--module` syntax) and the binary that puts it behind the adapter. Grows into the box core (scheduler, signals, snapshots, policy, box API) unless a card splits it. | built |
| `tcpethernet-io` | The I/O layer: `IoMap` (devices, signals, bindings) and its validation, the `Driver` contract, the Modbus TCP driver, a device simulator, the conformance harness. | built (no scan integration yet) |
| `tcpethernet-graph` | Node-graph model and scan simulator behind the canvas; the model `st-graph-view` will lower ST into. | built (lab tool) |
| `tcpethernet-canvas` | egui canvas, native and wasm. Today an editor over `tcpethernet-graph`; per `canvas-edit-scope` it becomes the read-only ST view. | built (lab tool) |

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

Built in `tcpethernet-plc` (`runtime.rs`; contract in `tests/runtime.rs`).
On the command line a module is `--module PATH[,name=N][,scan=MS][,i=START+LEN][,q=START+LEN][,m=BYTES]`;
a bare program argument is a module with every default (whole image), so
existing deploys run unchanged. The `module-format` manifest replaces the
flag syntax later.

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

## The I/O layer, as built (`tcpethernet-io`)

- **`Device`**: an endpoint the box is master of: `id`, `driver` name
  (`modbus-tcp`), `endpoint` (`host:port`), driver params (`unit`,
  `timeout_ms`).
- **`Signal`**: what logic names: an identifier, a type (`Bool`, `Int`,
  `Real`), a direction (`In`, `Out`) and a **`Binding`**, either a point on a
  device in that driver's own address syntax (`coil:0`, `hr:100:f32`) or one
  of the box's own channels (`do:N`, `di:N`).
- **`IoMap::validate`** checks everything that needs no device: unique
  names, identifiers, known devices, parseable points, signal type matches
  point type, outputs only on writable points, one writer per point. It
  returns every problem, not the first.
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
  only pretends to write fails. `cargo run -p tcpethernet-io --example
  conformance -- --sim`.

Not yet wired: nothing in the scan reads the map. `io-runtime` adds a poll
task per device filling input signals, modules reading and writing signals by
name, outputs written on change, and a declared safe state when a device
drops.

## Trust boundaries

| boundary | what crosses | enforced by | status |
| --- | --- | --- | --- |
| module ↔ module | nothing directly | per-module windows, compile-time bounds, `%Q` overlap rejected, per-module fault stop | built |
| module ↔ CPU | one scan's work | instruction budget per scan (`Watchdog` fault) | built |
| module ↔ network | nothing | ST has no network primitives; sinks (webhook, OPC UA) are core-owned | built by construction; sinks planned |
| module ↔ devices | signal values by name | the I/O map: only bound signals, one writer per point, writable points only | map built; scan side `io-runtime` |
| driver ↔ box | strings and values | the `Driver` contract; WASM drivers get one capability, a socket to their own device's endpoint, plus fuel and memory limits | contract built; sandbox `wasm-driver` |
| box ↔ observed PLCs | tag reads | the box build exposes read services only (no Write Tag, no `Set_Attribute`) | planned (`logix-tag-read`); **today the `tcpethernet` library and CLI can send any service and drive Class 1 outputs** |
| box ↔ scanner | assemblies 150/100 | the adapter's connection rules | built |
| agent ↔ box | proposals only | box API + approval policy: the agent drafts, a human with the right PIN applies; anything that can move an output needs an engineer PIN and a sim replay | planned (`box-api`, `approval-policy`) |
| text ↔ agent | tag names, comments, bundle text, manuals | treated as data, never instructions, in the agent prompt; the host rules above hold regardless | planned (`agent-loop`) |
| box ↔ Claude | prompts, tool calls | the relay holds the key; boxes hold only a per-box token | planned (`relay-v0`) |
| bundle ↔ box | modules, params, maps | signature + manifest checked by the host; AI review can only refuse | planned (`bundle-format`, `marketplace`) |

The rule under all of these: limits are enforced by the host, never by the
prompt. The agent never touches the Rust core.

## Where the planned pieces attach

- `hot-swap`: replace one module's `Program` between its scans; its windows
  and the adapter connection stay up. Needs `multi-module`.
- `module-format` → `signal-model` → `box-api`: the manifest (signals read,
  outputs driven, schedule, params) is what the scheduler, the I/O map and
  the policy check; the box API serves it.
- `trace-tap` → `st-graph-view` → `canvas-web`: per-scan values from the
  interpreter, rendered on the ST diagram, served by the box.
- `snapshots`: each applied change set (modules, params, bindings) is an
  immutable, content-addressed record in SQLite; rollback is one call.
- `sim-replay`: a RAM ring buffer of signal values that a proposed module is
  replayed against before approval.
