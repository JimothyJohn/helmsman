# Devices, not protocols: the overnight plan

A brief a session can run from cold. Start it with: "Run PROTOCOLS.md".
Tick each step here in the PR that lands it; delete this file in the PR
that lands the last one (the card `device-profiles` in TODO.md stays as
the record).

## The goal

Nick, 2026-11-11: "the comms protocol isn't critically important since we
can talk to everything. It's most important for the user to easily find,
connect, and talk to the I/O on devices." A simpler, more limited
interface; power users are not the audience.

A user finds a device, clicks it, names it and uses its channels by
name. Nothing asks them for a protocol, an address, a byte order, a cycle
time or a vendor file.

## Decided (interview 2026-11-11; do not re-ask)

1. **Protocol: never shown, never chosen.** The box picks from the
   profile's order and falls back by itself. The protocol is named only
   in a failure message and in a device's details.
2. **Unknown device: the agent builds the profile** from the vendor's
   file or manual; the user checks the channel names and can publish it.
3. **Channels first** (named, typed, unit, safe value). A standard face
   per kind (a drive's enable, speed, position, fault) waits for the
   MDrive on the bench.
4. **Addresses and names: the box suggests, the user confirms.**
5. **Today's add-a-device form and typed points stay**, behind an
   Advanced switch.
6. **Timing is fully automatic**: from the program's scan period and the
   profile's limits. The user sees healthy, slow or lost.
7. **A profile is copied into the project** when the device is added.
   Library updates are offered, never applied.
8. **The Moxa E1213 proves it first.**

## What exists (read these before writing anything)

- `crates/helmsman-io/src/profile.rs` and `tests/profile.rs` (#351, in dev): the
  profile format (`helmsman-device 1`), its checks, and
  `Profile::map(transport, id, endpoint)`, which writes the I/O map for
  one transport with a signal per channel.
  `docs/devices/moxa-iologik-e1213.device` is the first profile.
- `crates/helmsman-io/src/driver.rs`: how a driver is named, opened and
  its points parsed (`parse_point_on`, `open_on`, `output_keys`).
- `crates/helmsman-plc/src/driver_dir.rs`: how `eds=NAME.eds` names a
  project file and is resolved to a path when the map loads. A profile
  in a project follows this.
- `crates/helmsman-plc/src/api/discover.rs` and
  `crates/helmsman-io/src/resolve.rs`: Scan today. EtherNet/IP
  `ListIdentity` always, Modbus identification with `?modbus=1`, the
  library lookup by identity.
- `crates/helmsman-project/src/devlib.rs`: the library index
  (`~/github/helmsman-devices`, `device.toml` per device, points as
  prose).
- `crates/helmsman-canvas/src/devices.rs`, `device_form.rs`,
  `device_form_ui.rs`: the Devices tab, "On the network", the add form.
- `crates/helmsman-plc/src/link.rs`: a link's poll period (`poll_ms`),
  link and point errors, what "down" means.
- docs/ARCHITECTURE.md (`profile`, `enip`, `profinet`, "Signals in the
  scan"), docs/UI.md (the Devices tab paragraph), CLAUDE.md (the gate,
  the panel-width rules, Voice for anything a user reads).

## The steps, one PR each, in this order

Each PR: a branch and worktree off `origin/dev`, the gate green
(`./scripts/lint.sh`, `cargo test --workspace --locked`, each on its
own), auto-merge on. Merge `origin/dev` into the next branch before
starting it. Steps 1 and 2 touch different files and may run as two
workers at once; 3 needs both; 4 and 5 follow 3.

### [x] 1. A map binds a channel; the box picks the protocol

- A device line names a profile, not a driver:
  `device rio device:moxa-e1213 mac=00-90-e8-7e-af-be`. Signals bind
  channels: `in part_present bool rio ch:DI-04`.
- `moxa-e1213` is `moxa-e1213.device` in the project (a new project file
  kind, resolved as `eds=NAME.eds` is; refused if it does not pass
  `Profile::parse`). No project (a bare box): the box's own folder, as
  WASM drivers do.
- A `ProfileDriver` in helmsman-io wraps the real drivers: it opens the
  profile's transports in order, uses the first whose link comes up, and
  moves to the next on a link error at open (the E1213's EtherNet/IP
  connection held by another owner must end on Modbus). Once one works
  it stays; after a later link error it starts again from the first. It
  translates `ch:NAME` to that transport's point. A channel error stays
  a point error.
- The transport in use is reported (the driver's `address()` neighbour,
  `/api/devices` gains `transport`), for details and failure text only.
- `output_keys` and the one-writer rule work on channels.
- Tests: the same ST program and map run against the simulated E1213
  over EtherNet/IP, then with EtherNet/IP refused (a second owner holding
  the simulator's connection, or a fault added to it for this) over Modbus, with no
  change to the map; a profile that is missing or malformed is a map
  error naming the file; a channel the profile does not have is a point
  error at load. The six-copies rule applies.
- Docs: ARCHITECTURE `profile`, the map format in `text`.

### [x] 2. One Scan, every protocol

- `POST /api/devices/discover` runs EtherNet/IP and Modbus
  identification by default (drop the `?modbus=1` switch, keep the
  bounds), and PROFINET DCP Identify when the build has raw Ethernet
  (`profinet-io`) and an interface is known; without it the answer says
  PROFINET was not looked for, in words.
- The answer is one list of **devices**, not three of protocols: entries
  that share a MAC are one device, with every identity it answered and
  the library's match. The old fields stay for one release so the canvas
  in the field keeps working.
- Tests: the simulated E1213 answering on both protocols is one entry;
  two units are two; the bounds (hosts, concurrency, time) still hold.
- PROFINET on a real wire is NOT part of this (see "Do not").

### [ ] 3. Add by click

- "On the network" lists devices by vendor and model. A row the library
  knows (it carries a profile) offers **Add**; the form asks for one
  thing, the device's name, and shows the channels it will add. Confirm
  writes the profile into the project, the `device ... device:NAME` line
  with the identity the scan saw, and a `ch:` signal per channel named
  `<device>_<channel>` (`profile::signal_name`).
- A row the library does not know says so and offers the agent (step 5)
  and Advanced.
- **Advanced** is today's form and typed points, unchanged, behind a
  switch that is off by default and remembered.
- The device's details (a tooltip or a fold) say the protocol in use and
  each channel's point, read-only.
- The library: `index.json` gains `profile` (a file name beside
  `device.toml`); add `moxa-iologik-e1213.device` to
  `~/github/helmsman-devices` as a PR into its `dev`; `devlib` reads it.
- Tests: `ui_tests.rs` at the panel's default width
  (`every_tab_leaves_the_panels_their_width` stays green), the form's
  writes as a project diff, `make canvas-web` then `make canvas-smoke`
  with a look at the screenshots. UI text follows CLAUDE.md "Voice".
- Docs: docs/UI.md's Devices paragraph, the help page if it shows the
  form.

### [ ] 4. Timing by itself; healthy, slow, lost

- A profile transport may give limits (`cycle_ms=100..3000`). With no
  `poll_ms` on the line, a link polls at the fastest period of the
  modules that read or drive its signals, clamped to the transport's
  limits; cyclic transports (`rpi_ms`, PROFINET `cycle_ms`) get the same
  number.
- Status per device from what the link measures: **healthy**, **slow**
  (its measured update period is more than twice what was asked, over
  the last N cycles), **lost** (the link is down, with the sentence the
  driver gave). `/api/devices` carries it and the measured period; the
  Devices tab and the Monitor show the word, the number only in details.
- A typed `poll_ms` (Advanced) still wins.
- Tests: scripted clocks, no sleeps before assertions.

### [ ] 5. The agent builds a profile

- A device the library does not know: the user drops the vendor's file
  (EDS today; GSDML through `scripts/gsdml-slots.py`'s logic moved into
  Rust if time allows, else EDS only and say so) and the agent proposes
  a profile. It is accepted only through `Profile::parse`; the user sees
  the channel list and renames before it is written.
- Publish: the existing library-PR path from the relay
  (`devices-repo`), now carrying the profile.
- If this step needs a relay profile change (a new id in
  `../agent-relay`), stop at the box side and put the relay half on a
  card: a served profile never changes.

## Not tonight

- Step (f) of the card, suggested addressing, and (g), kinds: (f) needs
  a decision on the address pool per site, (g) needs the MDrive.
- The node: its I/O map (helmsman-nodecore `iomap`) keeps addresses for
  now. Say so on the card.

## Do not

- Do not debug PROFINET on the wire, flash the P4, or touch branch
  `feat/profinet-wire` (parked: the Studio's raw socket received
  nothing; suspected, not confirmed, that it is neither promiscuous nor
  in the DCP multicast group).
- Do not put `profinet-io` in the default build.
- Do not remove or restyle the existing form: it becomes Advanced.
- Do not start the try box or point anything at the bench E1213's
  EtherNet/IP connection: the node (press-5) reads it over Modbus and
  everything here runs on the simulators.
- Do not merge `dev` into `master`, in either repository.
- Do not stop for a question. Take the option that keeps the user's
  screen simpler, write the question on the board (TODO.md, `/todo`
  format) with what was assumed, and carry on.

## When it is done or stuck

- Rewrite `.claude/handoff.md` at each merged PR: what landed, each
  worker's worktree, the next step.
- Stuck after two attempts on one step: write what was tried and what
  happened into the handoff and the card, skip to the next step that
  does not depend on it.
- At the end: remove merged worktrees, list what is in `dev` and what is
  not, and send one push notification.
