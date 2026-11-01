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
 │  │ assembly 150 in, 100 out│   │ WASM drivers (built: wasm-driver)   │         │
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
| `helmsman-proto` | CIP originator: encapsulation, CPF, discovery (`ListIdentity`), explicit messaging (`RegisterSession`, `SendRRData`, `Get_Attribute_Single`), Logix tag reads (`logix`: Read Tag, Multiple Service Packet, Read Tag Fragmented, Symbol object browse, backplane routing), Class 1 scanner, the box's one Class 1 I/O endpoint (`endpoint`), EDS reader (`eds`). Byte codecs plus socket-level session, discovery and scanner. Any-service requests sit behind the `unrestricted` feature, the scanner behind `scanner` (which `unrestricted` implies). | built |
| `helmsman-adapter` | CIP target: a sans-IO `Device` state machine (every call takes `now`) and a std-thread server. Identity, assemblies, `Forward_Open`, cyclic I/O on its own I/O endpoint or the box's shared one (`Adapter::start_on`); layouts `opener_sample`, `moxa_e1213` and any EDS (`from_eds`); a simulated E1213 application (`e1213`); misbehaviour knobs for scanner tests; a Logix tag-service fixture (`logix::Controller`) for tests. | built |
| `helmsman-cli` | `helmsman discover / get / io`: the originator from a shell. | built |
| `helmsman-iec` | IEC 61131-3: ST lexer, parser, checker, interpreter; `PLCopen` TC6 XML (LD/FBD/ST) lowered to ST; the standard FB library; the ruSTy differential oracle (dev only). One `Program` scans one `Image` (`%I`, `%Q`, `%M` byte areas) with an instruction budget. | built |
| `helmsman-project` | A project's rules and its `.zip`, no I/O: names, file contents, `Files`, the hand-written ZIP reader/writer. Shared by helmsman-plc's `project` (folders on the box) and the canvas's simulation (projects in browser storage), so an archive moves between them byte for byte. Pure Rust (`miniz_oxide`), wasm-safe. | built |
| `helmsman-plc` | The runtime: a library (`runtime::Runtime`, the multi-module scheduler; `spec`, the `--module` syntax; `store`, snapshots in SQLite) and the binary that puts it behind the adapter. Grows into the box core (scheduler, signals, snapshots, policy, box API) unless a card splits it. | built |
| `helmsman-io` | The I/O layer: `IoMap` (devices, signals, bindings) and its validation, the `Driver` contract, the Modbus TCP driver, the box's channels (in memory until GPIO), a device simulator, the conformance harness, the map file format (`text`), and WASM drivers (`wasm`, the `wasm` feature: drivers compiled to WebAssembly, sandboxed to their device's socket). | built |
| `helmsman-diagram` | A module's ST lowered to a read-only FBD-like diagram (`st-graph-view`): nodes, wires, gated regions, each tagged with its `StmtId`; deterministic layout; a trace's values and gates on it; "on because …" as data. Depends only on `helmsman-iec`. | built |
| `helmsman-agent` | The chat panel's agent loop (`agent-loop`): the relay client (protocol v1, HTTP/1.1 on std plus rustls), a bounded SSE reader, assistant-turn assembly from Claude's stream events, the fixed tool set run against the box API, budgets. No dependency on the runtime: its tools are HTTP calls to the box API. Also the playground's local proxy (`examples/playground_proxy.rs`, a dev tool). | built |
| `helmsman-canvas` | The browser app (docs/UI.md), egui, native and wasm: an always-on header (the open project, Save, Deploy, Sign in; Proposals, Upload and Download behind its ⋯ menu) over the Editor (projects, files and program tree, the live diagram with forcing on a box, the ST on demand, I/O / Trend / Claude tabs) or the Monitor (status tiles, phone to wall TV, watch only). Where the program runs is a setting: in a browser the box served, that box's modules over its API; on docs/playground/ (or `?playground`, `?demo`, native) a simulated box in the page, against virtual I/O, with Claude on the visitor's key. | built |

Planned owners, not yet crates: the relay (separate stack, private per
`relay-repo`, in `../agent-relay`: client-agnostic, helmsman is one client). The WASM module host
(`wasm-tier`) is built inside `helmsman-plc` (`wasm.rs`, the `wasm`
feature), below.

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
  (`modbus-tcp`, `modbus-rtu`, `enip-io`, `logix-tag`), `endpoint` (`host:port`, or a
  serial port's path for `modbus-rtu`), driver params (`unit`,
  `timeout_ms`, and the serial settings).
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
- **`modbus`**: the Modbus application layer (points, PDUs, reply checks,
  `app`) over a `Transport`, and the TCP client; the driver is generic over
  the transport. From the spec (citations in `docs/SOURCES.md`).
- **`rtu`**: `modbus-rtu`, the same application layer on a serial line:
  RTU framing and CRC-16, t3.5 silence before each request, the response
  time-out, broadcast (unit 0: writes only, no reply, then the turnaround
  delay), units 1..=247. The receiver hunts for the expected unit and
  function and takes the first frame whose CRC checks, so noise and other
  units' replies are skipped; after a link error the line is drained until
  quiet for a whole time-out, so a late reply is never taken for the next
  answer (RTU has no transaction id; a reply later than about two time-outs
  still could be). CRC errors, truncated frames and time-outs are link
  errors; exceptions are point errors. The serial port is a `Line` trait:
  `rtu::serial` (termios through rustix, the `serial` feature) on the box,
  `rtu::pipe` (in memory) in tests.
- **`sim`**: a Modbus device with cross-wires (a coil driving a discrete
  input, like a wire on the bench) and injectable faults, over TCP; and
  `rtu::sim`, the same tables as an RTU unit on a pipe, with the serial
  faults (bad CRC, truncated frame, another unit answering, noise, a reply
  in pieces, late replies).
- **`harness`**: driver conformance, reported as `PASS`/`FAIL`/`SKIP` lines.
  Writes are proven through loops (output wired to input), so a driver that
  only pretends to write fails. Each simulator lists the faults that mean
  something on its transport (a cyclic transport also says how long a
  fault may take to show: `Faults::fault_window`). `cargo run -p
  helmsman-io --example conformance -- --sim` (TCP), `-- --sim-rtu` or
  `-- --sim-e1213` (EtherNet/IP).

- **`enip`**: `enip-io`, an EtherNet/IP adapter (remote I/O) the box
  masters over one Class 1 connection, from a built-in profile
  (`profile=moxa-e1213`), the vendor's EDS (`eds=PATH`, read by
  `helmsman_proto::eds`) or explicit points and sizes. Points are bits and
  little-endian words of the input and output assemblies (`di:N`, `do:N`,
  `in:BYTE.BIT`, `out:OFFSET:FMT`, ...); reads answer from the latest T->O
  packet, writes change the O->T image. No T->O data within the
  connection's timeout, T->O of the wrong size, or a refused `Forward_Open`
  is a link error: the driver closes the connection (releasing exclusive
  ownership) and reopens it with the outputs it held from the first packet.
  A point past an assembly or a value its format can't hold is a point
  error. Behind helmsman-io's `enip` feature, which helmsman-plc's `enip-io`
  feature turns on; the box build (`box`) leaves it out until the E1213
  bench passes (TODO.md `enip-in-box`), so a box map naming `enip-io` is
  refused as "not in this build" (`helmsman_plc::drivers::not_in_build`)
  and the E1213 runs over Modbus/TCP meanwhile. `scripts/lint.sh` lints
  helmsman-plc with `enip-io` so it keeps compiling.
  `enip_sim` (feature `enip-sim`) is a simulated Moxa ioLogik E1213 for
  the harness: `conformance -- --sim-e1213`. The device:
  `docs/devices/moxa-iologik-e1213.md`.
  On a box the links share the box's one Class 1 I/O port with its adapter
  (next item); a device line's `io_port=N` gives a link a socket of its own.

- **One Class 1 I/O port per box** (`helmsman_proto::endpoint`). The box
  is a target (scanners send its adapter O->T) and an originator (`enip-io`
  devices send it T->O), and both arrive at UDP 2222. `helmsman-plc` binds
  one `IoEndpoint` on `--bind:--io-port` (2222 by default) and hands it to
  the adapter (`Adapter::start_on`) and to every `enip-io` link
  (`helmsman_io::open_on`). One receive thread routes each datagram by the
  connection ID in its address item. Every ID that arrives is one the box
  chose: for point-to-point the consumer of each direction picks it (the
  target picks O->T, `OpENer` `cipconnectionobject.c`
  `ConnectionObjectGeneralConfiguration`; the originator's T->O ID from the
  `Forward_Open` is what the target produces with, `cipioconnection.c`
  `SendConnectedData`), so the endpoint allocates them for both sides
  (`Router::allocate`; the adapter's `Device` takes its O->T IDs from it)
  and an ID has one owner. Each route is bound to its peer's IP (the
  scanner that opened the adapter connection, the device a link opened), as
  `OpENer`'s `HandleReceivedConnectedData` checks the sender. Unknown IDs,
  the wrong sender and datagrams that don't parse are dropped and counted
  (`IoEndpoint::stats`), never delivered. Each owner (the adapter, each
  link's connection) has its own queue of `QUEUE_LEN` (256); full, it drops
  its oldest datagram (cyclic I/O wants the newest), and one owner's flood
  never touches another's queue. Sends go out through the same socket, so
  devices and scanners see the one port. Closing or timing out a
  connection drops its route. `--io-port` moves the whole endpoint; off
  2222 the links' `Forward_Open` carries a T->O socket-address item (CPF
  0x8001) naming the port, which targets honour (`OpENer`
  `OpenProducingPointToPointConnection` takes its port, else 2222), and the
  box's adapter honours one from its scanners the same way. A real scanner
  that sends to 2222 without one still needs the box on 2222. Tests:
  `helmsman-proto/tests/endpoint.rs` (the router as a property, spoofing,
  overflow), `helmsman-adapter/tests/shared_io.rs` (adapter and originator
  flowing together, churn, a junk flood), `helmsman-plc/tests/bin.rs`
  (`the_adapter_and_an_enip_io_link_share_one_io_port`).

- **`logix`**: `logix-tag`, read-only signals from an observed Logix
  controller by tag name (`in temp real plc Program:Main.Temp@real`); the
  `@TYPE` is the tag's Logix type and is checked on every reply. Reads are
  batched into one Multiple Service Packet per cycle.

- **`local`**: the box's own channels (`do:N`, `di:N`, and
  `i2c:BUS:ADDR:REG[:FMT]` integer inputs from a sensor register) in
  memory, the default. **`board`**: the same channels on the board: a
  `Board` driver over `Gpio` and `I2c` traits (direction rules, formats,
  errors: a line that won't request or a sensor that doesn't answer is a
  point error), and `board::linux` (Linux, the `gpio` feature): GPIO lines
  through the character device's uAPI v2 (gpiocdev) and `/dev/i2c-N` with a
  combined write-then-read (i2cdev). `helmsman-plc --gpio-chip
  /dev/gpiochipN` picks it; the chip is checked at start. Both features are
  in helmsman-plc's `box`, so the canvas's wasm build has neither.
- **`text`**: the map as a file, one `device` / `in` / `out` line each, with
  optional `unit=`, `poll_ms=` (inputs) and `safe=` (outputs) in any order;
  `in NAME TYPE module MODULE` is another module's output, `out NAME TYPE
  screen TILE` an output no device takes (the retired floor screen's
  tiles, below). Older maps read unchanged;
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
param    speed   real 1.5 min=0 max=10 unit=m/s
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

## WASM modules (`wasm-tier`, built)

A module directory may hold `module.wasm` instead of `module.st`, with
the same manifest (`wasm.rs`, `logic.rs` in `helmsman-plc`; contract in
`tests/wasm.rs`). The runtime holds either as a `Logic` (`St` or `Wasm`),
so scheduling, binding, hold-on-missing, faults, `swap` and `reconfigure`
are the same code for both. Behind the `wasm` feature, which `box`
turns on; the canvas's wasm build never has it.

- **Interface `helmsman/v1`** (versioned by its import module names). A
  module imports `helmsman/v1/in NAME : () -> T` per input,
  `helmsman/v1/out NAME : (T) -> ()` per output, optionally
  `helmsman/v1 now_ms : () -> i64` (the scan's time on the runtime's
  clock, the only clock), and exports `scan : () -> ()`. `T` is `i32`
  (`BOOL`, 0 or 1), `i64` (`LINT`) or `f64` (`LREAL`). Each import is a
  port, named and typed like a `VAR_INPUT`/`VAR_OUTPUT`, so the manifest
  is checked against the imports exactly as against ST (a module whose
  manifest lies is refused before the adapter starts) and ports bind to
  signals by name at load. Any other import (WASI, a memory, a table, a
  global, another function) refuses the module; so do a `start`
  function, f32 ports, a port name that is not an identifier or is used
  twice, and a missing `scan`. No windows: a WASM module's manifest has
  no `i`, `q` or `m`.
- **Runtime: wasmi** (a pure-Rust interpreter; eager translation at
  load, fuel metering, NaN canonicalisation). Chosen over wasmtime for
  the floor of a soft PLC: deterministic fuel per scan without a JIT,
  no executable memory, a small footprint, and the same behaviour on the
  Pi as in CI. Measured (`examples/wasm_bench.rs`, release, M-series
  Mac): a small module's tick 0.16 µs (ST 0.30 µs); a 1 000-pass loop
  16 µs (ST 185 µs); load and instantiate 21 µs. The box binary grows
  about 2.1 MiB (7.9 MB to 10.1 MB, macOS aarch64 release).
- **Limits** (`wasm::Limits`): 1 000 000 fuel per scan (about one unit an
  instruction; out of fuel is a `Watchdog` fault, as ST's budget), linear
  memory capped at 4 MiB (more declared is refused at load, `memory.grow`
  past it returns -1), one memory, one table of at most 10 000 elements.
  Fixed for every module for now (`wasm-limits` asks about per-module
  ones).
- **Faults**: a trap stops the module and sends its outputs safe, as an
  ST fault does; the others keep their schedule. Out-of-bounds memory or
  table access is `IndexOutOfRange`, integer division by zero
  `DivisionByZero`, a `BOOL` output other than 0/1 or a non-finite real
  `OutputRange`, anything else (`unreachable`, overflow, a stack
  overflow in the module) the new `Trap` kind.
- **State**: outputs keep their last value until set again. A swap
  carries outputs and exported mutable globals by name and type; linear
  memory restarts from its data segments. Across kinds (ST to WASM) nothing
  carries. A WASM module is not traced (the tap's request stays open) and
  `Runtime::program` is `None` for it.
- **Stored, proposed and bundled like ST** (`wasm-store`): the snapshot
  store and bundles carry `module.wasm` as a binary blob under its SHA-256
  (a `wasm-module` tree line), a proposal carries it as base64 (`wasm` in
  place of `source`), and the tier comes from the manifest as for ST, which
  is sound because nothing applies until the binary loads and its imports
  match the manifest (`compile_set`). Where the API shows a source it shows
  the module's imports instead (`render::wasm`: interface, size, SHA-256,
  every import's module, name and type); `/api/modules/{name}` keeps the
  ST comment naming size and SHA-256, beside the ports. Over the box API a
  binary is capped by the 1 MiB request body (about 750 KiB of WASM);
  the store and bundles take up to 1 MiB.

## WASM drivers (`wasm-driver`, built)

The `Driver` contract across the same interpreter (wasmi, same version
and features as the module tier), so a driver the agent writes reaches
its own device and nothing else. `helmsman_io::wasm::WasmDriver`, behind
helmsman-io's `wasm` feature (off by default, so the canvas never has
it); the interface is in the module docs of `wasm.rs`.

- **Interface `helmsman-driver/v1`.** The guest exports `memory`,
  `buffer(len) -> ptr` (where the host puts a call's input), `parse`,
  `read` (addresses as `u16` LE length + bytes), `write` (address, a type
  code and the value's 64 bits) and optionally `configure` (the device's
  params as `key=value` lines). It answers through imports: `point`
  (parse), `value` (once per address, in order), `fail` (an error kind
  that maps onto `Error`, a code, a message of up to 256 bytes).
- **One capability**: `send(ptr, len)`, `recv(ptr, len)` (exactly `len`
  bytes) and `close()`, a TCP stream to the device's endpoint from the
  map. No argument names a peer, so there is no other host or port to
  ask for; any other import (WASI, a clock, a memory, a table, a global,
  a `connect`) and a `start` function refuse the guest at load, every
  problem listed (`wasm::check`). `parse` and `configure` get no socket.
  Time-outs are the host's: each socket wait is at most `timeout_ms`, a
  whole call at most that times one more than its addresses.
- **Limits** (`wasm::Limits`): 1 000 000 fuel per call, linear memory
  capped at 1 MiB (declaring more is refused, `memory.grow` past it
  traps), one table of at most 10 000 elements, 64 KiB per `send`/`recv`
  and per call's input, addresses of up to 255 bytes.
- **Faults.** A trap (fuel, the memory cap, a pointer outside its memory
  handed to an import, `unreachable`) or a contract breach (more or fewer
  values than addresses, `parse` answering nothing, a `BOOL` of 2) fails
  the call with `Error::Driver`, a point error: the link reading the
  device marks that point bad (`io-point-errors`) and keeps the others,
  and the runtime carries on. The instance and its connection are
  dropped; the next call starts a fresh one.
- **The first guest**: `crates/helmsman-io/guests/modbus-tcp/`, the
  Modbus/TCP driver ported to `no_std` Rust (same point syntax, PDUs and
  reply checks, no dependencies), 10 KB of WASM. It is not a workspace
  member; `scripts/build-wasm-drivers.sh` rebuilds it for
  `wasm32-unknown-unknown` and the `.wasm` is committed beside it, so the
  tests need no wasm toolchain. It passes the conformance harness the
  native driver passes, and agrees with the native driver, on the same
  simulator and on hostile peers, call for call (property tests in
  `tests/wasm_driver.rs`).
- **Files, named by the map** (`wasm-driver-map`, `wasm-driver-home`:
  "a file path on the box"; `helmsman_plc::driver_dir`, contract in
  `helmsman-plc/tests/driver_map.rs`). A map device line `device ID
  wasm:NAME ENDPOINT ...` runs on the file `drivers/NAME.wasm`, looked
  for in the project first (`<state-dir>/projects/P/drivers/`: the
  project being deployed, else the one `running-project` names), then in
  the box's folder (`<state-dir>/drivers/`, in every mode, a store or
  none). **The first file found is the one**: a project that carries a
  driver runs the bytes it was written and tested with wherever it is
  deployed, the box's folder supplies what a project leaves out, and a
  bad project file is an error, never a fall back to the box's.
  - **Names** are slugs (`helmsman_project::driver_name`: 1..=48 of `a-z
    0-9 _ -`, starting with a letter or digit), so a name is one path
    segment and two never differ by case alone; `wasm:../x`,
    `wasm:Modbus` are refused as "not a driver name" before any file is
    looked for.
  - **Read safely**: regular files only, at most 256 KiB
    (`helmsman_project::MAX_DRIVER`); a symbolic link (the file or a
    `drivers/` folder) is refused, never followed, and on Unix the file
    opened must be the one looked at (device and inode; a rename racing
    the read is read again).
  - **Checked on load** as a driver always was: `wasm::check` and an
    instance started under the default limits without a device
    (`wasm::sandbox`: memory within the cap, segments in bounds,
    `configure` not called), then each point parsed by the guest loaded
    with its device's params (`drivers::Points`, even on a device no
    signal uses). A bad file is a map error naming the device and the
    file (`device rio: wasm:modbus: drivers/modbus.wasm in the box:
    ...`) and nothing loads; a name no file answers is "no such WASM
    driver (no file drivers/NAME.wasm in the project or on the box)".
  - **When a change runs**: files are read when a map loads (at boot,
    and by every apply, rollback or deploy, which reads each file the
    target map names once and loads exactly those bytes), never while
    one runs. A link's identity includes its driver's SHA-256
    (`LinkSet::plan_with`), so a changed file under an unchanged map line
    replaces that device's link at the next load, between scans (outputs
    the new link still has are written by it, as for any changed device;
    the old guest's connection is not carried). A step whose loaded
    driver bytes differ from what the links run on (added, dropped, or
    the file changed) is `output-affecting` (`policy::classify_with`),
    whatever else it changes, so a changed file never rides in on a
    param or logic change; the apply's `reload.drivers` says which bytes
    loaded from where (`{name: {from, sha256}}`). Rollback restores a
    map, not driver bytes: it loads the files as they are then.
  - **The box's folder** is managed by an engineer (the `/api/drivers`
    routes below): uploads are held to the load's check first, then
    written atomically (a temporary file renamed over the old), at most
    64 files; a file the running map loads from the box (no project
    driver in front of it) is not deleted (409).
  - **A project's** `drivers/` travels in its `.zip` as
    `drivers/NAME.wasm` (at most 16, files and drivers together within
    the project's 900 KiB), is copied with it, and is checked as a box
    upload is when an archive is installed (a bad one refuses the whole
    archive). The browser's simulation runs no WASM driver: an archive
    with one is refused there (`helmsman_project::from_zip`).
  - **Not in snapshots or bundles.** A change set holds no driver; a
    bundle carries none (its capability manifest still lists `wasm:NAME`
    under `drivers`, as the map names it) and one an older build wrote
    with a `wasm-driver` line is refused as malformed, saying so. A store
    written when drivers were store blobs (#89, schema 3) still opens:
    a snapshot with `wasm-driver` lines is read without them (their blobs
    still checked; `store::Loaded::ignored_drivers`, logged at boot and
    on every read through the API), and rolling back to one commits that
    content as its own snapshot, recorded as a rollback to the old id
    (`tests/fixtures/store-v3-drivers`).
  - The API shows a driver as interface, size, SHA-256 and problems,
    never the binary. The box ships none built in: the Modbus guest is a
    test fixture (`tests/driver_map.rs` reads the simulator through it
    from a driver file, end to end through the box runtime).

| route | who | body / answer |
| --- | --- | --- |
| `GET /api/drivers` | engineer | `{drivers: [{name, map_name, bytes, sha256, ok, problems}]}` |
| `PUT /api/drivers/{name}` | engineer | the `.wasm` bytes (`application/wasm` or `application/octet-stream`); 201 new, 200 replaced: `{name, ..., replaced, takes_effect}`; 400 name, 413 size, 415 type, 422 `{error, problems}`, 409 a link in its place, 507 full |
| `DELETE /api/drivers/{name}` | engineer | `{deleted}`; 409 while the running map loads it from the box |

- **Not yet**: stream sockets only: a protocol over UDP would need a
  datagram import. Adding a driver to a project is an archive upload (no
  per-file route).

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

## Floor screen (`andon-screen`, retired)

The box-driven andon view (a `--screen FILE` layout of tiles, state codes
and colours, served at `GET /api/screen` and drawn by the canvas at
`/?andon`) is retired: the Monitor replaced it and builds its tiles from
the program, not from a layout file (card `andon-layout-after-monitor`).
helmsman-plc refuses `--screen` with that reason, `/api/screen` is a 404,
and the startup line for `/?andon` is gone; `?andon` opens the Monitor.
The code is in git history (`crates/helmsman-plc/src/screen.rs`,
`crates/helmsman-canvas/src/andon.rs`).

What stays: `out NAME TYPE screen TILE` still parses in an I/O map
(helmsman-io's `Binding::Screen`), so an old map loads and reloads
unchanged. Such an output is one no device takes: no link polls or writes
it, a module drives it through a `VAR_OUTPUT` of that name under the
one-writer rule and safe values, and the Monitor shows it as one of the
program's outputs. No layout has to fit it any more.

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

## Trends on the box (`box-trend-source`, built)

The Editor's Trend tab on a box opens with the last minutes of each pinned
signal, then streams (`api/trend.rs`, contract in `tests/trend.rs` and the
`api.rs` and canvas `box_projects_tests.rs` tests; the canvas side is
`box_trend.rs`).

- **Not the recorder.** It keeps only what modules read, never their
  outputs, and a pinned lamp is an output. Instead a ring per tracked
  signal, fed from the views the scan thread publishes (the values
  stream's source; the API's accept loop samples the newest one every
  stream period, so the scan never waits): a point when the value
  changes, on the scheduler's clock.
- **Bounds.** 10 min a signal (the value the window opens on kept), 2400
  changes a signal, 64 signals at once (one more is refused, never
  swapped for another's): about 7 MiB worst case. At most 16 signals a
  request (400 past that); names the box has no signal for come back
  `unknown` and are never tracked.
- **Who pins.** Signed in, the person's pins (an account; a role-PIN
  session pins for its role) are kept by the box: `GET /api/trends/pins`
  and `PUT /api/trends/pins {signals}` (a login session; the box token is
  no one, 400), at most 16, 256 people, in memory (card
  `trend-pins-persist`). Signed out, the browser keeps them
  (`localStorage`, wrapped). `GET /api/trends?signals=a,b` (open, like
  every read) answers `{now_ms, window_ms, signals: [{name, points: [[t_ms,
  value]]}], unknown, refused}` and keeps those signals tracked for a day
  after the last ask; the page asks on open, on every pin change and every
  10 min. A person's pins stay tracked while pinned.
- **The tab** draws up to six pins as steps over the box's window,
  extended by each values event (`at_ms`), the last value held to now.

## Snapshots (`snapshots`, built)

`helmsman_plc::store` (`store.rs`; contract in `tests/store.rs`) keeps
every applied change set in one SQLite file, `DIR/snapshots.db`;
`helmsman-plc --store DIR` runs whatever head is (and takes no other
module, map or calendar source).

- **A change set is files, byte for byte**: each module's
  `module.manifest` and its program, `module.st` or `module.wasm` (params
  live in the manifest), the I/O map (the bindings) and the calendar. Commit checks that each manifest
  parses and names its module and that the map parses; it does not compile
  (whoever applies compiles first).
- **Content-addressed**: blobs are keyed by SHA-256; a snapshot's id is the
  SHA-256 of a canonical text tree (`helmsman-snapshot 1`, then one
  `module NAME MANIFEST SOURCE` or `wasm-module NAME MANIFEST WASM` line
  per module by name, `io-map H`, `calendar H`). Same content, same id, in
  any store; an ST snapshot's id is what it was before WASM modules. Blobs
  are bytes; the tree says which are text, and one read as text that is not
  UTF-8 is an error. Every read re-hashes
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
  file carries an application id and `user_version` (schema 3), and a
  newer schema is refused rather than misread. Schema 2 is schema 1's
  tables plus `wasm-module` lines: a schema 1 store opens as it is and is
  marked 2 by the first commit holding a WASM module, so an older build
  keeps opening it until then and is told to upgrade after
  (`tests/fixtures/store-v1` is a store from before). Schema 3 added
  `wasm-driver NAME H` lines (drivers as store blobs, #89); this build
  writes none and reads them as "WASM drivers" says (the driver left out
  and logged). Writers take SQLite's write
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
and MCP (`POST /mcp`, below) (`api/`; contract in `tests/api.rs`). Endpoints are listed at
`GET /api` and in `api/mod.rs`.

- **HTTPS (`box-https`, built; `api/tls.rs`, contract in
  `tests/https.rs`).** On by default (`--https on|off`; a build without
  the `https` feature is plain HTTP). At first start the box makes a
  P-256 key with `ring` and a self-signed X.509 v3 certificate for it,
  written in `api/tls.rs` rather than by a crate (rcgen is not in the
  lockfile, and a few hundred bytes of DER do not need one): `CN=helmsman-plc`, valid to
  9999-12-31, CA false, serverAuth, names `localhost`, the host's name,
  `127.0.0.1` and `::1`. Cert and key are one PEM file, `tls.pem` (mode
  0600) in the state directory, made once (atomically: starts racing
  settle on one) and then only read, so the fingerprint stays across
  restarts and upgrades. A file that does not hold a matching cert and
  key stops the box rather than being replaced: a new certificate means
  every browser trusting it again, so it is the operator's call (delete
  the file). The SHA-256 fingerprint (as browsers and `openssl x509
  -fingerprint -sha256` print it) goes to the journal at every start and
  is at `GET /api/policy` as `transport.fingerprint`, which the canvas
  shows in its ⋯ menu. The person compares it once with what the
  browser shows, then trusts the certificate. rustls (ring, TLS 1.2 and
  1.3, ALPN `http/1.1`) serves it on the API's own port: the first byte
  of a connection decides. A TLS handshake (`0x16`) is TLS. Plain HTTP
  from this host (loopback, or the box's own address: scripts, the
  agent's own calls, `curl` in the deploy script, an SSH tunnel) is
  served as before. Plain HTTP from anywhere else is never served: a
  `GET`/`HEAD` is redirected (301) to `https://` on the Host it asked
  for, anything else is 403. A client mid-handshake gets nothing back,
  not even an error, and every connection stays under the request
  deadline whatever it sends; at the worker cap a box with HTTPS drops
  the connection instead of a plain 503. Relative URLs and the
  `EventSource` streams follow the page's scheme, so the canvas needs
  nothing.

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
  journal), or a login session from `POST /api/login` (below). A token,
  password, PIN or session in a query string is refused on any request.
  Every authenticated call is appended to the audit log whatever its
  outcome, with the snapshot id it concerns and, for an account's
  session, the person.
- **Browser sessions (`canvas-token`, built).** The box token is for
  machines (scripts, the agent, MCP); the browser never holds it.
  `POST /api/login` takes `{user, password}` on a box with accounts, or
  `{role, pin}` on a box without (the role's PIN through the same gate
  apply uses: lockout, 1 KiB cap, constant-time check; refused, and
  counted toward nothing, once an accounts file exists), JSON only (415
  otherwise: a cross-site form cannot send it). A login carrying
  `X-Helmsman-Request: 1` and a same-origin proof (below) is the
  canvas's: the session comes back only as `Set-Cookie:
  helmsman_session=<64 hex>; HttpOnly; SameSite=Strict; Path=/api;
  Max-Age=3600`, never in the body, so no script on the page can read
  it; a login over a live cookie ends that session first (rotation).
  Without the header the session comes back as `token` in the body, for
  `Authorization: Bearer`, as before. The server keeps only the token's
  SHA-256, in the accounts session table (1 h, 15 min idle, 8 per
  account or per role, 256 per box); a role-PIN session is the role's
  (audited with the role and no person; history says `ROLE (PIN)`) and
  ends at its next use once an accounts file appears. Over the box's
  HTTPS (below) the cookie is also `Secure`; over plain HTTP (this host
  only, or a box run with `--https off`) it is not.
  `GET /api/session` says who the cookie (or a bearer session) is, so a
  reloaded page knows; `POST /api/logout` ends it on the box and sends
  `Max-Age=0`; any 401 on a cookie also clears it.
- **CSRF.** A request authenticated by the cookie (any method but `GET`,
  which is a read) must carry exactly one `X-Helmsman-Request: 1` (a
  cross-site form cannot send a custom header; a cross-site `fetch` with
  one needs a preflight the box never answers) and an `Origin` equal to
  the scheme the request came over (`https://` over TLS, `http://`
  otherwise) + its `Host`, or, with no `Origin`, a `Referer` on that
  origin; anything else (missing or wrong header, another site's or an
  opaque `null` origin, two origins, a look-alike `http://box.evil/`
  referer, a form post) is 403 before the session is looked up, so a
  forgery neither uses nor refreshes it, and is only logged. The
  canvas's `fetch` sets `referrerPolicy: same-origin` on its own calls:
  under the pages' `Referrer-Policy: no-referrer` a browser would send
  `Origin: null` even to the same origin.
- **Precedence.** An `Authorization` header, when sent, is the
  credential and the cookie is not read: a wrong token is 401 whatever
  cookie rides along, and a bearer is never held to the cookie's
  same-origin rule (no other site can make a browser send one). `/mcp`
  takes the box token only (and sits outside the cookie's path). A
  cookie the box did not issue (made up, the box token, one character
  off, two `helmsman_session` cookies at once) is 401.
- **No write path.** There is no route that writes a signal, an output, a
  param or a module. `POST /api/proposals` stores a checked draft (a module
  directory's two files, or one param's value): parsed, compiled against its
  windows, held to its manifest, its signals checked against the box, and,
  when the box runs a change set, carrying the change set applying it would
  commit, its snapshot id, its diff and its tier. The only way a draft runs
  is `apply`, the only way back `rollback`, both through the approval
  policy (below). A project deploy (`projects`, below) is an apply of
  the project's build, through the same path, by an engineer's session.
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
  builds, on the API's origin (no CORS). Only `GET`; a path is refused
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
  (bindings, devices, safe values), the WASM driver bytes the map loads
  added, dropped or changed (its file; it is what reaches a device), a module's `drives` or `%Q` window
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
  A PIN past 1 KiB is wrong without being hashed as typed (PBKDF2
  rehashes a long key every iteration).
- **Accounts replace PINs (`local-accounts`, built).** `policy/account.rs`,
  `api/login.rs`; contracts in `tests/accounts.rs`. `accounts` in the
  state directory (`helmsman-accounts 1`, mode 0600, refused if others may
  read it, hand-parsed): one `USER ROLE enabled|disabled pbkdf2-sha256
  ITERATIONS SALT HASH` line per person, the PIN file's hash and
  iteration floor. `helmsman-plc account add USER ROLE | passwd USER |
  disable USER | enable USER | list` edits it, passwords on stdin only.
  **Once the file exists the box refuses every role PIN** (403, even a
  right one) and approvals come from a login session; without it, PINs
  work exactly as before. The file is read again on every login and every
  use of a session, so disabling an account, changing its password or
  role, or removing the file ends its sessions at their next use, without
  a restart; a file that will not load stops the box at startup, and one
  that breaks while it runs refuses every login, session and PIN (503).
  `POST /api/login {user, password}` (no token) answers a session token:
  32 random bytes, kept server-side only as their SHA-256, ending 1 h
  after login, after 15 min unused, on `POST /api/logout`, or when the
  account changes; at most 8 per account (a further login ends the
  oldest) and 256 per box. A wrong password counts toward a lockout of
  that account (the policy's `lockout`, doubling to 1 h) and toward a
  box-wide one (4 times the allowance within an hour, any accounts,
  known or not); checks are serialized; an unknown user costs the same
  PBKDF2 as a known one; compares are constant-time; a disabled account
  looks like a wrong password. A session carries its account's role: an
  apply or rollback with one sends only `{replay?}`, and a role or PIN
  beside it is refused (400), so nothing in a request raises it. A
  session goes wherever the box token goes (replays, drafts, chat;
  `session-scope`), except `/mcp`, which takes the box token only and
  audits no person. So does a role-PIN session on a box without
  accounts: knowing the operator PIN now opens what the box token opens,
  except apply above the operator's tier.
  Logins that worked and wrong passwords are audited with the account; a
  name is logged or audited only if it is an account (a password typed
  as a user name never lands in a log); refusals while locked are only
  logged, so an unauthenticated flood cannot fill the audit log.
  `GET /api/policy` says which applies (`credentials`: `pins`,
  `accounts`, `unavailable`); the canvas's review screen signs in with a
  user and masked password there, or with a role and its masked PIN on a
  box without accounts, and the session is the browser's `HttpOnly`
  cookie (Box API, above), never the page's.
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
  `POST /api/snapshots/{id}/rollback` take a login session (an
  account's, or a role-PIN one) and `{replay?}`, or (no accounts) the
  token and `{role, pin, replay?}`; in
  order: target known; draft not stale (its base is what
  runs, else 409); tier and rule; role (the session's, or the one named)
  at least the rule's (403, no PIN
  spent); evidence (428); PIN (403/429; a session's password was checked
  at login); every module compiles (422); another map or calendar
  loads (422) and the modules bind to it (409). Then the scan thread runs
  `Runtime::reconfigure` between ticks (or, with another map or calendar,
  the reload below) (refused: nothing changed, 409; not
  taken within 5 s: the job is withdrawn and can never land later), the
  store commits the target with author, prompt, approver (`USER (ROLE)`
  for a session, `ROLE (PIN)` for a PIN) and
  time, and the API's view of the box moves to it; if the commit fails the
  scan thread is swapped back (500). Applies are serialized, so of two
  racing conflicting proposals one wins and the other is stale. Needs
  `--store`. The response names the previous snapshot as the one-call
  undo.
- **Audited, every path.** Every apply, rollback and replay, refused or
  not, is an audit entry with time, status, role (once the body parsed),
  the person (`user`, for every call made with a login session), and
  target and base snapshot ids; an apply that acts writes one entry before
  (so a full log stops it) and one after.

## Map and calendar reload (`map-reload`, built)

An apply or rollback whose I/O map, calendar or WASM driver files differ from
what runs reloads them on the running box, through the same approval path
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
- **`screen` outputs** (the retired floor screen's tiles) carry like any
  other signal; with no layout file there is nothing they must fit.
- **Around it.** Sinks rebind their emits by name; the API's catalog,
  device list and replay table follow. The recording restarts, so the
  next replay's window starts at the reload. A published live view from
  just before the catalog moves can be read against the other one for a
  moment (display only).

## Projects on the box (`projects`, built: server side)

The editor's projects (docs/UI.md "Projects", build step 2):
`helmsman_plc::project` (folders, rules, `build`, `zip`) and
`api/projects.rs` (routes); contract in `tests/projects.rs`.
`helmsman-plc --projects` turns it on: projects live in
`<state-dir>/projects/<name>/` (`/var/lib/helmsman-plc/projects/` on a
Pi), the store defaults to `<state-dir>/store/`, and the box boots the
store's head, or nothing (zero modules, the adapter up) before a first
deploy. `--projects` takes no PROGRAM, `--module`, `--module-dir`,
`--io-map` or `--calendar`.

- **A project is a folder** of free-form `.st` files (any PROGRAMs,
  FUNCTION_BLOCKs, FUNCTIONs and TYPEs, compiled together), plus an
  optional `io.map` (the I/O map it runs with, `helmsman_io::text`), an
  optional `diagram.layout` (the diagram's shared arrangement, below;
  the build ignores it) and an optional `project.settings` (`author`; `program NAME scan_ms=
  i= q= m=`; `param PROGRAM NAME TYPE VALUE ...`, a manifest's param
  line), and optional WASM drivers its map names, `drivers/NAME.wasm`
  ("WASM drivers"; they come in with its `.zip`). Many are stored; one
  runs; editing or saving one never touches what runs (a driver file is
  read at the next deploy or map reload, as the box's are).
- **Names and contents.** A project is 1..=48 of `a-z 0-9 _ -` (lower
  case, so a case-folding file system never merges two); a file is
  `NAME.st` (1..=64 of `A-Z a-z 0-9 _ -`, no case-only twins), `io.map`
  or `project.settings`. Checked after percent-decoding, so `..`,
  `%2e%2e`, `a%2fb`, `a\b`, `C:` and NUL name nothing. Contents are
  UTF-8 without NUL, other controls (tab, CR, LF aside) or bidirectional
  overrides (no Trojan Source in reviewed code); at most 256 KiB a file,
  64 `.st` files and 900 KiB a project, 256 projects. Symbolic links
  under the folder are never followed.
- **Writes** are atomic (temporary file, rename) and serialized for the
  whole folder, so a deploy or a download reads each file whole, before
  or after a racing save. A save may name the version (SHA-256) it read
  (`version`, or `null` for "new only"): a stale one is 409, so two
  editors never silently clobber each other.
- **Build** (`project::build`): each PROGRAM becomes a module named as
  the PROGRAM in lower case. Its `module.st` is every file in name
  order under a `(* file NAME *)` line with every other PROGRAM blanked
  to spaces (line breaks kept), so it is one self-contained unit with one
  PROGRAM and a module line is a line of one file: compile errors come
  back as `{file, line, col, message}`. Its manifest is generated:
  `reads` its signal `VAR_INPUT`s, `drives` its `VAR_OUTPUT`s, `author`
  the settings' (else `project NAME`), schedule and windows from the
  settings (else `--scan-ms` and no windows; the only PROGRAM of a
  project gets the whole image, like a bare program). The same files
  always build the same snapshot. The project's `io.map` is the map; the
  calendar is carried over from what runs. The WASM drivers its map
  names are its own `drivers/NAME.wasm` files, then the box's ("WASM
  drivers"): files, not part of the snapshot.
- **Deploy is an apply** (`approve::apply` with `Target::Project`): the
  build is the target change set, then the same steps as an apply:
  tier from the diff, the policy's role and replay rule
  (`POST /api/projects/{p}/replay` makes the evidence), compile, swap
  between scans (`swap.rs`, state carried by name), commit with author
  `project NAME`, approver the engineer. A project that does not build is
  422 with its diagnostics and nothing changes; rollback is the snapshot
  rollback it always was. A deploy records `NAME SNAPSHOT` in
  `<state-dir>/running-project`; `GET /api/running` says which project
  that is and whether it is still what runs (a later apply or rollback
  moves the head, and `current` goes false). The running project cannot
  be deleted; a rename carries the record.
- **ZIP** (`project::zip`, in the `helmsman-project` crate since the
  simulation uses it too; hand-written on APPNOTE 6.3.10; DEFLATE through
  `miniz_oxide`, already in the lockfile): a download is the files
  stored, sorted, with fixed timestamps, so download, upload, download is
  byte-identical. An upload (2 MiB body) is hostile: at most 128 entries
  and 900 KiB uncompressed (declared and actual; each entry inflates into
  at most its declared size, then CRC and length must match), no ZIP64,
  encryption, symlinks or other non-files, names UTF-8, local and central
  headers agreeing. One common top-level folder (a plain name, never
  `..`) is stripped and `__MACOSX/` and `.DS_Store` skipped; anything
  else must be a project file name at the top level or a WASM driver,
  `drivers/NAME.wasm` (NAME a slug; "WASM drivers"), so zip-slip names
  are refused, not cleaned. A refused upload writes nothing, and a
  replacing one leaves the old project whole until the new one is in.
- **Import** (`POST .../import?file=F`): `F.st` as it is; `F.xml`
  (PLCopen TC6) lowered to `F.st` by the existing importer; `F.L5X`
  (Rockwell ladder) converted to `F.st` by `helmsman_iec::ladder` (below,
  "L5X ladder import"), or 422 with every problem, each at its program,
  routine and rung (the first 20 in the message, then a count).
- **The engineer gate.** Every write (create, copy, rename, delete,
  save, upload, import, replay, deploy) needs a login session of an
  engineer or above (an account's, or the engineer's role-PIN session on
  a box without accounts). The box token alone is 403 there: the agent
  drafts proposals, never edits the editor's files. `check` takes the
  token or any session; reads are open like every `GET`. Every call is
  audited with the person.

Endpoints (also in `api/projects.rs` and `GET /api`):

| route | who | body / answer |
| --- | --- | --- |
| `GET /api/projects` | open | `{projects: [{name, running}], running}` |
| `POST /api/projects` | engineer | `{name}` or `{name, from}` (copy); 201 |
| `GET /api/projects/{p}` | open | `{name, running, files: [{name, bytes, version}], drivers: [{name, bytes, sha256}]}` |
| `DELETE /api/projects/{p}` | engineer | 409 for the running project |
| `POST /api/projects/{p}/rename` | engineer | `{to}` |
| `GET /api/projects/{p}/files/{f}` | open | `{name, text, version}` |
| `PUT /api/projects/{p}/files/{f}` | engineer | `{text, version?}` (a draft); `{name, version, bytes}` |
| `DELETE /api/projects/{p}/files/{f}` | engineer | |
| `POST /api/projects/{p}/check` | token or session | `{ok, snapshot, modules: [{name, file, reads, drives}], approval}`, or 422 `{diagnostics: [{file, line, col, message}]}` |
| `POST /api/projects/{p}/replay` | engineer | sim replay evidence, as `POST /api/proposals/{id}/replay` |
| `POST /api/projects/{p}/deploy` | engineer | `{replay?}`; as apply, plus `project: {name, recorded}` |
| `GET /api/projects/{p}/zip` | open | `application/zip` |
| `POST /api/projects/{p}/zip[?replace=true]` | engineer | `application/zip` body; 201 new, 200 replaced |
| `POST /api/projects/{p}/import?file=F[&replace=true]` | engineer | the file's bytes; 201 `{name, version, bytes}` |
| `GET /api/running` | open | `{project, snapshot, running, current}` |

**In the Editor** (`helmsman-canvas`: `box_projects.rs` decides,
`box_view.rs` draws, `projects.rs` is the Files sidebar the simulation
shares; contract in `box_projects_tests.rs`, against this API with real
accounts). A box without `--projects` (`GET /api/projects` 404) keeps the
module view. Otherwise:

- **Sign in** is in the header: the box's own login panel (user and
  password, or role and PIN), the session the browser's `HttpOnly`
  cookie. Signed out or below engineer, the ST is read-only and every
  write control is disabled, with why on hover; a 401 on any project
  call signs the header out.
- **Files** lists the projects (the running one marked), the open
  project's files (every file is read on open; a dot marks unsaved
  edits), and new, copy, rename and delete for both. A file rename is a
  write of the new name (`version: null`, never over another file),
  then a delete of the old.
- **Save** writes every edited file with the version it read; a 409 says
  someone else saved it and offers Reload (the box's text, the edits
  here dropped). **Deploy** saves first, then `check`, then `replay` when
  `approval.needs.replay` says so (or the deploy answers 428), then
  `deploy` citing it, and says what runs (project, modules, snapshot). A
  422 lists every error as `file:line:col`; a click opens the file at the
  line. A click on a block of the running project's diagram opens its
  file at its line (`(* file NAME *)` headers in the build).
- **Upload** (⋯): a `.zip` becomes the project its file name says
  (`projects::slug`), a taken name offers Replace; any other file is
  imported into the open project, and a refusal (an L5X outside the
  ladder subset, 422) is shown in the box's words. **Download**: the browser fetches `GET .../zip`.

**In the simulation** (`sim_projects.rs`, the same Files sidebar and
header; contract in `sim_projects_tests.rs`): projects live in the page's
`localStorage` (`helmsman-projects 1` JSON, every project held to the
rules on read; every access wrapped, and a page whose storage throws
keeps them in memory and says so). The built-in examples are read-only
projects (named after their PROGRAM): they open and edit like any other,
and Save keeps a copy under a free name. The build is the box's (one
`.st` as it is; several in name order under `(* file NAME *)` lines), so
errors and block clicks map to a file and line. Deploy compiles and runs
it in the page. Upload and Download use `helmsman-project`'s `.zip`;
`.st`, `PLCopen` `.xml` and `.L5X` import into the open project. An
L5X goes through the box's converter (`helmsman_iec::ladder`, wasm-safe),
so it becomes the same `.st` the box makes, or is refused whole with the
box's list of reasons (`sim_projects_tests.rs`).

**The diagram's arrangement** (`arrangement-persist`; `arrange.rs` in
the canvas, the file format in `helmsman_project::layout`; contract in
`arrange_tests.rs`, `box_projects_tests.rs` and `sim_projects_tests.rs`).
Two layers. The shared default is the project's `diagram.layout`
(`helmsman-layout 1`, one tab-separated `move` line per block: POU, which
of the blocks with that label and text, offset, label, text; at most 4096
moves, offsets within ±10⁶, refused whole when it does not read, on a
save and in an upload). On a box it is the running project's, read with
the project list and saved by Save as default as an ordinary file write
(engineer only, with the version read: a stale save is 409 and read
again); in the simulation it is a file of the browser-stored project
(not of a read-only example). Personal tweaks are offsets on top of the
default, kept in the browser's `localStorage`
(`helmsman-canvas/arrangements`, every access wrapped, at most 32 views,
the least recently changed dropped), keyed by view (`box:PROJECT`,
`box-modules`, `sim:PROJECT`). A new default reaches a screen with its
tweaks still on top. Blocks are matched by POU, label, text and
ordinal, so an edit that renumbers them keeps both layers; one
arrangement per POU serves every module that uses it. Reset layout drops
the tweaks; a second step stores the automatic layout as the tweak.

 `scripts/pi-deploy-plc.sh` installs the
binary and the canvas and runs `helmsman-plc --projects`
(`PLC_MODE=program` keeps one program file). Its unit runs `helmsman-plc
project seed NAME FILE.st` before every start (`project::seed`): only on
a first install (no project, nothing deployed) it makes the project and
commits its build as the store's first snapshot (approver `first
install`), so the box boots running `line`; afterwards it touches
nothing. `scripts/pi-account.sh` makes the engineer login.

### L5X ladder import (`l5x-import`, built)

`helmsman_iec::ladder` (`mod.rs` the converter, `rung.rs` the rung
neutral text, `lib.st` the Logix semantics in ST); contracts in
`helmsman-iec/tests/ladder.rs` (fixtures, refusals, hostile files) and
`tests/ladder_equiv.rs` (the proof below). Card `l5x-scope`: the ladder
subset converts, the rest is refused, and either the whole export
converts or nothing is written.

- **Subset.** XIC, XIO, OTE, OTL, OTU; branches (legs may be empty);
  ONS, OSR, OSF; TON, TOF, RTO, CTU, CTD, RES; EQU/EQ, NEQ/NE, LES/LT,
  LEQ/LE, GRT/GT, GEQ/GE, LIM/LIMIT (both the old and the v36
  mnemonics); ADD, SUB, MUL, DIV, MOD, CPT (`+ - * / MOD`, negation,
  parentheses), MOV/MOVE, CLR, COP between elements of one type; JSR to a
  ladder routine of the same program, without parameters, as the last
  instruction of its rung (inlined there under the rung condition, each
  call site a copy; recursion, more than 25 levels or more than 4 MiB of
  inlined text is refused). Tags of controller and program scope of type
  BOOL, SINT, INT, DINT, REAL, TIMER, COUNTER and arrays of them, with
  constant subscripts; bits of integers (`Word.5`); aliases of such tags
  (replaced by the tag they name).
- **Semantics** (each from RM003's execution table or flow chart, pages
  in docs/SOURCES.md): a rung runs left to right with a rung condition;
  a branch gives each leg the condition at its start and ORs the legs. A
  TIMER or COUNTER stays a structure (`LOGIX_TIMER`: PRE, ACC, EN, TT,
  DN and a hidden `LAST`; `LOGIX_COUNTER`: PRE, ACC, CU, CD, DN, OV, UN)
  the rungs read and write; TON/TOF/RTO/CTU/CTD are stateless blocks in
  `lib.st` that copy it in and out and follow the flow charts: ACC in ms
  from the scan's clock (a TON on `T#100000d` read once a scan, so a
  scan sees one time), overflow pinning ACC at 2147483647, a negative
  PRE or ACC faulting the program (Logix's major fault 4/34), counters
  wrapping with OV/UN and DN left alone while either is set. Numbers
  follow Logix's conversions: SINT and INT widen to DINT, an operation is
  REAL if any operand or the destination is, a REAL stored in an integer
  rounds to the nearest (ties to even) and keeps the low bits, narrowing
  keeps the low bits; integer DIV by zero gives Source A, MOD by zero 0
  (CPT's `/` and `MOD` too: RM003 does not say, see SOURCES). Before the
  first scan the PROGRAM runs Logix's prescan once (`LOGIX_PRESCANNED`):
  OTE clears, ONS/OSR arm, OSF clears, TON/TOF/RTO reset as their
  Prescan rows say, CTU/CTD set CU/CD, every called routine once.
- **Shape.** One file (`F.st`): a header with the notes (a periodic
  task's rate, renamed tags, tags left out), only the helpers it uses,
  then one `PROGRAM` per Logix task, named as the task, running its
  programs in order from each main routine. A program's `Input`/`Output`
  parameters of an elementary type are the PROGRAM's `VAR_INPUT`/
  `VAR_OUTPUT` (the box's ports, bound to signals); every other tag is a
  `VAR` with its decorated initial value (RM014 pp.43-46) and the
  timers' and counters' pseudo-operands (RM003 p.856). Program tags of
  two programs in one task that share a name, or that shadow a
  controller tag, become `PROGRAM_TAG`; a tag whose name ST keeps
  (`Log`, `Max`, `Limit`) becomes `Log_tag`. An export without tasks is
  one PROGRAM named as the controller. Every PROGRAM is compiled before
  it is returned: a converter fault is a refusal, never a broken file.
- **Refused**, each with its place: any other instruction (add-on
  instructions named as such), FBD, SFC and ST routines (Logix ST calls
  instructions IEC ST lacks), tags of other types (UDTs, LINT, STRING,
  CONTROL, module I/O such as `Local:1:I.Data.3`, also through an alias),
  indirect subscripts, produced/consumed and InOut tags, constant tags
  written, JSR with parameters or inside a branch, JSR to the main
  routine, programs in no task or disabled, event tasks, a controller
  tag two tasks use (the box's PROGRAMs share no memory), conflicting
  pseudo-operands, malformed rungs. The file passes the L5X reader's
  limits (no DTD so no entities or XXE, nesting 256, 8 MiB, 100 000
  items); rungs nest at most 32 branches and 1000 instructions.
- **Not reproduced:** math status flags and minor faults (no tag
  changes), `S:FS` and other system values (refused as unknown tags),
  forces (noted), the microsecond clock (a timer reads the scan's ms).
- **The proof** (`tests/ladder_equiv.rs`): random rungs from the subset
  (branches three deep, every instruction, a JSR to a subroutine, bits,
  arrays, SINT/INT/DINT/REAL mixes and extremes) run through a reference
  evaluator written in the test from RM003 and through the converted ST
  on the interpreter, with random inputs and scan times; every tag must
  match after every scan and a fault must come on the same scan (400
  property cases and 200 fixed seeds; the fixed run also checks that
  most programs run all 40 scans, so it is never vacuous). A real Studio
  5000 export is still to come (`l5x-real-export`).

### Forced inputs (`force-inputs`, built)

`helmsman_plc::force` (the set and how it applies) and `api/forces.rs`
(routes); contract in `tests/projects.rs` (`force` tests) and the
module's unit tests.

- **What.** An engineer pins an input to a value: an input signal (an
  `in` signal of the map or the calendar, never another module's output)
  with a value of its type, or an address of the box's input image
  (`%IXb.n`, `%IBn`, `%IWn`, `%IDn`, little-endian as the image is) with
  a value that fits. Forcing a forced point again replaces it; at most 64
  at once.
- **When.** Every scan, after the I/O read (the assembly copied into
  `%I`, the calendar evaluated) and before any module runs, the scan
  thread writes every force over what was read (`force::Reader::apply`).
  The API publishes a new immutable set on each change and bumps a
  generation; the scan picks it up with a `try_lock` and otherwise keeps
  applying the set it has, so it never waits on the API.
- **Who and how long.** `POST /api/forces` and the releases need an
  engineer session (the box token and lesser roles are 403). A force
  belongs to the session that made it (kept as the session token's
  SHA-256): it ends at that session's logout, and the API sweeps once a
  second for sessions that expired or were revoked (`Logins::live`, which
  never restarts a session's idle time). Every force ends when a project
  is deployed. Making, replacing and releasing are audited with the
  person; an automatic release is audited as the box's, with why.
- **Seen.** `GET /api/forces` lists them; `/api/signals` (and each
  signal) carries `forced`; the values stream carries `forced` per
  signal and a `forces` list; every trace (`GET .../trace` and its
  stream) carries `forces`, so the diagram marks forced points FORCED.
- **In the Editor.** For an engineer, an input signal's block on the
  diagram is a control: a BOOL flips, a number drags, and either forces
  it. A forced input is marked FORCED in the warning colour over its
  block and in the I/O tab, from the values stream; a banner over the
  diagram lists every force with release and Release all (disabled, with
  why, for anyone else). Address forces (`%IX...`) have no click: the
  diagram's reads are variables, and only map signals are clickable.

| route | who | body / answer |
| --- | --- | --- |
| `GET /api/forces` | open | `{forces: [{id, signal, address, value, by, at}]}` |
| `POST /api/forces` | engineer | `{signal, value}` or `{address, value}`; 201 the force |
| `DELETE /api/forces/{id}` | engineer | `{released: [id]}` |
| `DELETE /api/forces` | engineer | `{released: [ids]}` |

## Agent loop (`agent-loop`, built)

`helmsman-plc --relay URL --relay-token FILE [--relay-agent v3]` turns on
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

## MCP on the box (`box-mcp`, built)

`POST /mcp` on the box API's listener (`api/mcp.rs`; contract in
`helmsman-plc/tests/mcp.rs`) serves the chat agent's tools to an
engineer's own MCP client (Claude Code, Claude Desktop): `claude mcp add
--transport http helmsman http://localhost:8080/mcp --header "Authorization:
Bearer <token>"` on the box or through an SSH tunnel; from the LAN,
`https://<box-name>:8080/mcp`, with the client trusting the certificate
part of the box's `tls.pem` (Node: `NODE_EXTRA_CA_CERTS`), since plain
HTTP from another host is refused (Box API, HTTPS). Sources:
`docs/SOURCES.md`, MCP specification.

- **Transport.** Streamable HTTP, one JSON-RPC message per POST, answered
  with one JSON object, through the same bounded parser, worker cap,
  deadline and 1 MiB body as every route. Dual-era: a request carrying
  `_meta.io.modelcontextprotocol/protocolVersion` is served statelessly as
  2026-07-28 (`server/discover`; `MCP-Protocol-Version`, `Mcp-Method` and
  `Mcp-Name` must match the body, else `-32020`); `initialize` selects
  2025-11-25 or 2025-06-18, with no session minted, and later legacy
  requests name their version in `MCP-Protocol-Version`. `GET` and
  `DELETE /mcp` answer 405; a JSON array (batch) is refused. No stdio
  proxy: every MCP client this is for speaks HTTP with a header, so a
  `helmsman-plc mcp` binary would only add a second path to secure.
- **Auth and audit.** It is a mutating route: the box token (401 before
  any MCP parsing), never in the URL; an `Origin` other than the box's own
  is refused (403). Every message is one audit entry with source `mcp`,
  and every box API call a tool makes is audited as that route always is,
  also `mcp` (audit entries now carry `source`: `api` or `mcp`).
- **Tools.** The agent's eleven (`helmsman_agent::tools::NAMES`), by the
  same executor, which reaches the box through `tools::Caller`: the chat
  agent's goes over loopback HTTP, MCP's calls the API's handler in
  process and may reach only the reads and the four draft routes
  (`mcp::reachable`). No tool applies, rolls back or takes a PIN; approval
  stays a person on the canvas. Results are text inside the agent's
  `<box-data>` envelope, never `structuredContent`.
- **Drafts.** The protocol has no sessions, so drafts are server-minted
  handles in one workspace shared by every MCP client of the box (they
  hold one token): at most 32 (the oldest forgotten), dropped after an
  hour without a stateful call, watch-only once any MCP call reads a PLC
  program, until they expire.

## Offline chat (`local-fallback`, built)

Relay unreachable: installed modules keep running and the chat degrades
to rule-based param edits, with no model on the box (`fallback.rs`,
`api/chat.rs`; contracts in `tests/fallback.rs` and `tests/agent.rs`).

- **State, explicit.** The chat route keeps the agent's state: `unknown`
  until the relay is first asked, `online`, or `offline` since a time
  (unix seconds) with the reason (the relay error's kind and message, of
  the failure that started the outage). `GET /api/chat/status` serves it
  (`unconfigured` without `--relay`); `GET /api/events` sends an `agent`
  event with it on connect and on every change (the canvas's banner); a
  chat turn that takes the agent offline ends with a `status` event, and
  every message read offline starts with one.
- **Detection.** A turn that fails because the relay is unreachable,
  times out, answers 5xx, or refuses the box token (401/403) takes the
  agent offline; any other relay answer counts as online. A turn failing
  in its own way (a stream error, a malformed stream) does not.
- **Recovery, automatic.** While the agent is not online, a watcher thread
  probes the relay every 15 s (`GET {relay}/v1/health` with the box token;
  only a 2xx counts, card `relay-health`: the relay answers 200 when it
  reaches the model API with its key and 503 when it cannot, and the
  banner's reason then carries the relay's message and `upstream`
  category; a relay without the route (404) or any other answer stays
  offline; no conversation is sent, no tokens are spent), at once on
  start. Each chat message also probes once before it is read offline, so
  a reply goes back to the model as soon as the relay can serve it.
- **The grammar** (`fallback::parse`, hand-written, no regex): one command
  per message on one line, `set REF to VALUE`, `set REF = VALUE` or
  `REF = VALUE`; REF is `PARAM`, `MODULE PARAM` or `MODULE.PARAM`, a
  param's words joined by `_`; VALUE a number (`[+-]D[.D]`, no exponent,
  hex, inf or NaN) with an optional unit, or `on/off/true/false/
  enable(d)/disable(d)`. Names match exactly (ASCII case aside); a ref that
  reads as two params is refused with both, never guessed; near names
  (edit distance, prefixes) are only suggested. Ints take whole numbers,
  values must be in the manifest's range and differ from the current one,
  and a unit must be the param's own `unit=` (a few spellings accepted,
  never converted). Anything else, including more than 200 bytes, a
  second line, a character outside ASCII letters, digits, blanks and
  `_ . - + % = /`, or any trailing word, is refused whole with the reason
  and the editable params with their ranges.
- **Only a proposal.** An accepted command is posted to
  `POST /api/proposals` (`kind: param`) with the API token, like the
  agent's, so it is checked, stored as a draft, audited, and applied only
  by `apply` with whatever role and PIN the policy's `param` tier asks.
  The offline path applies nothing, and a param proposal cannot touch
  logic or an output's wiring. Offline messages do not enter the
  conversation; the model sees their proposals through its tools later.

## Drop-ins (`dropin-prose`, `dropin-plc`, `dropin-photo`, `dropin-pdf`, `dropin-pid`, built)

A person hands the agent a document instead of describing everything in
chat (`api/dropin.rs`, `helmsman_iec::l5x`, the agent's v3 tools;
contracts in `helmsman-plc/tests/dropins.rs` and
`helmsman-iec/tests/l5x.rs`).

- **Upload.** `POST /api/dropins?name=N[&kind=K]` (token, audited without
  its name or text) takes the document as the body: plain text or
  Markdown (`prose`), a Rockwell L5X export (`l5x`), a `PLCopen` TC6
  project (`plcopen`) or Structured Text (`st`), sniffed from the content
  unless `kind` says. At most 1 MiB, UTF-8 without control characters but
  line breaks and tabs. A photo or a PDF is known by its bytes (below);
  this route alone takes a body up to 2 MiB (`Limits::upload`, the
  relay's per-PDF cap; every other route keeps 1 MiB). The box keeps the
  newest 32 documents and 16 MiB in memory; a restart forgets them.
  `GET /api/dropins` lists them (no text); `GET /api/dropins/{id}?chunk=K`
  serves the text in 6 KiB chunks cut at line breaks; a PLC program is
  read once at upload into an inventory served by
  `GET /api/dropins/{id}/plc?view=overview|tags|routine|names` in pages of
  at most 20 KiB, so no tool result outgrows the agent's limit.
- **The L5X subset** (`helmsman_iec::l5x`, hand-written on the same XML
  reader as the `PLCopen` importer: no DOCTYPE so no entities, nesting
  capped at 256, 8 MiB and 100 000 items): the controller's name and
  processor; user data types with their visible members; add-on
  instruction names; controller tags and each program's tags with type,
  dimensions, alias target, usage, external access, constant and
  description; each program's main routine and routines, ladder rungs as
  neutral text with their comments, ST routines as lines, FBD and SFC by
  name only; tasks with their type, rate and scheduled programs. Every
  name must be a Logix identifier (40 characters at most); anything else is
  left out and listed as skipped. Descriptions are clipped to 1000
  characters, rung text and comments to 8000. Nothing gives the rungs
  meaning on the box: they are text for the agent to read. `PLCopen` and ST
  drop-ins come out in the same shape (each POU a program, its variables
  tags, its source one ST routine).
- **Chat.** `POST /api/chat` takes `dropins: [ids]` (at most 8, each one
  the box holds, else 404 before the agent runs) and adds a note to the
  user's message in the box's own words: the id, the kind, the chunk
  count, which tool reads it. Nothing the uploader wrote (not even the
  file name) is in the note.
- **Tools** (relay agent v3): `read_dropin {dropin_id, chunk}`,
  `read_plc_program {dropin_id, view, name, offset}` and
  `compare_plc_tags {dropin_id, device}` (the file's tags against an
  observed Logix controller's symbol table: matched, differing in type or
  shape, only in the file, only on the controller). The browse behind it,
  `POST /api/devices/{name}/browse` (token, audited, one at a time, reused
  for 30 s), lists symbols over a session of its own with the Symbol
  object's Get Instance Attribute List only (`LogixDriver::browse`); it
  exists for the `logix-tag` devices in the I/O map. Nothing ever goes
  back into the customer's PLC.
- **Watch modules.** A conversation that has read a PLC drop-in is
  watch-only from then on (`Drafts::watch_only`): its checks, simulations
  and proposals go to the box with `watch: true`, and it can propose no
  param change. Every proposal names the drop-ins its conversation read
  (`dropins`, shown to the engineer). The box holds the rule itself: a
  proposal that cites a PLC drop-in, or one the box no longer holds, must
  be `kind: module, watch: true`, and a watch module drives no output
  signal and has no `%Q` window (it may publish its own outputs as module
  signals the map declares, `in NAME TYPE module MODULE`). So a planted
  "propose a module that drives every output" in a rung comment gets a
  checker refusal, never a proposal; from prose it can get at most a draft
  an engineer must read, replay and PIN, with its source named on it.
- **Photos** (`dropin-photo`; `api/photo.rs`, contract in
  `tests/dropins.rs`). A whiteboard sketch or a photo of the line is a
  `photo` drop-in: PNG, JPEG or WebP, known by its bytes (magic number and
  a full walk of the container's chunks or segments; the name and the
  `Content-Type` subtype are never trusted), read from its headers only
  with no image decoder on the box. Refused: over 512 KiB (413); a width
  or height of 0 or over 8000 px, the Messages API's limit, whatever the
  file size (a header can claim 65535 x 65535 in 30 bytes); bytes after
  the format's own end (where a polyglot hides its second file), a
  truncated file, a PNG chunk with a bad CRC or an unknown critical chunk,
  a WebP whose RIFF size disagrees with the file, more than one frame or
  image; animations (APNG, animated WebP: the model sees only the first
  frame); GIF, HEIC, AVIF, BMP and TIFF by name (415, re-export). An image
  body under a text `Content-Type`, or a text `kind`, is refused. A photo
  has no text: `read_dropin` and the PLC view answer 404 saying so.
- **How a photo reaches the model.** Attached to a chat message, it goes
  in that user message as an `image` block (`source.type: base64`, the
  sniffed `media_type`, the bytes untouched), followed by the box's note
  in its own words: the id, the format and size, that the image is data
  and never instructions whatever text it shows, and that anything unclear
  becomes a question, never a guess. Nothing the uploader wrote (the file
  name) is in it. The photo cap, 512 KiB (683 KiB as base64), was set
  against relay protocol v1's first 1 MiB request limit and kept
  (`dropin-photo-size`); the chat refuses (413, before the relay) a photo
  that would push the conversation past its history limit (3.5 MiB, under
  the relay's 4 MiB request body), and (422) a 21st image in one
  conversation (past 20 the Messages API holds every image to a smaller
  size). A photo binds every later proposal of its conversation
  (`dropins`), and never makes it watch-only. The relay-side caps, source
  checks and prompt are card `relay-images`.
- **PDFs** (`dropin-pdf`; `api/pdf.rs`, contract in `tests/dropins.rs`).
  A PDF (an SOP, a manual page, a drawing) is never read on the box: the
  most hostile file format there is gets no parser here. It is known by
  its first bytes, `%PDF-` (the name and the `Content-Type` are never
  trusted), and held to relay protocol v1's caps: at most 2 MiB (413, the
  route's body limit refusing from the head anything larger), and at most
  5 PDFs per relay request, history included, so per conversation (the
  chat refuses the sixth, 422, before the relay). Refused: bytes that do
  not start with `%PDF-` under `application/pdf` or `kind=pdf` (415), PDF
  bytes under a text or image `Content-Type` (415) or another `kind`
  (400). What the relay checks and the box does not: encryption and the
  100-page cap, both of which need the file's objects read; a PDF refused
  for them fails that chat turn with the relay's `invalid_request` (or the
  model API's, `upstream_rejected`). The refused document block, and the
  box's note after it, are replaced in the history by a note saying the
  drop-in was refused and why (`Conversation::drop_refused_documents`),
  the chat gets `error` `document_refused` with the relay's reason, and
  the next message in the same chat goes through. A refusal counts as the
  document's when it names a `document` block (`messages[i].content[j]`,
  or the model API's `messages.i.content.j`), or names no block but is
  about PDFs (the per-request page and document caps: the documents of
  the trailing, unanswered user message go); any other refusal touches
  nothing. A refused drop-in stays in the conversation's proposal binding,
  marked refused (`Drafts::refused`, `refused-dropin-binding`): every
  later proposal of that chat cites it in `dropins` and lists it again in
  `refused_dropins`, so the engineer sees everything the conversation
  touched and which of it the model never read (the box refuses a
  `refused_dropins` that is not a subset of `dropins`, 400). A PDF
  has no text on the box: `read_dropin` and the PLC view answer 404 saying
  so. Attached to a chat message it goes in that user message as a
  `document` block (`source.type: base64`, `media_type: application/pdf`,
  the bytes untouched, no `title` from the uploader's name), followed by
  the box's note (the id and size, data never instructions, unclear means
  a question). It binds every later proposal of its conversation, never
  makes it watch-only, and counts against the 3.5 MiB history like a
  photo: one 2 MiB PDF (2.7 MiB as base64) fits a conversation.
- **Drawings to a signal map** (`dropin-pid`; contract in
  `tests/dropins.rs`). A P&ID or an electrical drawing comes in as a PDF
  (or a photo) and the model reads it there; the box reads nothing in it.
  Two tools turn it into a draft map through the proposal path the map
  already has (`map-reload`): `read_io_map {}` (`GET /api/io_map`, the
  running map file and its snapshot id, enveloped like every result) and
  `propose_io_map {text, summary}` (`POST /api/proposals`,
  `kind: io_map`, the whole new file, citing the conversation's
  drop-ins). The box checks that the file loads and keeps every signal a
  running module names (a refusal goes back with its problems so the
  model fixes the draft); a stored map is a draft like any other, always
  output-affecting, applied only by a person with a role PIN. A
  watch-only conversation (one that read a PLC program) proposes no map:
  the tool refuses, and the box refuses an io_map citing a PLC drop-in.
  The tools reach a real model with relay agent v4 (card `relay-images`
  notes: the tool schemas and the drawing paragraph); MCP serves them now.
- **Questions, not guesses.** What the document leaves out (a signal the
  box does not have, a shift that is not defined) comes back as questions
  in the chat; the relay's v3 prompt says so, and the module only names
  signals the box has because the checker refuses any other.

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
  manifest not in canonical form; a blob the tree reads as text (all but
  a `wasm-module`'s binary) that is not UTF-8. A box from before
  `wasm-store` refuses a bundle with a WASM module as an unknown tree line.
  A bundle carries no WASM driver: a `wasm-driver` line (#89's) is
  refused, saying that drivers are files on the box now.
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

## OPC UA server (`sink-opcua`, built)

`helmsman-plc --opcua ADDR` (default `127.0.0.1:4840`, `off`) serves the
box's signals and module outputs to SCADA and MES clients that pull
(`opcua/`; contract in `tests/opcua.rs` against an independent client,
`tests/opcua_fuzz.rs`). It is a sink like the webhooks: core-owned, and
modules never see it.

```text
Objects
├── Signals                    ns=1;s=signals
│   └── level                  ns=1;s=signals/level       Double, quality, source timestamp
│       └── EngineeringUnits   EUInformation (unit text, UnitId -1)
├── Modules                    ns=1;s=modules
│   └── fill                   ns=1;s=modules/fill
│       ├── Status             running | held | faulted: why
│       └── Outputs            ns=1;s=modules/fill/Outputs
│           └── pump           the signal its VAR_OUTPUT drives
└── Server                     NamespaceArray, ServerStatus (State, CurrentTime, ...)
```

- **Values.** `bool` → Boolean, `int` → Int64, `real` → Double. Inputs as
  last read, outputs as commanded, module outputs as last published.
  Quality comes from the signal table. A missing value is Bad and carries
  no value: `BadNoCommunication` for a device or box channel (the link is
  down), `BadOutOfService` for a module output while its module is held,
  faulted or not yet scanned. A value whose point has an error standing
  (a refused write) is `Uncertain`. The source timestamp is when the value
  or quality last changed, as published; it is null while the wall clock
  is untrusted, as for the calendar and the sinks.
- **Read-only.** Every variable has `AccessLevel` `CurrentRead` and
  `WriteMask` 0. Write answers `BadNotWritable` (or `BadNodeIdUnknown`).
  Every service that could change anything (`AddNodes`, `Call`,
  `HistoryUpdate`, ...) answers `BadServiceUnsupported`. The server reads
  only the published snapshot, never the runtime or the table.
- **Services.** GetEndpoints, FindServers, Create/Activate/CloseSession,
  Browse, BrowseNext, TranslateBrowsePathsToNodeIds, Read,
  Register/UnregisterNodes, Cancel, subscriptions (create, modify, delete,
  publishing mode) and monitored items (Value attribute, data change
  without deadband, queue of one), Publish, Republish (last 10 messages).
- **Security: None only.** `SecurityPolicy#None`, `MessageSecurityMode`
  None, anonymous identity; any other policy, mode or identity token is
  refused. Nothing is signed or encrypted. That is why the default bind is
  loopback, and why a non-loopback bind logs a warning at start. Putting
  it on the plant LAN is a choice to trust that LAN, the same choice as
  the API's open reads. Sign and SignAndEncrypt are an open question
  (`opcua-security` in TODO.md).
- **The scan never waits on it.** The scan thread offers a snapshot (one
  copy of the values, plus the box's shape only after start, an apply or a
  reload) with a `try_lock` at most every 100 ms, and wakes at least that
  often while OPC UA runs. Connection threads build the address space from
  the snapshot's shape. 100 ms is also the fastest publishing interval
  and the reported `MinimumSamplingInterval`.
- **Bounded.** Hand-rolled on `std::net`, no new runtime crates. 16
  connections (`BadTcpServerTooBusy` beyond), 64 KiB chunks, 1 MiB
  requests in at most 64 chunks, one session per connection, 8
  subscriptions per session, 1000 monitored items each, 1000 operations
  per request, 10 queued publish requests, 8 browse continuation points.
  Hello within 5 s, a channel within 10 s, an expired channel token or
  session closes. Decoding checks every length against the bytes left
  before allocating and stops nesting at depth 8. The connection is a
  bytes-in/bytes-out state machine (`opcua::Conn`), property-tested with
  random input, flipped bytes in valid exchanges and random request
  bodies: a malformed message ends in `ERR` or a `ServiceFault`, never a
  panic.
- **A port that won't bind stops the box** with a line naming
  `--opcua off`, as the API does (`opcua-bind-fatal` in TODO.md).

**Dependency review (why hand-written).** Checked 2026-10-30 against
crates.io, GitHub and RustSec:

| option | maintained | footprint | security policies | verdict |
| --- | --- | --- | --- | --- |
| `opcua` (locka99) | no: last release 0.12.0, 2024-01-01; moved to `async-opcua` | n/a | n/a | rejected |
| `async-opcua` 0.19.0 (freeopcua), pure Rust | yes, releases every few months (0.19.0 on 2026-07-18) | server: 157 crates (`cargo tree`): tokio, futures, chrono, regex, icu, deprecated `serde_yaml` + `unsafe-libyaml` | via RustCrypto `rsa` 0.9.10: RUSTSEC-2023-0071 (Marvin, network timing key recovery, unpatched) | rejected for the box; its client is the test oracle |
| `open62541` 0.12.1 / `-sys` 0.6.5 (C bindings) | yes (2026-10) | 19 MB of vendored C, bindgen (libclang) and cmake at build, 58 crates with build deps | mbedTLS feature | rejected: a C parser on the network behind FFI, and the arm64 cross-build cost |
| hand-written subset, None only | ours | no new crates (`ring`, already in the box, for random tokens) | none | **chosen** |

The only thing the libraries buy over the subset is the signed and
encrypted policies, and the pure-Rust one does that with an advisory
open on exactly that path. So the box ships None-only, and the secure
policies wait on that question. `async-opcua`'s client (`=0.19.0`,
`client` feature only) is a dev-dependency. The server is tested against
an implementation that isn't ours, so a misreading of the spec can't
pass on both sides. `tests/box_build.rs` fails if `async-opcua`, `rsa` or
tokio ever reach the box build.

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
- **NOT is a bubble**, the FBD convention, not a block: a boolean `NOT`
  is `Wire::negated` (a circle on the pin it feeds; `y := NOT x` puts it
  on the write's pin) or `Region::negated` (`IF NOT jam`). `NOT NOT x` is
  a plain wire, `NOT TRUE` folds into the constant `FALSE`, and FB
  outputs, array elements, bits, struct fields and BOOL functions get a
  bubble like any variable. NOT on a BYTE/WORD/DWORD/LWORD stays a block
  (a bitwise complement; a bubble would hide that every bit flips), as
  does NOT on an operand whose type the declarations do not give.
  Properties over generated programs: every negated wire carries a BOOL
  (by the compiled program's types), and no NOT block has a BOOL input.
- **Layout**: statements stack in source order, each a band laid out as a
  tidy tree layered by longest path (inputs left, assignment right); arms
  are boxes to the right of their conditions. Sizes come from label
  lengths, not fonts. Editing a statement moves nothing above it; the same
  source always lays out the same. No layout dependency.
- **Live**: `annotate` puts one `Trace` on a diagram (FB bodies viewed
  through one instance's prefix): variable values at scan end, logic and
  comparisons re-evaluated from them, arithmetic left blank; gates open,
  closed or not reached from arm hits and loop passes. A negated wire
  shows its source's value; the block it feeds sees the inverse
  (`Live::input`, `Live::gate_value`), and the bubble is outlined in the
  colour of what the consumer sees.
- **"On because"** (`explain`): the last assignment to the variable that
  ran, the gates around it with the conditions they needed TRUE or FALSE
  and their values, and the same question for every variable read, to a
  depth. FB outputs are followed into user FB bodies (and FB inputs back
  out to the caller's argument); library FBs report their inputs. No
  assignment ran: "held", with the closed gate that stopped each. Data, not
  prose: explain-mode phrases it. Negated gates read as such (`NOT JAM`
  with its inverted value), so the phrases are unchanged.
- **The highlighted path** (`drivers`): the same rules as `explain`, as
  sets of nodes, wires and regions of the diagram shown (one POU, one
  instance prefix): the assignment that ran last, everything wired into
  it, the gates around it (an ELSIF/ELSE with the earlier arms' conditions
  that had to fail), and the same for every variable read; held, the
  closed gates, their conditions and the skipped assignments; an
  instance's output, its last call. Each node and variable is visited
  once, so it runs every frame. Tapping an output, an assignment or a
  variable's block sets the "because" variable and the canvas draws its
  path at full strength and dims the rest; tapping empty space clears it.
  Contract: `tests/live.rs` (the latch's MOTOR is start, stop and the
  seal-in, nothing else), a property that the path holds the assignment
  `explain` names, and a headless canvas test in `st_view.rs`.
- **Canvas**: the Editor (docs/UI.md), with two sources. **Box**
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
  and tested natively against the real API. **Simulation** (the ST
  playground below: docs/playground/, `?playground` or `?demo`, native,
  or "Simulate in this browser" from the ⋯ menu or when the box is
  unreachable). Pan, zoom, drag blocks; a click on a block sends the ST
  editor ("ST", beside the diagram; read-only on a box) to its line;
  tapping an output shows its highlighted path.
- **Monitor** (docs/UI.md; `monitor.rs`, contract in `monitor_tests.rs`
  against the real API scanning `programs/line.st`). One `Feed` trait
  over both sources: a box's `BoxState` (module status from
  `/api/events`; while the Monitor shows, every module's traced scans on
  one stream, `GET /api/traces/events`, which holds one of the box's
  eight stream slots however many modules run, each module's source
  fetched and lowered against its trace) and the simulation's session.
  Tiles come from the source's declarations (`Shape`: the PROGRAM's
  outputs, then each user FB instance, nested) and the scan's values by
  path. A program whose newest scan is over 3 s old, or every program
  while the values stream is down or silent for 3 s, is drawn grey with
  the age: never a colour. It sends only GETs and stream opens (tested);
  leaving it closes the all-modules stream and reopens the Editor's.

## Explain mode (`explain-mode`, built)

Why a variable is in its state right now, in two layers. The library and
the box route are built; the canvas's Why panel and its "Ask why" were
removed (docs/UI.md: the diagram itself carries why, as the highlighted
path that drives an output).

- **The box's own words** (`helmsman_diagram::phrase`, no model, works
  offline and in the demo): the `explain` facts phrased by fixed
  templates. Assigned (the line, what it read, the gates that let it
  run, which change would stop that line), held (each skipped line and
  the gate that closed it), TON/TOF/TP in time (`blink turns ON after
  400 ms of its input being ON. Now blink.in is ON and blink.et is
  340 ms of 400 ms so far`), counters against their preset, edges,
  latches, FB arguments, CASE (the selector's value and the branch it
  picked), loops. Units come from the signal model, TIME values read as
  durations, and an input seen at one value over several scans says so
  (`unchanged in the scans seen over the last 12 s`: sampled, so "seen").
  It never claims more than the trace: every sentence lists the
  variables it names and they are all in the explanation (a property
  test over generated programs); "what would change it" only says which
  line would stop or start running, never what a value would become.
  The cited source lines are highlighted on the diagram.
- **Ask why** (`POST /api/explain {module, path}`, token or session, audited): the
  box explains its own newest trace of the module (not one the browser
  sends), streams those sentences as an `explain` event, then runs the
  agent loop on one user message: a fixed ask (no plant text in it) and
  the facts, sentences and cited source lines inside the `<box-data>`
  envelope. The loop runs with `tools::READ_ONLY` (list/read signals and
  modules): a draft, check, simulate or propose call is answered "not
  available in this mode", unrun, so an explanation stores nothing, not
  even a proposal. One question per conversation, none kept; its own
  budget (6 turns, 32k output tokens, 3 min); the chat's running slots.
  `--relay-explain-agent V` asks the relay for an explain profile
  (default `explain-v1`: the relay's read-only, floor-language profile;
  the chat defaults to agent `v3`, which adds the drop-in tools). `GET /api` says whether the box has an
  agent. The route takes the browser's session cookie as well as the
  token; no canvas screen calls it now.

## Proposal review (`proposal-ui`, built)

The screen an engineer approves on: "Proposals" in the header's ⋯ menu,
badged with the drafts waiting (the list is read every 30 s)
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
- **Sign in, approve and undo.** The left panel signs in the way the
  box's policy says: a role picker and a masked PIN, or (accounts) a user
  and a masked password. The session is the browser's `HttpOnly` cookie
  (Box API, above); the page knows only who is signed in (`GET
  /api/session` after a reload), sends every call same-origin with
  `X-Helmsman-Request: 1`, and never sees or takes the API token.
  "Sign out" ends it on the box. Submitting moves the PIN or password
  into the request body and empties the field whatever happens; neither
  shows in a `Debug` output or a log; a wrong one or a lockout (with
  when to try again) shows under the fields. Approving sends only the
  replay it cites: the session's role approves. The box's refusals are
  shown verbatim with what to do: 401 (the session ended, expired or the
  box restarted: back to the sign-in panel), 403 role, 428 replay missing
  or stale, 429 locked with a countdown, 409 stale draft. After an
  apply: the new snapshot and the history head, and an undo
  (`/api/snapshots/{previous}/rollback`) through the same replay and
  approval.

## Site view (`site-view`, built)

Boxes of one site find each other on the LAN, and any box's canvas lists
them all (`site.rs` in `helmsman-plc`, `site.rs` in the canvas; contracts
in `tests/site.rs`, `site_tests.rs`).

- **Opt-in.** `--site NAME` (off by default; needs `--api` with a fixed
  port), `--box-name NAME` (default: the host name). The box id is a
  labelled SHA-256 of the API token, first 8 bytes: stable, one-way.
- **Beacon** (hand-written, no mDNS crate): six fixed text lines
  (`helmsman-site 1`, site, id, name, API port, version), at most 512
  bytes, every 5 s to 239.255.72.77:41910 with TTL 1. One thread sends it
  and hears the others'.
- **Untrusted input; a box only lists.** Strict parse (exact lines and
  order, small alphabets, length caps); the peer's address is the
  datagram's source IP with the beacon's port; sources outside private,
  CGNAT, link-local and loopback IPv4 dropped; 100 datagrams a second
  looked at, the rest dropped unread; a peer refreshed at most once a
  second; 64 peers at most, a new id waits for one to expire (30 s); an id
  heard from a second address stays where it was and is flagged as a
  conflict. The box never answers a datagram, never connects to a peer,
  never forwards what it heard. Every drop is counted and served.
- **`GET /api/site`**: this box (beacon fields and module/device-link
  counts by state, no names or values), the live peers, the drop counts.
  It is the one route other origins may read (`Access-Control-Allow-Origin:
  *`, no credentials, no preflight), and with `--site` the canvas page's
  CSP allows `connect-src http:` (question `site-view-cors`).
- **No view for now.** The canvas's Site view (`?site`) was removed with
  docs/UI.md's clean-up; the site returns with a fleet view. The box side
  above is unchanged.

## ST playground (`playground`, built)

The public demo: the canvas's Editor with the simulation as its source
(the page `scripts/build-playground.sh` builds into `docs/playground/`,
which GitHub Pages serves from master as /helmsman/playground/; `?playground`
anywhere). The same screens as on a box (`editor.rs` lays them out for
both). No box, no server: everything runs in the page (`playground.rs`,
`playground_view.rs`, `claude.rs` in the canvas; contracts in
`playground_tests.rs` and `ui_tests.rs`).

- **Editor → program.** A multiline ST editor with line numbers; it
  compiles 0.35 s after the last keystroke with `helmsman_iec::compile`
  (image: 8 bytes of `%I` and `%Q`). Diagnostics are drawn on their line
  (dark red ground, the column's word underlined, the line number red) and
  listed under the editor. A source that does not compile leaves the last
  good program running. Four starter examples: blink (TON), start/stop
  latch (located), counter (CTU), traffic light (CASE + TON).
- **Run / Stop / Step**, scan period 1-1000 ms, Restart (cold). The
  program is a `helmsman_iec::Program` scanned directly on the page's
  clock (at most 500 scans a frame); the last scan of each frame is
  traced for the diagram. A fault stops it; Run or Step restarts it cold.
  Loading an example or Claude's code compiles it and leaves it stopped.
- **Virtual I/O** from the program itself: each `VAR_INPUT` a control
  (BOOL: a push button when named like start/stop/reset/pulse, else a
  switch; integers: slider and field in the type's range; REAL: field;
  TIME: ms slider), each `VAR_OUTPUT` a lamp or readout; `AT %IXn.m` /
  `%QXn.m` bits and direct `%IX`/`%QX` addresses in the code likewise. A
  value outside its type's range never reaches the program. The Trend
  tab: up to six pinned signals over the last 10 s.
- **The live diagram** is the box's (`st_view::Viewer`).
- **Claude** (its tab). Calls the Messages API from the page:
  `claude-sonnet-5-5` by default (Opus 5.5 and Haiku 5.5 selectable),
  streamed, API version `2026-10-21`, `max_tokens` 16000, effort medium,
  the fixed system prompt (the playground's dialect: what helmsman-iec
  supports and does not, how ports and `%IX`/`%QX` become I/O, "answer
  with one complete program in a ```st block") cached with
  `cache_control`, and `fallbacks: "default"` (beta
  `server-side-fallback-2026-07-01`; not for Haiku). Each question
  carries the editor's source and compile errors inside
  `<current_program>`/`<compile_errors>` tags the source cannot close,
  plus the last six exchanges. "Use this code" loads the last closed
  fenced block holding a whole `PROGRAM … END_PROGRAM` (ST-tagged first;
  64 KiB at most; no control characters) and never runs it.
- **The key.** Pasted into a password field, held in page memory; an
  explicit "Remember on this device" (off by default, with a warning)
  keeps it in `localStorage`, and unticking or Forget removes it. It goes
  only into the `x-api-key` header of a request to
  `https://api.anthropic.com/v1/messages` with
  `anthropic-dangerous-direct-browser-access: true`; never into a URL, a
  body or a log (`ApiKey`'s and the request's `Debug` redact it). The
  page's CSP allows `connect-src 'self' https://api.anthropic.com` only.
- **Local proxy** (`scripts/playground-serve.sh`): the playground built
  for `/`, served on 127.0.0.1 by helmsman-agent's `playground-proxy`
  example, whose `POST /proxy/v1/messages` adds `x-api-key` from
  `ANTHROPIC_API_KEY` and streams the answer back (the agent crate's
  rustls client, no new crate). The page finds it (`GET /proxy/health`,
  asked only when served from localhost) and hides the key field. It
  requires its own `Host`, a `x-helmsman-playground: 1` header (forcing a
  preflight it never grants), a same-origin `Origin` when one is sent,
  1 MiB bodies; it passes on only `anthropic-version` and `anthropic-beta`
  and serves files only from its directory.
- **Publishing.** The built page, loader and wasm (wasm-opt `-Oz` through
  trunk, LTO and `opt-level=s` for this build only: 4.7 MB, 1.9 MB
  gzipped) are committed under `docs/playground/`; rebuild with
  `make playground` after a canvas change. Live once dev is merged to
  master.

## Trust boundaries

| boundary | what crosses | enforced by | status |
| --- | --- | --- | --- |
| module ↔ module | nothing directly | per-module windows, compile-time bounds, `%Q` overlap rejected, per-module fault stop | built |
| module ↔ CPU | one scan's work | instruction budget per scan (`Watchdog` fault); a WASM module's fuel per scan (the same fault) and linear-memory cap | built |
| module ↔ network | nothing | ST has no network primitives, and a WASM module can import only `helmsman/v1` (its ports and the scan time: no WASI, sockets, files or clocks), refused at load otherwise; a module drives a signal and core-owned sinks publish it (address checks, no redirects, bounded queue, never blocking the scan) | built: webhook sink and the OPC UA server (read-only, SecurityPolicy None; `sink-opcua`) |
| module ↔ devices | signal values by name | the I/O map (only bound signals, one writer per point, writable points only), binding at load (one writer per output signal: a module or the calendar), hold-on-missing-input | built |
| driver ↔ box | strings and values | the `Driver` contract; a WASM driver (`helmsman-driver/v1`) imports only its socket to its own device's endpoint (it never names a host or port; no files, clocks or WASI, refused at load otherwise), and only in `read` and `write`, under fuel and memory limits and a per-call deadline; a trap fails that point (`Error::Driver`), not the link or the runtime; the binary is a file (`drivers/NAME.wasm`, a slug, regular files only, links never followed, checked at upload and at every load) | built (`wasm-driver`, `wasm-driver-map`; `helmsman-io/tests/wasm_driver.rs`, `helmsman-plc/tests/wasm_driver.rs`, `helmsman-plc/tests/driver_map.rs`) |
| box ↔ observed PLCs | tag reads | `helmsman-proto` without its `unrestricted` feature can only send reads: `Session::get_attribute_single` and `logix::TagReader`, whose `ReadRequest` has no constructor that takes a service code (no Write Tag, no `Set_Attribute`); `Session::request`/`request_raw` exist only with the feature; the Class 1 scanner (its own `scanner` feature, out of the box build for now, below) sends only `Forward_Open`/`Forward_Close` to an Assembly path. The `logix-tag` driver's points are not writable and its `write` is refused. `helmsman-plc/tests/box_build.rs` fails if the box build's resolved features ever include `unrestricted`. | built (`logix-tag-read`); the CLI, a bench tool, enables `unrestricted`; real-controller check waits for hardware |
| box ↔ scanner | assemblies 150/100 | the adapter's connection rules | built |
| box ↔ mastered EtherNet/IP adapters | Class 1 I/O: the device's output assembly, its input assembly | `helmsman-proto`'s `scanner` feature, which only helmsman-plc's `enip-io` feature turns on (out of the box build until `e1213-bench` passes): it sends `Forward_Open` and `Forward_Close` to an Assembly-class path and nothing else (no other service, no `Session::request`); only devices in the I/O map, through `enip-io`, with the map's one-writer rule and approval tier. `tests/box_build.rs` checks the box build has neither `scanner` nor `unrestricted`, and that `enip-io` brings `scanner` alone. | built (`enip-io`, out of the box); real-device check `e1213-bench` |
| box ↔ LAN peers | site beacons in, this box's beacon out | strict parse, source-address rule, intake and refresh limits, a bounded list, no takeover of a live id; the box only lists peers and never connects to one; `/api/site` is the only cross-origin read | built (`site-view`) |
| agent ↔ box | proposals only | box API + approval policy: the agent drafts, a human with the right PIN applies; anything that can move an output needs an engineer PIN and a sim replay | built: box API (no write route; mutations need the token and are audited) and `approval-policy` (tier from the diff, policy floor in code, role PINs with lockout, replay bound to content hashes) and `local-accounts` (per-person logins that replace the PINs once an accounts file exists; the audit names the person) |
| text ↔ agent | tag names, comments, bundle text, manuals, drop-in documents, PLC programs and photos | treated as data, never instructions: the relay's system prompt says so, and every tool result wraps plant text in a `<box-data>` envelope it cannot close; the tool set is fixed and has no apply (explain mode: reads only, and the box writes the ask itself, the plant text only inside the envelope); a conversation that read a PLC program proposes only watch modules, and the box refuses a watch module that drives an output; a photo goes as an image block of a format, size and shape the box read from its headers, with the box's note saying it is data; the host rules above hold regardless | built (`agent-loop`, `explain-mode`, `dropin-prose`, `dropin-plc`, `dropin-photo`; `helmsman-plc/tests/agent.rs`, `tests/dropins.rs`) |
| browser (canvas) ↔ box | sign-in, drafts' replays, approvals, Ask why | the session is an `HttpOnly; SameSite=Strict; Path=/api` cookie the page cannot read (no API token in the browser); every cookie call but a `GET` needs `X-Helmsman-Request: 1` and the box's own `Origin` (or `Referer`), else 403; a bearer, when sent, wins and the cookie is not read; the box serves HTTPS with a self-signed certificate whose fingerprint the person checks once (journal, ⋯ menu), the cookie is `Secure` over it, and plain HTTP is served only from the box's own host | built (`canvas-token`, `box-https`; `helmsman-plc/tests/https.rs`, `helmsman-plc/tests/accounts.rs`, the canvas's `review_tests.rs`) |
| engineer's MCP client ↔ box | tool calls, plant data | `POST /mcp`: the box token, an `Origin` check, the agent's fixed tool set run in process against only the read and draft routes (no apply, rollback or PIN), results in the `<box-data>` envelope, every call audited as `mcp` | built (`box-mcp`; `helmsman-plc/tests/mcp.rs`) |
| playground visitor ↔ Claude | the visitor's own key, program and question | the key only in `x-api-key` to api.anthropic.com (redacted `Debug`, page CSP `connect-src` to that host and self), memory unless "remember" is ticked; the program and errors in tags it cannot close; Claude's code only loaded into the editor, never run; or the local proxy holds the key | built (`playground`) |
| box ↔ Claude | prompts, tool calls | the relay holds the key; boxes hold only a per-box token, sent only to the relay; the box API token never leaves the box | box side built (`agent-loop`); relay `relay-v0` |
| bundle ↔ box | modules, params, maps | signature + manifest checked by the host; AI review can only refuse | built: `bundle-format` (Ed25519 signatures against the box's trusted publishers, capability manifest held to the compiled modules, import only as a proposal through the approval policy); AI review planned (`marketplace`) |

The rule under all of these: limits are enforced by the host, never by the
prompt. The agent never touches the Rust core.

## Where the planned pieces attach

- `approval-policy` (built, above) → `proposal-ui` (built, above): the
  canvas shows a draft's `approval` (tier, reasons, what it needs), charts
  its replay, and posts the PIN (or, with accounts, the login session) to
  `apply`. `box-mcp` (built, above) wraps the same routes;
  `local-accounts` (built, above) replaces role PINs with people, keeping
  the role check and the gate.
- `canvas-web` (built, above) → the Monitor (built, above; docs/UI.md,
  step 3): a screen of the same app, on the same values stream plus one
  stream of every module's traces; it replaced the andon view.
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
  (built, above) runs the same loop on read-only tools; `dropin-prose` is
  a prompt over the full set.

- `agent-loop` (built, above) → `local-fallback` (built, above) answers
  the chat route by rule when the relay is unreachable; `explain-mode`
  is a prompt over the same tools, plus a trace tool. `dropin-prose` and
  `dropin-plc` (built, above) add the drop-in tools; `dropin-photo`
  (built, above) shows photos to the model as image blocks; `dropin-pid`
  would add drawing drop-ins beside them.
