<a href="https://helmsman.advin.io/"><img src="docs/brand/helmsman-banner.svg" alt="Helmsman Factory Automation" width="100%"></a>

**[Try it in your browser →](https://helmsman.advin.io/playground/)** A simulated controller and its live diagram, no install.

# Helmsman Factory Automation

The soft PLC you program by describing the machine. The agent writes
standard IEC 61131-3 Structured Text from your spec, you check it on a live
diagram, and an engineer approves it before it runs. **Early release.**

- **Robust:** a minimal set of dependencies; every change passes more than
  1,300 automated tests and interoperability tests in CI.
- **Simple:** one controller, one service, its own web screen. No cloud
  needed to run the machine, no database server, no microservices.
- **Safe:** a memory-safe language, engineer approval for every change, an
  audit trail, and safeguards that stay out of your way.
- **Reliable:** compiled, native performance on a fixed scan; a watchdog
  bounds every scan, and a faulted program stops alone with outputs safe.

A pure soft PLC: field I/O lives in junction boxes and remote I/O modules
(such as the Moxa ioLogik) on industrial Ethernet. It speaks EtherNet/IP
(CIP) and Modbus TCP, and imports PLCopen XML.

**Try it in your browser:** <https://helmsman.advin.io/playground/>
writes and runs Structured Text against a simulated controller, with its
live diagram; no install.

## For developers

`docs/ARCHITECTURE.md` is the map of the code, `docs/UI.md` the shape of the
app, `TODO.md` the plan. A Rust cargo workspace, crates under
`crates/helmsman-*`: `helmsman-proto` (CIP originator), `helmsman-adapter`
(CIP target), `helmsman-cli` (the `helmsman` command), `helmsman-iec` (ST
compiler and interpreter, PLCopen XML), `helmsman-io` (I/O map, drivers,
Modbus TCP), `helmsman-plc` (the runtime), `helmsman-diagram` (ST as a live
diagram), `helmsman-canvas` (the browser app). `make help` lists the dev
targets. `docs/SOURCES.md` reviews the reference stacks and ODVA documents
this project draws on.

## Command line

```
cargo build --release
target/release/helmsman discover                     # ListIdentity broadcast
target/release/helmsman identity 10.0.0.50           # Identity attributes 1-7
target/release/helmsman get 10.0.0.50 0xF5.1.5       # any attribute, as hex
target/release/helmsman io 10.0.0.50 --config 151 --out 150 --in 100 \
    --out-len 32 --in-len 32 --rpi 50 --set 0102...     # Class 1 exclusive owner
```

`helmsman help` lists every flag. `io` binds UDP 2222 locally because
targets send input there.

