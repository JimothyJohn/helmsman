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
  first argument, so those helpers build a bare `Command`. A box a test
  spawns (one that keeps running) gets `HELMSMAN_EXIT_WITH_STDIN=1` and a
  piped stdin its `Child` keeps, so it dies with a killed test; don't set
  it on `.output()` runs (that closes stdin at once).
- **Timing in tests.** No fixed sleeps before an assertion: wait with a
  deadline (`wait(...)` in tests/reload.rs). Watchers start once the state
  they watch holds. The suite must pass with 6 copies running at once.
  A box's reads (`/api/modules`, `/api/signals`, the values stream) are
  the live view the scan thread publishes every few ms, so they lag a
  control or a deploy by a tick: wait for the new state, never read it
  right after the reply (a stop test did and failed CI on 2026-11-07).
- **dev no longer requires branches to be up to date** (turned off
  2026-11-06): a green PR lands without re-running behind each merge; the
  push run on dev catches a clash. .github/workflows/update-branches.yml
  still updates auto-merge PRs when dev moves.
- **Protocol claims** get a docs/SOURCES.md entry and a § / page citation.
- **A device reply that will not decode: capture it before theorising.** A
  20 s `tcpdump` (sudo, so a hand-off script) settled the E1213's echoed
  socket-address item and showed the refusal the strict decode hid
  (2026-11-07, docs/devices/moxa-iologik-e1213.md). Hold the captured bytes
  as a regression test, and make the device's own answer readable.
- **A device that will not come back: look at the box before the
  network.** `sample <pid>` (no sudo) shows what each link thread is
  doing, and `scripts/enip-probe.py` asks the E1213 for its identity
  from the same launch context. On 2026-11-07 the link thread looped on
  a ListIdentity sweep whose send failed, and the probe run with
  Homebrew's python said `No route to host`: macOS Local Network
  privacy, not the firewall or the driver. Any box that talks to bench
  devices runs as a LaunchAgent (the try box's entry below).
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

## Checkouts and services (Nick, 2026-11-06)

- **The main checkout stays on `dev`.** No feature work in
  /Users/nick/github/helmsman itself: every branch gets a worktree.
- **Worktrees live under `.claude/worktrees/` only** (the Agent tool's and
  hand-made ones alike: `git worktree add .claude/worktrees/<topic> -b
  <branch> origin/dev`). No `../helmsman-*` siblings, except the service
  checkouts below.
- **One branch, one worktree.** Never `worktree add -f` or
  `--ignore-other-worktrees`: a branch checked out twice moves under the
  other copy, which then shows every file of the newer commit as a change
  (three such trees, 100+ "changed" files each, on 2026-11-06).
- **`scripts/prune-branches.sh` removes merged worktrees** (clean, HEAD in
  origin/dev or origin/master) before it deletes branches; a locked
  worktree is kept. Remove yours when its PR merges anyway.
- **Services run from a locked, dedicated checkout, never from an agent's
  worktree.** Today there is one:
  - Try box, `../helmsman-try` (locked), https :8081, state
    `scratch/try-state` and the relay token in that checkout's scratch/
    (sign-in in .claude/handoff.md). Rebuild at the dev tip and restart
    with one command: `~/github/helmsman-try/scratch/16-rebuild.sh`
    (untracked, in that checkout). From an `EnterWorktree` session that
    is a hand-off: its paths contain "github", which the isolation check
    reads as git. It runs as the LaunchAgent `io.advin.helmsman-try`
    (2026-11-07, `scratch/run-box.sh` in that checkout) in Nick's desktop
    session, because macOS Local Network privacy refuses LAN traffic to a
    binary started over SSH or from an agent session ("No route to
    host"; the box comes up but loses its devices). Restart:
    `launchctl kickstart -k gui/501/io.advin.helmsman-try` (works from
    SSH). A plain `kill` respawns it; stop for good with `launchctl
    bootout gui/501/io.advin.helmsman-try`. Never start a box that needs
    the bench devices with `nohup` from a shell.
  - A new service (board, mock-up server) gets its own entry here first.
  - Anything else running is a leftover: `scripts/dev-processes.sh` lists
    every Helmsman process with its port and checkout, `--stop` stops all
    but a locked service checkout. Run it before you stop a session.
- **`wip/recovered-*` snapshots expire.** One older than 7 days gets a
  board card: keep (merge it somewhere) or drop.
- **The soft PLC runs on the Studio, not the Pi** (Nick, 2026-11-07).
  Every box you start, test against hardware or demo runs on this Mac
  Studio (the try box is the standing one). The bench Pi
  (`nick@192.168.3.20`, `codesys-pi`) is out of the loop, its
  `helmsman-plc` service stopped and disabled, so no two platforms claim
  the same devices. Don't deploy to it, start it or point a device at it
  unless Nick asks for the Pi by name in that session; the Pi cards on
  the board are parked until then. (SSH to it needs Nick's key in the
  agent: `ssh-add --apple-use-keychain ~/.ssh/id_ed25519`.)
- **One EtherNet/IP owner per E1213.** The unit (192.168.3.60) takes one
  exclusive-owner connection; a second box's `Forward_Open` is refused,
  0x0106 ownership conflict or 0x0100 connection in use. With the Pi
  out, the owner is a Studio box; a second Studio box reads the unit
  over Modbus TCP.

## Next door

- `../agent-relay`: the relay (Rust, SAM, private repo JimothyJohn/agent-relay,
  deployed: serves helmsman/v7). Client-agnostic; the box↔relay contract is its
  `docs/relay-protocol.md`. Helmsman's agent profiles are
  `agents/helmsman/<name>/` (ids `helmsman/v6`, ...); the bare `v6` and
  `explain-v2` the box asks for are aliases. A served profile never
  changes: a prompt or tool change is a new id.
- `.claude/handoff.md` (untracked): where the last session stopped.
