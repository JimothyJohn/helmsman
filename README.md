# Helmsman Factory Automation

A small soft PLC for factory edge boxes (a Raspberry Pi 5 is the reference):
Structured Text modules that an agent drafts and an engineer approves, run
against named signals on the devices the box masters, plus an open-source
CIP-over-Ethernet originator and adapter. `docs/ARCHITECTURE.md` is the map
of the code; `TODO.md` is the plan.

Status: early. Rust cargo workspace, crates under `crates/helmsman-*`:
`helmsman-proto` (CIP originator), `helmsman-adapter` (CIP target),
`helmsman-cli` (the `helmsman` command), `helmsman-iec` (ST compiler and
interpreter, PLCopen XML), `helmsman-io` (I/O map, drivers, Modbus TCP),
`helmsman-plc` (the runtime), `helmsman-diagram` (ST as a live diagram),
`helmsman-canvas` (the browser app: ST and its live diagram, docs/UI.md). `make help` lists the dev targets. See `docs/SOURCES.md` for the
reference stacks and ODVA documents this project draws on.

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

