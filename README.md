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
`crates/helmsman-*`: `helmsman-proto` (CIP originator: discovery, explicit
messaging, the Class 1 scanner), `helmsman-adapter` (CIP target),
`helmsman-iec` (ST compiler and interpreter, PLCopen XML), `helmsman-io`
(I/O map, drivers, Modbus TCP), `helmsman-project` (a project's files and
archive), `helmsman-plc` (the runtime), `helmsman-diagram` (ST as a live
diagram), `helmsman-agent` (the chat panel's agent loop), `helmsman-canvas`
(the browser app). `make help` lists the dev targets. `docs/SOURCES.md`
reviews the reference stacks and ODVA documents this project draws on.

