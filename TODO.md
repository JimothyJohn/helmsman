# TODO

Done cards were removed on 2026-11-06 (git history has them). Three sections up top: decisions only you can make, things only your hands can do, and what an agent builds next. The roadmap below is the long view.

## Open questions for Nick

## Needs your hands

- [ ] **Promote the guided tour** — #248 (the playground tour) is in dev but landed one minute after your dev → master merge, so the live playground does not have it, and docs/playground on dev is still the build without it. An agent rebuilds docs/playground on a PR into dev; then the next dev → master merge (yours) puts /playground/?tour live with the help links that point to it.
  needs: the playground rebuild PR (build/playground-tour) merged into dev, then your dev → master merge
  result: helmsman.advin.io/playground/?tour runs the tour
  est: 15m
  difficulty: low
  id: tour-live
  asked: 2026-11-06
  by: claude-session

- [ ] **Review the Pi firewall fix (#245)** — draft PR fix/pi-scan-replies: the Devices scan on the Pi hears the answers through its firewall. Updated with dev 2026-11-06 (its TODO.md conflict with #249 resolved: the e1213-address card it annotated is done and gone); leave draft or flip it to ready (auto-merge then lands it).
  needs: your eyes on the PR
  result: the Pi's Devices scan finds devices
  est: 10m
  difficulty: low
  id: pr-245
  asked: 2026-11-06
  by: claude-session

- [ ] **A public device library: map a device once, for everyone** — interview 2026-11-02: a public repository (JimothyJohn/helmsman-devices) with one folder per device: the EDS or the Modbus map, the `enip-io` profile or map lines, a doc with the manual's citations, the identity (vendor, product code, model text). The box and the agent look a device up before generating anything: by identity first (ListIdentity's vendor and product code, a Modbus unit's identification), then by vendor/model text; semantic search via the relay only once there are enough entries to need it. An import in the Devices tab offers to publish the result (a PR to the repo from the relay, the engineer confirming).
  needs: Nick creates the public repo (a scratch script); the licence line per vendor file (the Moxa EDS carries no terms: a pointer and a hash, not the file, until Moxa answers)
  result: the second user of an E1213 gets its points from the library in one click; the agent's mapping work is never repeated
  est: 2d
  difficulty: med
  id: devices-repo
  asked: 2026-11-02
  by: claude-session
  notes: 2026-11-06: helmsman-devices PR #1 (index.json) is merged into its dev; main has no index.json yet. Left: dev → main there (yours), then tick.

- [ ] **Look at the box UI on the Pi, with projects** — the bench Pi (nick@192.168.3.20, on the Studio link) runs helmsman-plc with the relay on agent v7 (a real turn 2026-11-04). Its API and canvas do not answer on the LAN (8080 and 443 closed from the Studio, 2026-11-06), so reach it through SSH. Whether it already runs `--projects` and has an engineer account is unchecked (the agent's SSH has no key loaded).
  needs: your key in the agent (`ssh-add --apple-use-keychain ~/.ssh/id_ed25519`)
  result: you sign in on the Pi as an engineer and edit, deploy and force a project
  notes: connect from the Studio: `ssh -L 8080:127.0.0.1:8080 nick@192.168.3.20`, then http://127.0.0.1:8080/ ; from the Air: `ssh -J nick@10.0.0.41 -L 8080:127.0.0.1:8080 nick@192.168.3.20`, same URL. Not on projects yet: `scripts/pi-deploy-plc.sh nick@192.168.3.20` (projects is its default; RESTARTS helmsman-plc; keep PLC_API as the unit has it), then `scripts/pi-account.sh nick@192.168.3.20 nick engineer`.
  est: 15m
  difficulty: low
  id: pi-ui-demo
  asked: 2026-10-31
  by: claude-session

- [ ] **Check the board backend on the Pi** — run the GPIO / I2C check against the pick PC: finds the header's GPIO chip (gpiochip0 or gpiochip4 on a Pi 5, by kernel), lists /dev/i2c-1, builds and runs `board_check` (helmsman-io's Linux backend, the code behind `helmsman-plc --gpio-chip`) reading `di:4` by default. Add `READ="di:4 i2c:1:0x48:0:i16"` for a sensor, and `LOOP="do:17=di:27"` only with a jumper from that output to that input (it drives the output).
  needs: on the Pi `sudo apt install gpiod i2c-tools`, `dtparam=i2c_arm=on` and a reboot, the SSH user in the gpio and i2c groups; Docker here
  result: the board backend proven on real GPIO (and I2C if a sensor is wired); then `--gpio-chip` goes into the deploy unit
  notes: `! scripts/pi-gpio-check.sh nick@192.168.3.20` (runs here, not in the native terminal; read-only unless LOOP is set)
  est: 30m
  difficulty: low
  id: pi-gpio-check

- [ ] **Supply the datasheet image** — docs/helmsman-datasheet.pdf (docs/datasheet.html) has no picture yet. Nick 2026-11-01: card it, place it when the file exists. Drop the file under docs/brand/ (PNG or SVG; a photo at 1200 px wide or more, or a render) and say where it goes: in the top band right of the wordmark (about 45 mm wide, the lede moves down) or as a strip under the lede. The build (`make datasheet`) refuses a second page, so the slot comes out of the spec table's air.
  needs: the image file and its spot
  result: the datasheet carries the image; rendered, one page, re-committed
  est: 30m
  difficulty: low
  id: datasheet-image
  asked: 2026-11-01
  by: claude-session

- [ ] **Check the L5X reader against RM014 and a real export** — `helmsman_iec::l5x` reads the export shape from the `l5x` Python package and memory (docs/SOURCES.md); run it on a Studio 5000 export of a bench project (controller, program, routine and rung exports) and the manual, and fix what differs.
  needs: the RM014 PDF (403 from the agent) and a Logix Designer export
  result: the inventory is right for real customer files
  est: 3h
  difficulty: low
  id: l5x-real-export
  notes: source (l5x-source, 2026-10-31): Nick exports one from his bench project. Later 2026-10-31: RM014 is read and cited; the ladder converter (l5x-import) also needs the real export: run `POST .../import?file=X.L5X` on it and check the refusal list (or the ST) against the project, the neutral text of each instruction it uses (JSR's `JSR(Name,0)` form, `?` pseudo-operands, v36 mnemonics such as EQ/MOVE), decorated data of TIMER/COUNTER tags, and rung Type attributes other than N. The fixtures (line3.L5X, ladder.L5X) are hand-written, not exports.

- [ ] **Remove the dead tcpethernet worktrees** — `git worktree list` still names /Users/nick/github/tcpethernet-pi-demo (a checkout from before the rename whose .git file points at the old /Users/nick/github/tcpethernet path, so git cannot read it; it still holds HOWTO.md and a crates/ tree) and tcpethernet/.claude/worktrees/retro-20261030 (directory gone). Look at pi-demo for anything worth keeping, then remove both.
  needs: a look at the pi-demo directory
  result: `git worktree list` shows the main checkout and live worktrees only
  notes: `ls /Users/nick/github/tcpethernet-pi-demo` then `git worktree remove --force /Users/nick/github/tcpethernet-pi-demo` and `git worktree prune`
  est: 5m
  difficulty: low
  id: dead-worktrees
  asked: 2026-11-01
  by: claude-session

- [ ] **Try the node canvas** — ComfyUI/Node-RED style editor in Rust (egui, wasm): nested function blocks, live scan sim, not wired to the PLC. Kept as a lab tool (2026-10-29); per canvas-edit-scope the operator canvas is the read-only ST view (st-graph-view), which reuses tcpethernet-graph and this renderer. Serve it: `make canvas-web`, then `python3 -m http.server 8790 --bind 0.0.0.0 --directory target/canvas-dist` and open http://10.0.0.41:8790/ from the Air (stop: `kill $(lsof -tiTCP:8790 -sTCP:LISTEN)`). In it: hold a start button, double-click a motor to go inside, select nodes + Cmd+G to make a block, `?` lists the controls.
  needs: your eyes on the UI
  result: a list of changes, or none
  notes: `make canvas-serve` (trunk, live reload) only answers on localhost on the Studio: macOS's firewall holds the unsigned trunk binary's incoming connections behind a GUI prompt; python3 is allowed
  est: 15m
  difficulty: low
  id: canvas-try

## Next to build

- [ ] **Run, stop and restart a box's program from the Monitor** — Nick 2026-11-01: the Monitor needs play, start, stop. The simulation's transport (run or stop, step, restart) is on the Monitor now; a box has no such endpoint: a module runs from deploy until the next one. This adds `POST /api/modules/{name}/run|stop|restart` behind an engineer session and the approval policy, audited like a force, with HELD shown while stopped (outputs at their safe values), and the Monitor's buttons use it when signed in.
  needs: the runtime's held state exposed per module
  result: the same three buttons on a box's Monitor, audited; the module's state shows HELD while stopped
  est: 2d
  difficulty: med
  id: monitor-box-run-stop
  asked: 2026-11-01
  by: claude-session
  notes: Nick 2026-11-06: yes, an engineer may stop and restart a program from the Monitor.
  status: ready

- [ ] **The Agent on a box** — left out of agent-scoped-edits (#207): a box's canvas has no Agent tab (BoxView::tabs() is I/O and Requirements; a ui_test pins it), its page's CSP is connect-src 'self', and the box's agent runs through the relay as proposals (POST /api/chat). Give the box an Agent tab that talks to /api/chat, with the pages (#187), iterations (#201), the requirement gate (red_tests) and the edit scope (`scope::apply` takes plain files, so it is reusable) on the box side.
  needs: the relay deployed (relay-deploy) to try it against a real model; the rest builds against the relay fixtures
  result: the same agent workflow on a box as in the playground, every change still a proposal through the approval policy
  est: 3d
  difficulty: high
  id: agent-on-box
  asked: 2026-11-03
  by: claude-session
  notes: first cut in #217 (Agent tab over POST /api/chat, streamed, proposals); the box asks for v7 (#218), which the relay serves. Left: pages and iterations on the box, scope::apply box side.

- [ ] **Box client for relay protocol v2 (helmsman/v8)** — the relay now keeps the conversation (agent-relay PR #7; docs/relay-protocol-v2.md and docs/harness.md there): the box opens a session (`POST /v2/sessions {"agent":"helmsman/v8"}`), sends only new input per step (`user`, `tool_results`, `continue`), runs the client tool calls a step's `harness_yield` names with the existing executor and `<box-data>` envelope, and renders text from the forwarded stream. The relay compacts the context (EoCC), runs doc_grep/doc_read/history_* and fetch_document itself (a fetched EDS arrives as `harness_document`; GET /v2/docs/{id}/raw saves it to the project), so the box drops its 900 KiB history cap and its per-turn model budgets. Keep v7 on v1 as the fallback until v8 has run on the bench; the offline fallback and relay-health watcher stay. Vendor docs/relay-protocol-v2.md beside the v1 file.
  needs: agent-relay PR #7 merged and deployed (relay cards relay-eval-live, relay-deploy-v2)
  result: `--relay-agent v8` runs chat on relay-held sessions; long manuals and long sessions stop hitting body and history limits
  est: 2d
  difficulty: med
  depends: agent-on-box
  id: relay-v2-client
  asked: 2026-11-05
  by: claude-session
  notes: 2026-11-06: agent-relay PR #7 is still open.

- [ ] **Modbus identification probe (FC 43/14) for discovery and identity** — left out by the device-by-identity and discovery workers (2026-11-02 night): `cip=` identities and the ListIdentity sweep cover EtherNet/IP; a Modbus-only device is found today only by `mac=` once the host has seen it. Add Read Device Identification (FC 43, MEI type 14: VendorName, ProductCode, MajorMinorRevision) to helmsman-io's modbus driver, a bounded probe of the ARP table's hosts on 502 in `POST /api/devices/discover` when asked (`?modbus=1`), a `modbus=VENDOR:PRODUCT` identity endpoint resolved by that probe, and the library lookup by it (`identity.modbus` in device.toml exists). Cite the Modbus Application Protocol spec section in docs/SOURCES.md.
  needs: a Modbus device that answers FC 43/14 to test on (does the E1213? check on the bench; the simulator answers it either way)
  result: a Modbus-only block is found and matched like an EtherNet/IP one
  est: 1d
  difficulty: med
  id: modbus-identification
  asked: 2026-11-02
  by: claude-session
  notes: box side merged in #219 (FC 43/14 codec and probe, modbus= endpoints, discover ?modbus=1, spec §6.21). Left: the Devices tab asking ?modbus=1 and showing the list; identity.modbus in helmsman-devices build-index.py; a Modbus-identity query on GET /api/devices/library; whether the E1213 answers FC 43/14 (unchecked: it was off the LAN 2026-11-04).

- [ ] **Bump ruSTy past 1.0.7 and drop the unsigned-compare workaround** — PLC-lang/rusty#2015 fixes signed compares of UDINT/ULINT (and unsigned division, CASE, negation) on master; no release carries it yet. When one does: new version + SHA-256 digests in docker/rusty/Dockerfile, flip `rusty_unsigned_compare_bug` to assert agreement, drop the UDINT/ULINT dodge in `cond` (tests/oracle.rs), widen the sweep once.
  needs: a ruSTy release after 2026-10-21 (`gh release list -R PLC-lang/rusty`)
  result: oracle compares unsigned conditions directly
  est: 1h
  difficulty: low
  id: rusty-bump

## Product vision (interview 2026-10-29)

Working title until naming lands. A small, opinionated controller for factory edge boxes (Pi 5 reference, headless, browser UI), for non-mission-critical work: low-frequency detection, shift-aware scheduling of its own outputs, on-floor signalling (stack light, andon, screen). Data is pushed elsewhere (webhooks, OPC UA); the box keeps no historian.

Agentic first. The agent (Claude, behind our relay) is the primary programmer. Every module is Structured Text the agent writes and an engineer can tweak; the existing parser, checker, interpreter and ruSTy oracle are its feedback loop. The canvas is the operator's window into the code: ST rendered as a live diagram with values on wires and the reason each output is on. The canvas is the app shell; chat lives in its side panel.

Extension tiers: data (params, signals, schedules) → ST modules → WASM (drivers, heavy code; M3). Users are layered: operator (view, ask), engineer (approve, edit ST, PIN), integrator (modules, policy). The approval policy depends on tier and role. No user theming: the look is fixed. Param changes go live with undo; logic changes get a preview; anything that can move an output needs an engineer PIN plus a sim replay.

Hard limits, enforced by the host and never by the prompt: no writes to external PLCs (CIP is read-only toward them); no output change without human approval; modules have no network access (core sinks publish); the agent never touches the Rust core. Offline: everything installed keeps running, chat degrades to rule-based param edits (no local model). Business: open core (runtime + canvas MIT; relay, fleet and verified marketplace paid) plus a preloaded box. Standalone first, fleet-shaped data model from day one.

Update 2026-10-29 (I/O mapping): "This device is the PLC, it just may talk to other slave/client devices. This will be a master/server." The box masters its devices and writes to them through the I/O map; read-only stays the rule for other controllers it only observes (a CompactLogix it reads tags from). Agent-written drivers run in the WASM tier with one extra capability: a socket to their own device's endpoint.

## Roadmap M1: bench demo, talk → module → light

Demo: on the bench, "flash the light when CompactLogix tag X is high for 5 s". The agent drafts ST, the canvas shows it live against real values, a PIN approves it, the Pi's output toggles, and undo restores the previous snapshot.

- [ ] **Box outputs: GPIO stack light** — Pi GPIO (or a relay hat) as box-owned outputs with named channels; blink patterns as a stdlib FB.
  needs: a relay hat or 24 V stack light + driver on the bench
  notes: the GPIO backend is in (feat/sources-modbus-rtu: `do:N` is line N of `--gpio-chip`); the blink FB is in (feat/blink-fb: `BLINK_PATTERN(EN, MODE, PERIOD) -> Q` in helmsman-iec's library.st, modes off / steady / slow / fast / double flash, on the scan clock; not named BLINK so modules called blink still compile); left: the hardware and pi-gpio-check on the Pi
  result: the demo's light
  est: 1d
  difficulty: low
  id: gpio-outputs
  notes: stack light (stack-light-hw, 2026-10-31): a 24 V light driven from the E1213's DO through the map, no new hardware.

- [ ] **M1 demo run** — the sentence at the top of this section, on the bench, recorded. scripts/m1-demo.sh brings the box up from a clean deploy.
  needs: CompactLogix on the bench
  result: the thing to show people
  est: 1d
  difficulty: med
  depends: gpio-outputs
  id: m1-demo
  notes: plan (m1-demo-plan, 2026-10-31): E1213-only demo first, the CompactLogix version after.

- [ ] **Tabletop I/O bench** — one Modbus TCP remote I/O block with 24 V DI/DO on the Studio link (192.168.3.x), outputs DO0-3 wired back to inputs DI0-3, a 24 V supply, and a Pi GPIO to 24 V output stage for do:N. Then `conformance --endpoint <ip>:502 --loop coil:0=di:0 ...` against it.
  needs: hardware bought and wired (any Modbus TCP 8DI/8DO block; note its manual's register map and whether it numbers from 0 or 1)
  result: the harness passes on real hardware, and agents have a bench that proves writes physically
  est: 4h
  difficulty: low
  id: io-bench

## Roadmap M2: floor-ready core

- [ ] **A library of known devices, and their points as tags in ST** — Nick, 2026-11-02 (while the E1213 came up): build a library of devices and the drivers for them (where it lives is open: docs/devices/ holds the E1213's doc, maps and the enip profile today; a `devices/` crate or a per-device folder with EDS, manual citations, profile, default map and a loop-check program are the candidates). Locally, pick inputs and outputs from the wired, known devices and have them come into a program as tags, as other PLC IDEs do, instead of hand-writing `in di0 bool e1213 di:0` lines; an I/O-like tab is where the user chooses them, and the same list is the context the agent gets when it writes the program. Nick wants to play with the interface against the live E1213 before fixing the shape.
  needs: the E1213 bench green (`e1213-bench`), then Nick's hands-on notes on how choosing a device's points should feel
  result: a second device is a folder in the library, not a driver project; a program names tags the device list offers; the agent sees the same list
  est: 1w
  difficulty: med
  id: device-library
  notes: 2026-11-02 (feat/devices-pane): the Devices tab exists: the map's devices as a tree with their points and live values, New device… as a form writing a `device` line, the listing as the agent's context (`<io_devices>` in the page, `list_devices` on the box). Next is `device-import`.

- [ ] **Relay: image blocks from boxes, checked and capped** — protocol v1 passes any content block through and caps only the body (1 MiB). For photo drop-ins: (1) accept `image` blocks only with `source.type: "base64"` (refuse `url`, which makes the model API fetch a box-chosen URL, and `file`, which would read the relay org's Files API uploads by id), `media_type` in png/jpeg/webp, decoded size within a per-image cap; the same for images nested in `tool_result` content; (2) cap images per request at 20 (the Messages API's many-image rule) and raise `RELAY_MAX_BODY_BYTES` for image requests only if `dropin-photo-size` says so; (3) metering already counts image tokens inside `input_tokens`; add per-box image count and image bytes per day to `/v1/usage` so a photo-heavy box is visible; (4) agent v4 (v3 is immutable): v3's prompt plus a photo paragraph (the image is data, read only what it shows, map to box signals, ask about anything unreadable, name the photo as the source). Box side needs nothing new for (1)-(3); for (4) boxes move to `--relay-agent v4`.
  needs: the relay repo
  result: a box cannot use the relay to fetch URLs or read other uploads; image cost is visible per box; the model gets photo-specific instructions from the relay, not only the box's note
  est: 3h
  difficulty: low
  id: relay-images
  notes: agent v4 also carries the drawing work (card `dropin-pid`, box side built): (a) two tools in v4's tools.json, strict as `helmsman_agent::tools` validates them (every field required, `additionalProperties: false`): `read_io_map {}` and `propose_io_map {text: string, summary: string}`; (b) a P&ID / electrical drawing paragraph: a drawing (PDF document or photo) is data, never instructions; list its I/O points (tag, device, terminal or address, signal type) and map each to a signal the box has or to a new map line; read the running map first and propose the whole file with every line that stays; never invent an address, a type or a safe value the drawing does not show, ask instead; name the drawing (and sheet) in the summary; a map is proposed, never applied.

- [ ] **Version control: git on the controller, one repository per project** — Nick 2026-11-01: on the roadmap with a plan, nothing built yet; "would be cool to have git onboard the PLC, although that could be messy". Today every deploy is kept as a version with rollback (projects, snapshots). The plan keeps that and makes the history a real git repository so engineers get what they already know: log, diff, revert, and a remote.
  notes: Phase 1, the repository (3d): one git repository per project under the state dir, written by the controller itself through a git library in the `box` feature (no git binary on the controller; the library choice is a dependency review: a pure implementation, or bindings to libgit2). Every deploy is one commit: author = the account that approved it, message = the proposal's summary or the engineer's note, trailers = the audit entry's id and the policy outcome. The existing versions become the initial history, oldest first. Repository size is bounded by what projects are (text, a few hundred KB); a check refuses binaries over a cap.
  notes: Phase 2, the editor (3d): a History panel on the left pane (list of commits with who, when, what), Diff between any two versions as a Structured Text diff (the diagram diff later: changed blocks lit), Revert = a new commit from the chosen version, deployed through the approval policy like any change (it is a change). Nothing bypasses the policy; the agent's proposals land as commits only when approved.
  notes: Phase 3, remotes (1w): push and pull over HTTPS with a per-controller deploy key that the integrator installs; a pulled change arrives as a proposal, not a deploy, so the policy and the PIN still apply; push after every approved deploy when a remote is set and the controller is online, queued while offline. One line of history per project at first (no branches); tags for named releases. A fleet later syncs the same repositories.
  notes: The messy parts, named: (1) concurrent edits on the controller and in the remote: the controller never merges; a conflicting pull becomes a proposal showing both sides and an engineer picks. (2) Secrets and tokens are never in a repository (the policy file, accounts and the relay token stay outside the project). (3) Flash wear and power loss: commits are written with the same fsync discipline as snapshots, and a half-written repository is rebuilt from the kept versions. (4) The agent can read history but never pushes or pulls. (5) Offline is the normal case; a remote is an option, not a requirement. (6) A remote that rewrites history is refused; the controller only fast-forwards.
  needs: a decision on the git library (dependency review), and a remote to test against (a private repository is fine)
  result: `git log` of a controller's project reads like its audit trail; diff and revert in the editor; optional push and pull
  est: 2w
  difficulty: high
  id: version-control
  asked: 2026-11-01
  by: claude-session

## Roadmap M3: tiers, sharing, fleet

- [ ] **Map the network from the electrical drawings and the bill of materials** — "where it goes": the drawings drop-in (dropin-pid) reads one sheet into points today; this reads a whole drawing set plus the BOM into the devices, their addresses and the wiring between them, and proposes the complete I/O map and the device list, cross-checked against what ListIdentity and the Modbus scan actually find. Differences become questions to the engineer.
  needs: dropin-pid; a drawing set and BOM from a real project to test on
  result: an I/O map and device list from the project's documents, with every difference from the live network listed
  est: 2w
  difficulty: high
  id: network-from-drawings
  asked: 2026-11-01
  by: claude-session

- [ ] **Sort out and explain the behaviours already present in a system** — "where it goes": from recorded trends, the trace tap and sim replay, the agent groups what the system does into named behaviours (a cycle, an interlock, a fault pattern), explains each in plain words with the signals that drive it, and flags the ones no program accounts for. Explain mode today works on one program; this works on the plant's recorded behaviour.
  needs: box-trend-source and sim-replay recordings from a running machine; explain-mode
  result: a behaviour list for a machine, each one explained and traceable to its signals
  est: 2w
  difficulty: high
  id: behaviour-map
  asked: 2026-11-01
  by: claude-session

- [ ] **A running template of the machine you already built** — "where it goes": the holistic one. From the device list, the I/O map, the drawings and the observed behaviours, the agent proposes a complete starting project (programs, plant model for the simulation, signal names, a screen) that runs in the simulation on day one, so the engineer edits a working machine instead of describing one from a blank page. The machine starters are the hand-written version of this today.
  needs: network-from-drawings, behaviour-map, the starters as the shape to generate
  result: a new project that runs in the simulation from the plant's own documents and recordings
  est: 3w
  difficulty: high
  depends: network-from-drawings, behaviour-map
  id: machine-template
  asked: 2026-11-01
  by: claude-session

- [ ] **Fleet console + OTA** — the relay grows into the fleet backbone: box registry, snapshot sync, signed updates of the core.
  est: 3w
  difficulty: high
  notes: 2026-11-02 night: the node half moved: helmsman-node runs the scan bench and Modbus (PRs #193, #199); its own API + site beacon and the Site view across nodes are what this card is now.
  id: fleet-console
  needs: a decision taken (interview 2026-11-02): no console host at all; see notes
  notes: Nick 2026-11-02 (board): "I like this because it also allows me to set it up like my ../github/supervictor project. And now I realize that I may not have enough cores to do what I need with the network, uplink, communication, and program / process that may get complicated." Interview 2026-11-02: the fleet lives on the ESP32-POE2 as "every node is its own box": each node runs the runtime, serves its own API and announces itself on the LAN (`--site`), the fleet view is the canvas's Site view across nodes from any laptop, history and uplink go node → relay. Nothing in the middle to host. This card becomes: the Site view across boxes and nodes, the relay as the uplink, OTA of the node firmware (signed, A/B slots) once the node runs; depends on the edge spike (`node-modbus`) for the node half.
  status: ready

- [ ] **Resume the edge node: faster scans, PSRAM, and the network receive path** — parked by Nick 2026-11-03 after the first runs on the boards (docs/EDGE.md Phase 1 and 2). Three open threads, in order: (1) the network: on the POE2 frames from the Studio never register (ARP replies unanswered, no UDP report sent) and the POE-ISO gets no DHCP lease; first flash supervictor's known-good POE-ISO firmware on the same board to tell this firmware from the board or switch, then compare the executor and interrupt setup (esp-rtos) and the main task's share of the CPU; (2) scan speed: 28 ms for the 4k-statement bench against < 5 ms; measure flash at 80 MHz QIO (it boots at 40 MHz DIO), the interpreter's i128 `Value::Int` on a 32-bit core, the no_std BTreeMap lookups, esp-alloc heap-stats, before any rewrite (a compact compiled form or compile-on-laptop is the bigger lever); (3) the PSRAM corruption: compiling the batch-mixer starter with PSRAM asks for 0x3F40D781 bytes (a rodata address read as a length).
  needs: Nick's call to resume (2026-11-06: stays parked); the POE-ISO (WROOM, /dev/cu.usbserial-1130 on 2026-11-03) for the network thread since USB can stay on with PoE; the POE2 for PSRAM
  result: a node that scans a realistic program inside its period and talks to the Studio, or a firm no with numbers for each thread
  notes: the Studio's en0 has 10.0.0.241 (scratch/04-studio-wired-10net.sh; `off` removes it, a reboot too); LOG_SINK=10.0.0.241:9514 puts the reports on the wired side. Builds: scripts/node-build.sh [net], NODE_BOARD=poe-iso; flash: scripts/node-flash.sh [bench|modbus], Nick's step.
  est: 1w
  difficulty: high
  id: node-resume
  asked: 2026-11-03
  by: claude-session
  status: blocked

- [ ] **Vision sources** — deferred per interview; card kept so the signal model leaves room for camera-derived signals.
  est: 2w
  difficulty: high
  id: vision

## Roadmap M4: marketplace and the box

- [ ] **Community marketplace** — publish/browse/install bundles; floor per market-floor, then an AI review on install that explains risk in plain words and can only refuse.
  est: 3w
  difficulty: high
  notes: walking skeleton done 2026-11-02 night, PR #196: bundle::index (signed canonical JSON), bundle::market fetch (public hosts only), CLI bundle index/fetch, POST /api/market/list and /api/market/install through the bundle import and approval policy, --market-allow-local. Card stays open for accounts, payments, the AI review.
  id: marketplace
  needs: the walking skeleton only, for now
  notes: Nick 2026-11-02 (board): do it now with my own judgement; interview: walking skeletons. Cut: a static, signed index of bundles (a JSON file and the bundles in a public repo, signed by trusted publishers), a box lists it and installs from it through the existing bundle path and approval policy; no accounts, no payments, no AI review yet. Est for the skeleton: 2d.
  status: ready

- [ ] **Box SKU** — reproducible Pi image, first-boot pairing with the relay, provisioning script, enclosure/BOM for the preloaded kit.
  est: 2w
  difficulty: high
  notes: walking skeleton done 2026-11-02 night, PR #198: scripts/pi-image.sh (pi-gen in Docker, pinned ref), helmsman-firstboot pairing files → LoadCredential, docs/BOX.md; not booted. Pairing by code needs a relay endpoint (relay card). Card stays open for enclosure/BOM and a booted image.
  id: box-sku
  needs: the walking skeleton only, for now
  notes: Nick 2026-11-02 (board): do it now with my own judgement; interview: walking skeletons. Cut: a reproducible Pi image build script (pi-gen or rpi-image-gen, the box binary and the canvas preinstalled, the systemd unit, first boot pairs with the relay from a one-time code) checked by booting it in QEMU in CI; no enclosure or BOM yet. Est for the skeleton: 2d.
  status: ready

- [ ] **Modbus TCP from the node** — an async twin of helmsman-io's `Transport` over embassy-net's TCP socket behind a feature, `driver::Error` without std::io::Error, DI read and DO write batched to two round trips per scan; 10 ms scan against the E1213 (or the simulator) with DO0-3 looped to DI0-3; comms-loss safe state on yank.
  needs: flash on USB with Ethernet unplugged, THEN unplug USB, THEN PoE; the E1213's comms-loss safe state configured; a UDP log sink on the Studio
  notes: Nick 2026-11-02 (board): do it now with my own judgement. After `node-scan-bench`; then the node's own API and site beacon (`fleet-console`'s node half).
  result: G4 (overruns < 1 % of scans over 10 min) and G5 (the block goes safe within its timeout on Ethernet loss)
  notes: docs/EDGE.md §Sync scan, async network, Phase 2
  est: 1d
  difficulty: med
  notes: firmware done 2026-11-02 night, PRs #197 (helmsman-modbus no_std codec; helmsman-io frames through it) and #199: 10 ms scan, FC2 read + FC15 write, chase program, UDP report.
  notes: run 2026-11-03, then PARKED by Nick: the Modbus loop works on the POE2 (lease, chase program on the E1213's DO-00..02); the UDP reports never arrive (the node never takes the Studio's ARP replies); the POE-ISO build (#211) links at 100 Mbit but gets no lease. Findings in docs/EDGE.md Phase 2; picked up again by `node-resume`. G4/G5 not measured (G4 fails anyway at 28 ms scans).
  status: blocked
  id: node-modbus

- [ ] **EtherCAT master via ethercrab** — ethercrab (MIT/Apache-2.0, pure Rust) as the driver; EtherCAT slaves' PDOs mapped into the runtime's %I/%Q. Master only: a Pi cannot be an EtherCAT slave (needs an ESC chip).
  needs: a USB 3 gigabit NIC for the Pi (its one port is the Studio link) and one EtherCAT I/O slave to test against (e.g. a bus coupler + digital I/O terminal)
  result: an ST program toggles a real EtherCAT output from the Pi
  est: 1w
  difficulty: high
  id: ethercat-master
  notes: parked (fieldbus-hardware, 2026-10-31): no hardware until a customer asks.

- [ ] **PROFINET IO device (RT class 1)** — own Rust implementation over raw Ethernet (DCP, LLDP, RPC connect, cyclic RT frames); IRT is out (needs hardware). Lab use only: shipping a real device needs a PI vendor ID and certification.
  needs: a PROFINET controller to test against (a Siemens PLC, or a software controller) and access to the PROFINET spec (PI membership) or a decision to work from public captures + p-net's behaviour
  result: a PROFINET controller sees the Pi as an IO device and exchanges cyclic data with the ST program
  est: 3w
  difficulty: high
  id: profinet-device
  notes: parked (fieldbus-hardware, 2026-10-31): no hardware until a customer asks.
