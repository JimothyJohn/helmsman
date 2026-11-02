# Helmsman (this repo)

Rust cargo workspace (crates/helmsman-*). The map of the code is
docs/ARCHITECTURE.md; the plan and the open questions are TODO.md (the /todo
board). Branch flow and everything general is in ~/.claude/CLAUDE.md.

## Gate (before every push, each command on its own, unpiped)

- `./scripts/lint.sh`: fmt, clippy pedantic with warnings denied, helmsman-plc
  without default features, and helmsman-canvas for wasm32. CI runs only fmt
  and clippy of these, so the last two are yours to keep green.
- `cargo test --workspace --locked`

## Rules that cost a day to learn

- **`box` is a feature boundary.** helmsman-plc's store, API, policy, bundles,
  site discovery and OPC UA sit behind its default `box` feature (WASM behind
  `wasm` and sink TLS behind `https`, both on with the defaults), and
  helmsman-plc must still build without them (`lint.sh` checks it). New
  code that touches the network, SQLite, TLS or the OS goes behind `box`
  (and its test targets get `required-features = ["box"]`). The canvas
  ships as wasm32 on helmsman-iec and helmsman-diagram alone; helmsman-plc
  is only its native dev-dependency (the real box API in its tests).
  crates/helmsman-canvas/tests/wasm_deps.rs fails if a native-only crate
  reaches the canvas.
- **Test helpers that start the binary** (`plc()` in tests/bin.rs and its
  copies) pass `--opcua off`; a subcommand (`account`, `bundle`) must be the
  first argument, so those helpers build a bare `Command`.
- **Timing in tests.** No fixed sleeps before an assertion: wait with a
  deadline (`wait(...)` in tests/reload.rs). Watchers start once the state
  they watch holds. The suite must pass with 6 copies running at once.
- **PRs land one at a time.** dev requires branches to be up to date, and
  .github/workflows/update-branches.yml updates auto-merge PRs when dev
  moves (drafts stay behind until they leave draft).
- **Protocol claims** get a docs/SOURCES.md entry and a § / page citation.
- **A resizable egui panel grows to any row that overflows it, every
  frame.** A label in a plain `horizontal`, a combo box or field at full
  width beside a scroll bar, a grid cell with a full-width button: each
  widened the side panel by a third on 2026-11-01. Wrap rows
  (`horizontal_wrapped`), cap widths to the column, leave 12 pt for the
  bar, and keep `every_tab_leaves_the_panels_their_width` (ui_tests.rs)
  green: it visits every tab and asserts the panels stayed put.

## Voice (anything a user reads: the site, README intro, UI text, release notes)

The reader is a controls or plant engineer, not a developer. Benefits, not
features: **robust** (few dependencies, CI), **simple** (one controller, no
microservices), **safe** (memory-safe, approval and audit, friction hidden
to keep the user in flow), **reliable** (native performance, fixed scan,
watchdog). The headline is agentic programming: you write specs and
validate results, you don't write code. Proven IEC 61131-3 Structured Text,
no ladder and proud of it. EtherNet/IP (CIP) and Modbus TCP, Ethernet only;
a pure soft PLC with I/O in remote modules (e.g. Moxa ioLogik). Say "early
release". Never mention Rust, crates, the Raspberry Pi or the model vendor
by name. Claims must be true of what is built. Write like an engineer, with
no AI tells: no "you don't X, you Y", "X, not Y" contrasts used as flourish,
staccato fragments ("On purpose."), triplets or semicolon-chained slogans.

## Next door

- `../agent-relay`: the relay (Rust, SAM, private, not deployed; local only
  until Nick runs its `scripts/create-github-repo.sh`, which creates
  JimothyJohn/agent-relay). Client-agnostic; the box↔relay contract is its
  `docs/relay-protocol.md`. Helmsman's agent profiles are
  `agents/helmsman/<name>/` (ids `helmsman/v3`, ...); the bare `v3` and
  `explain-v1` the box asks for are aliases.
- `.claude/handoff.md` (untracked): where the last session stopped.
