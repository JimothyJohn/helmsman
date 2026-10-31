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
- **PRs land one at a time.** dev's protection isn't strict: test-merge
  overlapping PRs on a throwaway branch and run the gate before both land.
- **Protocol claims** get a docs/SOURCES.md entry and a § / page citation.

## Next door

- `../agent-relay`: the relay (Rust, SAM, private, not deployed; local only
  until Nick runs its `scripts/create-github-repo.sh`, which creates
  JimothyJohn/agent-relay). Client-agnostic; the box↔relay contract is its
  `docs/relay-protocol.md`. Helmsman's agent profiles are
  `agents/helmsman/<name>/` (ids `helmsman/v3`, ...); the bare `v3` and
  `explain-v1` the box asks for are aliases.
- `.claude/handoff.md` (untracked): where the last session stopped.
