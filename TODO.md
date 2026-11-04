# TODO

## Open questions for Nick

- [x] **Where does the relay live?** The relay holds the Anthropic key and meters boxes, which the vision puts on the paid side of open core, and the `open-core-split` card says to decide before paid code lands in this MIT repo. So relay v0 is being built in a NEW local repo `../helmsman-relay` (Rust, SAM template, no remote, nothing deployed). Creating the private GitHub repo and deploying the stack (account 403059190476, us-east-1) are your steps; scripts and cards are in that repo's TODO.md.
  asked: 2026-10-30
  by: claude-session
  needs: your call
  result: relay-v0 has a home; agent-loop can target its protocol
  options: private repo JimothyJohn/helmsman-relay (recommended) | relay/ in this repo, split later | private monorepo for all paid code (relay, fleet, market)
  est: 5m
  difficulty: low
  id: relay-repo
  notes: left (2026-10-31, later): run ../agent-relay `scripts/create-github-repo.sh`, then merge its `feat/images-metering` branch; deploying is `relay-deploy`.
  notes: 2026-10-31: the local repo is now ../agent-relay (renamed, client-agnostic, profiles namespaced helmsman/<name> with the old bare names as aliases). Tick once the GitHub repo exists: run its scripts/create-github-repo.sh.
  answer: private repo JimothyJohn/agent-relay (recommended), an agnostic endpoint that happens to work with helmsman
  notes: done 2026-11-03: JimothyJohn/agent-relay exists (private, main + dev), baseline wiring in its PR #1, the v4..v6 stack (feat/doc-fetch) in a PR into its dev. Left: relay-deploy.

- [x] **Give the E1213 a fixed address?** It is on the office LAN by DHCP at 10.0.0.57 (lease from 10.0.0.1), so the address in docs/devices/e1213-loop/io.map and the enip map can change under us. Options: a DHCP reservation for 00-90-e8-7e-af-be on the router (nothing to change on the unit), a static address on the unit outside the router's pool (I don't know the pool), or move it to the 192.168.3.x device link where the maps used to point (192.168.3.60). The maps say 10.0.0.57 until you choose.
  asked: 2026-11-02
  by: claude-session
  needs: your call (and the router's DHCP pool, or a reservation you set)
  result: a stable address in every map and script
  options: DHCP reservation on the router (recommended) | static on the unit, you name the address | move to 192.168.3.x
  est: 10m
  difficulty: low
  id: e1213-address
  answer: DHCP reservation on the router (recommended)
  notes: interview 2026-11-02: Nick changed to "find it by identity instead, skip the reservation": card `device-by-identity`; the maps keep 10.0.0.57 until that lands.

- [ ] **MCP clients cannot read /api/devices: allow it?** — found by the plant-states worker (2026-11-03): `mcp::reachable` lacks GET /api/devices, so `list_devices` fails over MCP (it always did) while it works for the relay agent. Allowing it widens what an engineer's own MCP client reads on the box (device names, addresses, link errors); read-only, no secrets.
  needs: your call
  result: list_devices works the same over MCP as through the relay, or stays relay-only on purpose
  options: allow GET /api/devices, /api/devices/library and the discover POST over MCP (recommended) | allow the GET only | keep MCP without devices
  est: 1h
  difficulty: low
  id: mcp-devices
  asked: 2026-11-03
  by: claude-session

- [ ] **Deploy the relay's agent v7, then point the bench Pi at it** — every turn on agents v3 to v6 is refused by the model API (strict tools with `minimum`, `maxLength`, `pattern`; found on the first live turn, 2026-11-04). agent-relay#4 (merged) adds `helmsman/v7`; deploying it was refused from the agent.
  needs: you to run the two scripts, in order
  result: a real chat turn from the bench Pi through the relay; then #218 leaves draft (box default v7)
  notes: `./scratch/06-relay-deploy.sh` (the key is stored: `!` works), then `! ./scratch/10-pi-agent-v7.sh` (adds --relay-agent v7 on the Pi, RESTARTS helmsman-plc, sends one read-only turn and checks text comes back)
  est: 10m
  difficulty: low
  id: relay-v7-deploy
  asked: 2026-11-04
  by: claude-session

- [ ] **Make v3 to v6 work on the wire too?** — boxes that ask for v3..v6 (the Pi's old binary defaults to v3) get a 422 on every turn. v7 fixes new asks; the relay could also strip the refused keywords from strict tools when it sends v3..v6, so old boxes work without a flag. That changes what a served id sends, which the profile rule forbids, so it is your call.
  needs: your call
  result: old boxes either work unchanged or are told to ask for v7
  options: leave v3..v6 as served; boxes move to v7 (recommended) | strip on the wire for v3..v6 | alias v3..v6 to v7
  est: 1h
  difficulty: low
  id: relay-old-profiles
  asked: 2026-11-04
  by: claude-session

## Tasks

- [ ] **Breakpoint on the picked block (Ctrl B)** — Nick 2026-11-01: keep it, off the F key (his "drop" on F9 overridden by his next answer). Pick a block or statement line in the simulation, Ctrl B pauses the scan before that statement runs, the diagram lit as of that moment; F10 runs the rest. Needs pause-at-statement in the interpreter (the trace tap knows the running statement). Box side later, read-only.
  needs: pause-at-statement in helmsman-iec's interpreter
  result: Ctrl B toggles a breakpoint on the picked block; a marker on the block and the ST line; the scan pauses there
  est: 2d
  difficulty: med
  id: hotkey-breakpoint-build
  asked: 2026-11-01
  by: claude-session

- [ ] **Step into / out of an instance (Ctrl ] / Ctrl [)** — Nick 2026-11-01: a non-F key. Stopped, Ctrl ] on a picked function-block block opens that instance's diagram and steps one scan; Ctrl [ goes up a level (Alt ↑ today) and keeps stepping.
  needs: nothing
  result: the two keys in hotkeys::TABLE, the open-and-step behaviour, docs/UI.md rows
  est: 1d
  difficulty: low
  id: hotkey-step-into-build
  asked: 2026-11-01
  by: claude-session

- [ ] **Force the picked input from the keyboard (Ctrl Shift F / Ctrl Shift U)** — Nick 2026-11-01: a non-F key. With a block picked, Ctrl Shift F forces it (flip a BOOL, open the number field), Ctrl Shift U releases every force (the banner's button). On a box the engineer's audited force path; in the simulation the switch.
  needs: nothing
  result: the two keys, acting through the existing force path
  est: 4h
  difficulty: low
  id: hotkey-force-build
  asked: 2026-11-01
  by: claude-session

- [ ] **Find in the Structured Text (Ctrl F, Ctrl G next, Ctrl Shift G previous)** — Nick 2026-11-01: keys that feel familiar rather than F3. A find box over the ST pane, the match marked like a diagnostic, Enter or Ctrl G next, Ctrl Shift G previous; the page takes Ctrl F from the browser (hotkeys::wants). Replace can wait.
  needs: nothing
  result: a find box in the ST pane on the three keys
  est: 1d
  difficulty: low
  id: hotkey-find-build
  asked: 2026-11-01
  by: claude-session

- [ ] **F2 / Shift F2 reserved for the Agent** — Nick 2026-11-01: reserve the keys only. F2 would focus the Agent prompt with the picked block or the ST selection as context; Shift F2 would ask it to declare the identifier under the caret, as a proposal. Nothing bound until decided.
  needs: a decision to build
  result: the keys stay free (docs/UI.md lists them as reserved)
  est: 2d
  difficulty: med
  id: hotkey-agent-reserved
  asked: 2026-11-01
  by: claude-session

- [ ] **Check the board backend on the Pi** — run the GPIO / I2C check against the pick PC: finds the header's GPIO chip (gpiochip0 or gpiochip4 on a Pi 5, by kernel), lists /dev/i2c-1, builds and runs `board_check` (helmsman-io's Linux backend, the code behind `helmsman-plc --gpio-chip`) reading `di:4` by default. Add `READ="di:4 i2c:1:0x48:0:i16"` for a sensor, and `LOOP="do:17=di:27"` only with a jumper from that output to that input (it drives the output).
  needs: on the Pi `sudo apt install gpiod i2c-tools`, `dtparam=i2c_arm=on` and a reboot, the SSH user in the gpio and i2c groups; Docker here
  result: the board backend proven on real GPIO (and I2C if a sensor is wired); then `--gpio-chip` goes into the deploy unit
  notes: `! scripts/pi-gpio-check.sh nick@192.168.3.20` (runs here, not in the native terminal; read-only unless LOOP is set)
  est: 30m
  difficulty: low
  depends: pi-deploy-iec, sources-modbus-gpio
  id: pi-gpio-check

- [ ] **Bump ruSTy past 1.0.7 and drop the unsigned-compare workaround** — PLC-lang/rusty#2015 fixes signed compares of UDINT/ULINT (and unsigned division, CASE, negation) on master; no release carries it yet. When one does: new version + SHA-256 digests in docker/rusty/Dockerfile, flip `rusty_unsigned_compare_bug` to assert agreement, drop the UDINT/ULINT dodge in `cond` (tests/oracle.rs), widen the sweep once.
  needs: a ruSTy release after 2026-10-21 (`gh release list -R PLC-lang/rusty`)
  result: oracle compares unsigned conditions directly
  est: 1h
  difficulty: low
  depends: iec-oracle
  id: rusty-bump

- [ ] **EtherCAT master via ethercrab** — ethercrab (MIT/Apache-2.0, pure Rust) as the driver; EtherCAT slaves' PDOs mapped into the runtime's %I/%Q. Master only: a Pi cannot be an EtherCAT slave (needs an ESC chip).
  needs: a USB 3 gigabit NIC for the Pi (its one port is the Studio link) and one EtherCAT I/O slave to test against (e.g. a bus coupler + digital I/O terminal)
  result: an ST program toggles a real EtherCAT output from the Pi
  est: 1w
  difficulty: high
  depends: iec-st-runtime
  id: ethercat-master
  notes: parked (fieldbus-hardware, 2026-10-31): no hardware until a customer asks.

- [ ] **PROFINET IO device (RT class 1)** — own Rust implementation over raw Ethernet (DCP, LLDP, RPC connect, cyclic RT frames); IRT is out (needs hardware). Lab use only: shipping a real device needs a PI vendor ID and certification.
  needs: a PROFINET controller to test against (a Siemens PLC, or a software controller) and access to the PROFINET spec (PI membership) or a decision to work from public captures + p-net's behaviour
  result: a PROFINET controller sees the Pi as an IO device and exchanges cyclic data with the ST program
  est: 3w
  difficulty: high
  depends: iec-st-runtime
  id: profinet-device
  notes: parked (fieldbus-hardware, 2026-10-31): no hardware until a customer asks.

- [ ] **See the box UI on the Pi** — the program tree, breadcrumbs, rearranging and the live diagram, on the real box, with the line demo (two stations × two motors, parts arriving on their own) so it moves with nothing wired.
  needs: Docker Desktop running on the Studio; the Pi's host key trusted here (the Studio has no known_hosts entry for 192.168.3.20, so SSH refuses it): read `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the Pi, then `scripts/pi-trust-host.sh 192.168.3.20 SHA256:…` (it offered SHA256:8vBxD7Qo0UB0ufxbbZFqn1gLukVcrUxZHp64iLBVlHU on 2026-10-31)
  result: helmsman-plc on the Pi running programs/line.st with the canvas on 127.0.0.1:8080; from the Air `ssh -J nick@10.0.0.41 -L 8080:127.0.0.1:8080 nick@192.168.3.20`, then http://127.0.0.1:8080/
  notes: `! scripts/pi-trust-host.sh 192.168.3.20 SHA256:…` runs here; then the agent (or you) runs `scripts/pi-deploy-plc.sh nick@192.168.3.20 crates/helmsman-plc/programs/line.st`, which restarts helmsman-plc on the Pi (the pick PC). Depends on #95 (tree) and #96 (line.st) being in the checkout.
  est: 30m
  difficulty: low
  id: pi-ui-demo
  asked: 2026-10-31
  by: claude-session

- [x] **Move the site to helmsman.advin.io** — the public page (master's docs/, GitHub Pages) moves to Nick's domain. Done 2026-11-02: master carries docs/CNAME, Pages serves https://helmsman.advin.io/ with HTTPS enforced, the datasheet, Why? and help pages are live.
  needs: a DNS record at advin.io's DNS host: `helmsman` CNAME `jimothyjohn.github.io.`; check with `dig +short helmsman.advin.io` (it should answer jimothyjohn.github.io)
  result: the site and playground at https://helmsman.advin.io/ and /playground/; github.io redirects there
  notes: order matters. DNS first; then the agent adds `docs/CNAME` (helmsman.advin.io) in a PR into dev, and the README link; it goes live at Nick's next dev → master merge. A CNAME before DNS sends visitors to a name that doesn't resolve. After it is live: Pages settings → Enforce HTTPS once GitHub has issued the certificate.
  est: 30m
  difficulty: low
  id: site-domain
  asked: 2026-10-31
  by: claude-session
  notes: DNS is live and docs/CNAME is in dev (#125); left: Nick's dev -> master merge, then Pages settings -> Enforce HTTPS. Tick it then.

- [ ] **Apply an L5X task's scan rate** — a converted periodic task's rate is only a note at the top of the generated `.st`; project.settings isn't touched, so the program runs at the default scan.
  result: an imported periodic task runs at its Logix period
  est: 2h
  difficulty: low
  id: l5x-task-rate
  asked: 2026-11-01
  by: claude-session

- [ ] **Test-started boxes die with their test** — a test process killed abruptly leaves its helmsman-plc running (two were found on the Studio, one holding port 8080). Give test children a parent-death signal, such as exiting when a stdin pipe closes.
  result: no box outlives the test run that started it
  est: 2h
  difficulty: low
  id: test-box-orphans
  asked: 2026-11-01
  by: claude-session

- [ ] **The Pi runs `--projects`** — deploy/helmsman-plc.service and scripts/pi-deploy-plc.sh start the box with `--projects` (state in /var/lib/helmsman-plc); a first install seeds project `line` from programs/line.st, deployed (`helmsman-plc project seed`, the unit's ExecStartPre); `PLC_MODE=program` keeps the old one-file mode. Built and tested here; yours to run against the Pi (native terminal or `!`: it needs Docker, trunk, and SSH with sudo on the Pi, and the firewall step is refused from an agent).
  needs: you to run the two commands (the Pi's IP); the Pi on Debian Trixie with key SSH and passwordless sudo (scripts/pi-prep.sh)
  result: the Pi serves the editor with projects, running `line`; you can sign in as an engineer and edit, deploy and force
  notes: PLC_API=0.0.0.0:8080 scripts/pi-deploy-plc.sh nick@<pi-ip>   then   scripts/pi-account.sh nick@<pi-ip> nick engineer
  est: 15m
  difficulty: low
  id: pi-projects
  asked: 2026-10-31
  by: claude-agent

- [ ] **Remove the dead tcpethernet worktrees** — `git worktree list` still names /Users/nick/github/tcpethernet-pi-demo (a checkout from before the rename whose .git file points at the old /Users/nick/github/tcpethernet path, so git cannot read it; it still holds HOWTO.md and a crates/ tree) and tcpethernet/.claude/worktrees/retro-20261030 (directory gone). Look at pi-demo for anything worth keeping, then remove both.
  needs: a look at the pi-demo directory
  result: `git worktree list` shows the main checkout and live worktrees only
  notes: `ls /Users/nick/github/tcpethernet-pi-demo` then `git worktree remove --force /Users/nick/github/tcpethernet-pi-demo` and `git worktree prune`
  est: 5m
  difficulty: low
  id: dead-worktrees
  asked: 2026-11-01
  by: claude-session

- [ ] **Supply the datasheet image** — docs/helmsman-datasheet.pdf (docs/datasheet.html) has no picture yet. Nick 2026-11-01: card it, place it when the file exists. Drop the file under docs/brand/ (PNG or SVG; a photo at 1200 px wide or more, or a render) and say where it goes: in the top band right of the wordmark (about 45 mm wide, the lede moves down) or as a strip under the lede. The build (`make datasheet`) refuses a second page, so the slot comes out of the spec table's air.
  needs: the image file and its spot
  result: the datasheet carries the image; rendered, one page, re-committed
  est: 30m
  difficulty: low
  id: datasheet-image
  asked: 2026-11-01
  by: claude-session

- [x] **Requirements become tests: the agent and the engineer agree on a result, and a test keeps it** — Nick 2026-11-01 (NEXTSESH): the killer feature is verification, the way Lean makes you prove the result you asked for. The conversation quantifies each requirement ("the lamp is on within 2 s of start_pb", "the belt never runs with the guard open"), each one becomes a simulation test (a scenario of inputs over time plus assertions on outputs and timing) kept with the project, and every deploy and every "Use this code" replays the whole set and refuses a regression, with the failing requirement named in the engineer's words. The agent proposes the tests from the spec; the engineer edits and approves them like code. Replay exists (`POST /api/projects/{p}/replay`, sim-replay); this is the requirement-to-scenario layer on top and the pane that shows red and green per requirement.
  needs: a small standard FB (ASSERT) and a test runner in the simulator and on the box; an agent profile that writes tests from a requirement
  notes: interview 2026-11-02: requirement tests are ST test programs in the project (`test_NAME.st`: script inputs, step the simulation N scans, ASSERT outputs and timing), deterministic, written and read by the agent in the same language, run in the page and on the box; a scenario table or agent-judged acceptance were declined. First cut (1w): the FB, the runner, red/green per test in a Requirements tab, and "Use this code" plus Deploy refusing a red one; the plain-words requirement is the test file's header comment.
  result: a Requirements tab: each requirement in plain words with its test, green or red after every deploy; the agent's code cannot land past a red one
  est: 1w
  difficulty: high
  status: ready
  notes: done 2026-11-02 night, PR #194: helmsman_iec::spec runner (two programs in lockstep), ASSERT FB (FAILED, FAILED_AT), test_*.st skipped by the build, POST /api/projects/{p}/tests, Requirements tab (right panel), Deploy 409 and Use this code refused on red. Left: nested ASSERT instances, located %IX inputs, the relay profile that writes tests from a requirement, a help picture.
  id: spec-tests
  asked: 2026-11-01
  by: claude-session

- [x] **"Use this code" keeps every iteration under one program, the newest on top** — Nick 2026-11-01 (NEXTSESH): "Use this code" should create a new program in a folder named by the user, where each iteration is the newest file and the earlier ones are hidden away in the Files pane. Today a PROGRAM answer becomes a project named from the PROGRAM (lamp_test, lamp_test-2, ...) and a block becomes a file in the open project. Wanted: the first answer asks for the program's name once; later answers to the same conversation land in that program as iterations (lamp_test/v3.st or a dated name), the pane shows the newest and folds the rest under a "earlier (4)" row, and the running one is marked.
  needs: nothing beyond the projects store
  notes: Nick 2026-11-02 (board): "just do serially for now. It will be wildly long, likely hundreds if not thousands but fuck it." Interview 2026-11-02: the newest is always `main.st`; the one it replaced is renamed `vN.st` (N counting up) and folded under "earlier (n)" in the Files tab; the build takes `main.st`, older ones are kept out of the build; the running one is marked. The first answer asks for the program's name once.
  result: a conversation's answers pile up under one named program, newest first, the pane uncluttered
  est: 2d
  difficulty: med
  depends: agent-pages
  status: ready
  notes: done 2026-11-02 night, PR #192 (merge queue, stacked on #187): FileKind::Iteration vN.st excluded from both builds, Name this program modal once per conversation, main.st newest, 'earlier (n)' fold, '· runs' mark. The name lives in page memory (asks again after a reload).
  notes: landed 2026-11-03 through #201 (#192 closed: its commits came in with #201).
  id: agent-iterations
  asked: 2026-11-01
  by: claude-session

- [x] **The Agent pane keeps the whole conversation as pages the engineer can curate** — Nick 2026-11-01 (NEXTSESH): every exchange is saved for context, shown as pages with back and forward arrows to switch between iterations; a page can be deleted; every remaining page goes into the context each time, so the engineer shapes what the agent sees to improve the output. Today the history lives in page memory only and "New conversation" clears it.
  needs: the conversation stored with the project (sim: browser storage; box: the store), a page strip in the tab
  result: pages with ‹ › arrows and a delete, the kept pages always sent as context, surviving a reload
  est: 2d
  difficulty: med
  notes: done 2026-11-02 night, PR #187 (merge queue): agent.md project file kind, page strip ‹ ›, delete, kept pages as context ≤512 kB, hidden from Files.
  notes: landed 2026-11-03: #187 merged.
  id: agent-pages
  asked: 2026-11-01
  by: claude-session

- [x] **Plant-model state variables map to real feedback devices, with suggestions** — Nick 2026-11-01 (NEXTSESH): the state variables a simulation's plant model keeps (the tank level, the belt position, the compressor pressure) should be easily replaceable by feedback devices on the real machine, and the agent should suggest the sensor that would capture each one, so what runs is modelled as closely as possible and the valuable signals are not left unmeasured. Each plant-model variable gets a "measured by" slot in the I/O map; empty slots are listed with a suggested device type (level transmitter, encoder, pressure transducer) and the agent's reasoning.
  needs: the plant-model blocks declaring their state variables (a convention or a pragma), the I/O map's point for each
  notes: Nick 2026-11-02 (board): do it now with my own judgement. Judgement: a `(* state *)` pragma on a plant_model.st VAR marks a state variable; the Devices tab lists them under a "plant states" row with a `measured by` slot that is a map `in` line of the same name; the agent's suggestions (sensor type and reasoning) come from a tool reading the list; the simulation reads the mapped signal when one exists. Starts after `agent-scoped-edits`.
  result: a list per project of plant states, which sensor measures each on the machine, and suggestions for the rest; the simulation reads the sensor when one is mapped
  est: 4d
  difficulty: med
  depends: machine-template
  status: ready
  notes: 2026-11-02 night: worker in progress (feat/plant-states) once #202 is in.
  notes: landed 2026-11-03: #206 merged ({state} pragma, Plant states in the Devices tab, the simulation runs from a mapped sensor, Add a sensor…, in list_devices).
  id: sim-feedback-devices
  asked: 2026-11-01
  by: claude-session

- [ ] **Run, stop and restart a box's program from the Monitor** — Nick 2026-11-01: the Monitor needs play, start, stop. The simulation's transport (run or stop, step, restart) is on the Monitor now; a box has no such endpoint: a module runs from deploy until the next one. This adds `POST /api/modules/{name}/run|stop|restart` behind an engineer session and the approval policy, audited like a force, with HELD shown while stopped (outputs at their safe values), and the Monitor's buttons use it when signed in.
  needs: a decision that an engineer may stop a running module from the page (the policy says who); the runtime's held state exposed per module
  result: the same three buttons on a box's Monitor, audited; the module's state shows HELD while stopped
  est: 2d
  difficulty: med
  id: monitor-box-run-stop
  asked: 2026-11-01
  by: claude-session

- [x] **The Agent edits only what is in focus, and every edit is filed, versioned and tested** — Nick 2026-11-01: the agent should only edit what it is focused on, and each of those edits should be file-managed, version-controlled and tested, the way a developer's change goes through a branch, a review and CI. Today "Use this code" replaces or adds a whole file and the answer carries whatever the model chose to rewrite. Wanted: the agent's scope is the file or block the engineer has picked (the ST selection, the picked block, the open file), it proposes a change to that scope alone and says so; each change lands as its own file or iteration (agent-iterations), as a commit in the project's history with the conversation page that produced it (version-control), and only after the project's requirement tests pass (spec-tests). A change outside the scope is refused by the page, not left to the model.
  needs: agent-iterations, spec-tests, project-history
  notes: Nick 2026-11-02 (board): do it now with my own judgement. Interview 2026-11-02: the focus is the open file by default, the selected lines or the picked block when there is one, and the page says which rule applied; a change outside it is refused by the page (a byte-for-byte check against the rest of the project). History is `project-history` (revisions on the box), not git.
  result: an agent change names its scope, lands as one filed and committed iteration, and cannot skip the tests
  est: 1w
  difficulty: high
  depends: agent-iterations, spec-tests, project-history
  status: ready
  notes: 2026-11-02 night: worker in progress (feat/agent-scoped-edits) once #187 #192 #194 #201 are in: scope module (file, selection or picked block), <edit_scope> envelope, byte-for-byte refusal outside the scope.
  notes: landed 2026-11-03 in the simulation: #207 (scope.rs, <edit_scope>, byte-for-byte refusal, tests gate, one revision). A box has no Agent tab yet: card agent-on-box.
  id: agent-scoped-edits
  asked: 2026-11-01
  by: claude-session

- [x] **Per-project history on the box: every save and every agent answer is a revision** — interview 2026-11-02 (the version-control card's first phase, chosen over git): each save and each "Use this code" writes a numbered revision into the project's folder on the box (the files plus the conversation page that produced it as a `.md`), listable in the Files tab (who, when, what), diffable as Structured Text, restorable as a new revision; deploys keep snapshotting into the store. Works from a browser; no git binary. `version-control` keeps the git export and remotes for later.
  needs: nothing new (the projects store and the agent pages)
  result: a History row per project: revisions with their conversation, diff and restore; the agent's edits are never lost or anonymous
  est: 3d
  difficulty: med
  depends: agent-pages
  status: ready
  notes: done 2026-11-02 night, PR #201 (merge queue, stacked on #192): <state>/projects-history/NAME/NNNNNN with the page, GET …/history, GET …/history/{n}, POST …/history/{n}/restore (engineer), History row, LCS diff, restore; sim keeps 20 in browser storage.
  notes: landed 2026-11-03: #201 merged.
  id: project-history
  asked: 2026-11-02
  by: claude-session

- [x] **Trend window: an exponential slider from 100 ms to 10 min, live** — Nick 2026-11-02 (NEXT.md): the window should go down to 1 s and 100 ms, and instead of the three buttons a slider, exponential, that changes the drawn window on the fly. The choppy line seen from localhost is a known quirk he has seen before and expects to vanish on a box; verify once, do not chase it.
  needs: nothing
  result: one slider under the Trend (log scale, 100 ms ..= 10 min, the value shown), the plot following as it moves; the three buttons gone; the choice kept per browser as before
  est: 4h
  difficulty: low
  notes: done 2026-11-02 night, PR #185: one log slider in both trends, 100 ms..10 min, value beside it; choppiness not reproducible headless, left alone.
  id: trend-window-slider
  asked: 2026-11-02
  by: claude-session

- [ ] **Modbus identification probe (FC 43/14) for discovery and identity** — left out by the device-by-identity and discovery workers (2026-11-02 night): `cip=` identities and the ListIdentity sweep cover EtherNet/IP; a Modbus-only device is found today only by `mac=` once the host has seen it. Add Read Device Identification (FC 43, MEI type 14: VendorName, ProductCode, MajorMinorRevision) to helmsman-io's modbus driver, a bounded probe of the ARP table's hosts on 502 in `POST /api/devices/discover` when asked (`?modbus=1`), a `modbus=VENDOR:PRODUCT` identity endpoint resolved by that probe, and the library lookup by it (`identity.modbus` in device.toml exists). Cite the Modbus Application Protocol spec section in docs/SOURCES.md.
  needs: a Modbus device that answers FC 43/14 to test on (does the E1213? check on the bench; the simulator answers it either way)
  result: a Modbus-only block is found and matched like an EtherNet/IP one
  est: 1d
  difficulty: med
  depends: device-by-identity
  id: modbus-identification
  asked: 2026-11-02
  by: claude-session

- [x] **A device named by its identity, not its address** — interview 2026-11-02 (the E1213 is a DHCP lease; Nick chose this over a router reservation): a map's `device` line may give `mac=00-90-e8-7e-af-be`, or `vendor=991 product=515 serial=0x2A629EAE`, in place of a host; at start and after a link loss the box resolves the address (ListIdentity broadcast for EtherNet/IP, ARP or a Modbus identification scan for Modbus TCP) and logs what it found; the Devices tab shows both the identity and the address resolved. The E1213 maps move to `mac=` once this lands.
  needs: nothing new (ListIdentity discovery exists; ARP reading is per platform)
  result: a DHCP device keeps working when its lease moves; a map travels between networks
  est: 1d
  difficulty: med
  notes: done 2026-11-02 night, PR #195 (merge queue): helmsman_io::resolve, `mac=` and `cip=V:P[:S]` endpoints, ListIdentity sweep, ARP table, /api/devices address; E1213 maps use mac=. No Modbus FC 43/14 probe (card modbus-identification).
  notes: landed 2026-11-03: #195 merged. The Modbus probe is card modbus-identification.
  id: device-by-identity
  asked: 2026-11-02
  by: claude-session

- [ ] **The Agent on a box** — left out of agent-scoped-edits (#207): a box's canvas has no Agent tab (BoxView::tabs() is I/O and Requirements; a ui_test pins it), its page's CSP is connect-src 'self', and the box's agent runs through the relay as proposals (POST /api/chat). Give the box an Agent tab that talks to /api/chat, with the pages (#187), iterations (#201), the requirement gate (red_tests) and the edit scope (`scope::apply` takes plain files, so it is reusable) on the box side.
  needs: the relay deployed (relay-deploy) to try it against a real model; the rest builds against the relay fixtures
  result: the same agent workflow on a box as in the playground, every change still a proposal through the approval policy
  est: 3d
  difficulty: high
  depends: agent-scoped-edits
  id: agent-on-box
  notes: 2026-11-04 overnight, PR #217 (first cut): the Agent tab on a box over POST /api/chat, streamed, tool rows, proposals -> Review #n -> Proposals, status line; tested against the real box API and a scripted relay. Left: pages and iterations on the box, scope::apply box side. Tried for real on the bench Pi: the relay answered 422 (v3..v6 strict tools carry minimum/maxLength/pattern, refused by the model API); agent-relay#4 adds helmsman/v7, #218 (draft) makes it the box default once deployed.
  asked: 2026-11-03
  by: claude-session

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

- [ ] **Relay v0** — the boxes' only path to Claude: per-box token, the Anthropic key server-side, usage metering, versioned system prompt and tool schema. Dev runs against a local relay with a .env key.
  needs: the AWS account/region for the stack
  notes: API Gateway + Lambda (response streaming) + DynamoDB for box tokens and usage (relay-hosting)
  result: no API keys on boxes; prompts upgrade without reflashing
  est: 3d
  difficulty: med
  depends: relay-hosting
  id: relay-v0

- [ ] **M1 demo run** — the sentence at the top of this section, on the bench, recorded. scripts/m1-demo.sh brings the box up from a clean deploy.
  needs: CompactLogix on the bench
  result: the thing to show people
  est: 1d
  difficulty: med
  depends: logix-tag-read, gpio-outputs, agent-loop, proposal-ui, hot-swap, canvas-web
  id: m1-demo
  notes: plan (m1-demo-plan, 2026-10-31): E1213-only demo first, the CompactLogix version after.

- [ ] **Tabletop I/O bench** — one Modbus TCP remote I/O block with 24 V DI/DO on the Studio link (192.168.3.x), outputs DO0-3 wired back to inputs DI0-3, a 24 V supply, and a Pi GPIO to 24 V output stage for do:N. Then `conformance --endpoint <ip>:502 --loop coil:0=di:0 ...` against it.
  needs: hardware bought and wired (any Modbus TCP 8DI/8DO block; note its manual's register map and whether it numbers from 0 or 1)
  result: the harness passes on real hardware, and agents have a bench that proves writes physically
  est: 4h
  difficulty: low
  depends: io-map
  id: io-bench

- [ ] **Agent writes a driver from the manual** — give the agent a device's documentation (PDF to text, as done for the Modbus spec); it writes the point map and, for a new protocol, a WASM driver, and iterates on harness output: simulator first, then the bench. Engineer approves the map before any output is bound.
  result: a new device or protocol without hand-written driver code
  est: 1w
  difficulty: high
  depends: wasm-driver, io-bench, agent-loop
  notes: EDS part started (feat/moxa-e1213): `helmsman_proto::eds` reads a vendor EDS and resolves its Class 1 connections, `enip-io` takes `eds=PATH`, so for an EtherNet/IP device the agent's job is choosing points, not writing a driver. Still to do here: the agent reading a manual's data map (Appendix B for the E1213) into points, and iterating on harness output.
  notes: 2026-11-02 night: the EDS half is PR #191 (device-import; in the merge queue); the manual half is the relay's helmsman/v5 profile (../agent-relay feat/driver-agent-profile): look up, EDS over hand-written points, manual table → cited map lines, inferred marked, propose and stop. Tick when #191 lands and the relay is deployed with v5.
  notes: 2026-11-03: EDS half #191 merged; the manual half is the relay's helmsman/v5 profile (feat/driver-agent-profile). Tick with the relay deploy.
  id: driver-agent
  needs: the EDS (devices/moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds, untracked) first, then the manual's Appendix B
  notes: Nick 2026-11-02 (board): "JUST USE THE EDS FILE FIRST TO MAKE SURE IT'S LEGIT". Order: the EDS path end to end through `device-import`, then the manual-to-points part.
  status: ready

- [x] **Bench: the Moxa ioLogik E1213 over EtherNet/IP** — `scripts/bench-e1213.sh` against the real E1213: ListIdentity (vendor 991, product 515), the Identity object, then the conformance harness over a Class 1 exclusive-owner connection at 100 ms (DI reads, DI-00's counter, the DO status byte, the four DO->DI loops, a point past the 51-byte input assembly). Turns the inferred parts of docs/devices/moxa-iologik-e1213.md into facts (bit order, counter byte order, DIO numbering): note what the run shows there.
  needs: the E1213 on the Studio link with its IP; EtherNet/IP enabled on its web console and activated for this unit (manual pp. 3-26, 5-1); DO-00..03 wired to DI-00..03 with field power (or `LOOPS="do:0=di:4 ..."`); UDP 2222 free on the Studio; the Moxa EDS at the repo root to check it too (optional)
  result: the enip-io driver passes on the real device; the device doc's "inferred" lines confirmed or corrected
  notes: `scripts/bench-e1213.sh <E1213-IP>`; the first run in the native terminal (the conformance binary is unsigned and the firewall asks once to accept incoming UDP; over SSH every read times out), later runs as `! scripts/bench-e1213.sh <IP>` here
  notes: 2026-11-02: the unit is on the office LAN at 10.0.0.57 (DHCP), EtherNet/IP already activated and enabled, EXT power wired, DO-00..02 looped to DI-00..02. Taken with a Python scanner (Apple's python3 passes the firewall): the E1213 names UDP 5222 for O->T in its Forward_Open reply and ignores O->T on 2222; the scanner and the simulator now honour that (feat/e1213-real-device); bit order and byte 50 confirmed; everything is in docs/devices/moxa-iologik-e1213.md "On the bench". Left: `! ./scratch/02-allow-firewall.sh` (sudo: lets the cargo binaries receive UDP), then `! scripts/bench-e1213.sh 10.0.0.57`, then tick this and start `enip-in-box-again`. Over Modbus the loop check (docs/devices/e1213-loop) already runs: 11 cycles, 0 faults.
  est: 30m
  difficulty: low
  depends: io-bench
  notes: done 2026-11-02 (after `scratch/02-allow-firewall.sh`): `scripts/bench-e1213.sh 10.0.0.57` → ListIdentity rev=3.1, 8 PASS over Class 1 (di:0, in:2:u32, in:50.0, do:0, the three wired loops, absent in:51.0). The loop check runs over EtherNet/IP in the canvas: 11 cycles, 0 faults.
  id: e1213-bench

- [x] **Put `enip-io` back in the box build** — `enip-in-box` took the EtherNet/IP Class 1 scanner out of the box until the bench passes: helmsman-plc's `box` no longer turns on helmsman-io's `enip` (so no helmsman-proto `scanner`), a box map naming `enip-io` is refused as "not in this build" (`helmsman_plc::drivers::not_in_build`), and the E1213 runs over Modbus/TCP (`docs/devices/moxa-iologik-e1213-modbus.map`). The code stays behind helmsman-plc's `enip-io` feature, which `scripts/lint.sh` lints. Once `e1213-bench` passes: add `enip-io` to `box` (or `default`), flip the `scanner` assertion in `tests/box_build.rs` back, drop the `cfg(not(feature = "enip-io"))` refusal tests in `tests/bin.rs` and `tests/driver_map.rs` (the shared-port test in `tests/bin.rs` then runs again), and restore docs/ARCHITECTURE.md's trust-boundary row.
  needs: `e1213-bench` green on the real E1213
  result: a box masters EtherNet/IP remote I/O again, from the default build
  est: 1h
  difficulty: low
  depends: e1213-bench
  notes: done 2026-11-02 (feat/enip-in-box): `enip-io` in `default`; box_build.rs asserts `scanner` present, `unrestricted` absent; the two not-in-this-build tests gone (the guard stays for a build without the feature, which lint.sh now lints as `--no-default-features --features box,https`); ARCHITECTURE row restored.
  id: enip-in-box-again

## Roadmap M2: floor-ready core

- [ ] **A library of known devices, and their points as tags in ST** — Nick, 2026-11-02 (while the E1213 came up): build a library of devices and the drivers for them (where it lives is open: docs/devices/ holds the E1213's doc, maps and the enip profile today; a `devices/` crate or a per-device folder with EDS, manual citations, profile, default map and a loop-check program are the candidates). Locally, pick inputs and outputs from the wired, known devices and have them come into a program as tags, as other PLC IDEs do, instead of hand-writing `in di0 bool e1213 di:0` lines; an I/O-like tab is where the user chooses them, and the same list is the context the agent gets when it writes the program. Nick wants to play with the interface against the live E1213 before fixing the shape.
  needs: the E1213 bench green (`e1213-bench`), then Nick's hands-on notes on how choosing a device's points should feel
  result: a second device is a folder in the library, not a driver project; a program names tags the device list offers; the agent sees the same list
  est: 1w
  difficulty: med
  depends: e1213-bench
  id: device-library
  notes: 2026-11-02 (feat/devices-pane): the Devices tab exists: the map's devices as a tree with their points and live values, New device… as a form writing a `device` line, the listing as the agent's context (`<io_devices>` in the page, `list_devices` on the box). Next is `device-import`.

- [x] **Import a device from its file: an EDS, or a manual the agent reads** — Nick, 2026-11-02: in the Devices tab, "Import device…" takes a file. A known kind (an EDS, EtherNet/IP) becomes a device line the box can run (`enip-io ... eds=PATH`: the box keeps the file, `helmsman_proto::eds` reads it at start, the points come from its assemblies); anything else (a Modbus register map in a PDF manual, a datasheet) goes to the agent as a drop-in (POST /api/dropins already takes PDFs and documents) with "add this device", and the agent proposes the map lines (`propose_io_map`) for the engineer to approve. Needs a place for device files on the box (not a project file kind today: `<state-dir>/devices/NAME.eds` and an API route, or a new project file kind so the file travels with the project zip) and the Agent tab on a box (`claude-on-box`). Test subject: `devices/moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds` (untracked), against the bench E1213 at 10.0.0.57.
  needs: a decision on where device files live (box state dir vs project file kind); the Agent tab on a box for the manual path
  result: a device goes from a vendor file to points in the tree without hand-written map lines; an unknown file still gets there through the agent, with approval
  options: box state dir + API route (EDS stays out of the project zip) | a project file kind (travels with the project; the zip grows) | both: project first, box dir later
  est: 2d
  difficulty: med
  depends: device-library
  status: ready
  notes: done 2026-11-02 night, PR #191 (merge queue): flat NAME.eds project file kind, Import device… in the Devices tab, eds=NAME.eds resolved from the project folder; the E1213 EDS checked as fixture by hash, never committed.
  notes: landed 2026-11-03: #191 merged.
  id: device-import
  answer: Start with the EDS to ensure it works.
  notes: interview 2026-11-02: device files are a project file kind (`devices/NAME.eds`, like `drivers/NAME.wasm`; travels with the zip); the Devices tab gets "Import device…"; an EDS becomes `device NAME enip-io ADDR eds=devices/NAME.eds` and the box reads it at start. Every imported device is also offered to the public library (`devices-repo`) so nobody maps it twice.

- [x] **Stock module: counters and uptime** — cycle counts, run/idle/down state, shift totals; the first shipped module, written in ST like everything else, so it doubles as the agent's few-shot example.
  est: 2d
  difficulty: low
  depends: m1-demo
  status: ready
  notes: done 2026-11-02 night, PR #190: starters/counters (Template): debounce + production_counters blocks, run/idle/down, shift totals on shift_id, uptime %, plant model, io.map on box channels.
  id: stock-counters
  notes: Nick 2026-11-02 (board): do it now with my own judgement. Judgement: a starter under Templates (`counters`): cycle count from a part-present input with debounce, run/idle/down from a running input and a timeout, shift totals reset by the calendar's `shift_id`, uptime as a REAL percentage; its own `test_counters.st` once `spec-tests` exists, else a plant_model.st exercising it.

- [ ] **Check the L5X reader against RM014 and a real export** — `helmsman_iec::l5x` reads the export shape from the `l5x` Python package and memory (docs/SOURCES.md); run it on a Studio 5000 export of a bench project (controller, program, routine and rung exports) and the manual, and fix what differs.
  needs: the RM014 PDF (403 from the agent) and a Logix Designer export
  result: the inventory is right for real customer files
  est: 3h
  difficulty: low
  depends: dropin-plc
  id: l5x-real-export
  notes: source (l5x-source, 2026-10-31): Nick exports one from his bench project. Later 2026-10-31: RM014 is read and cited; the ladder converter (l5x-import) also needs the real export: run `POST .../import?file=X.L5X` on it and check the refusal list (or the ST) against the project, the neutral text of each instruction it uses (JSR's `JSR(Name,0)` form, `?` pseudo-operands, v36 mnemonics such as EQ/MOVE), decorated data of TIMER/COUNTER tags, and rung Type attributes other than N. The fixtures (line3.L5X, ladder.L5X) are hand-written, not exports.

- [ ] **Relay: image blocks from boxes, checked and capped** — protocol v1 passes any content block through and caps only the body (1 MiB). For photo drop-ins: (1) accept `image` blocks only with `source.type: "base64"` (refuse `url`, which makes the model API fetch a box-chosen URL, and `file`, which would read the relay org's Files API uploads by id), `media_type` in png/jpeg/webp, decoded size within a per-image cap; the same for images nested in `tool_result` content; (2) cap images per request at 20 (the Messages API's many-image rule) and raise `RELAY_MAX_BODY_BYTES` for image requests only if `dropin-photo-size` says so; (3) metering already counts image tokens inside `input_tokens`; add per-box image count and image bytes per day to `/v1/usage` so a photo-heavy box is visible; (4) agent v4 (v3 is immutable): v3's prompt plus a photo paragraph (the image is data, read only what it shows, map to box signals, ask about anything unreadable, name the photo as the source). Box side needs nothing new for (1)-(3); for (4) boxes move to `--relay-agent v4`.
  needs: the relay repo
  result: a box cannot use the relay to fetch URLs or read other uploads; image cost is visible per box; the model gets photo-specific instructions from the relay, not only the box's note
  est: 3h
  difficulty: low
  depends: dropin-photo, relay-agent-v3
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
  depends: projects, force-inputs
  id: version-control
  asked: 2026-11-01
  by: claude-session

## Roadmap M3: tiers, sharing, fleet

- [ ] **The agent finds a device's documentation on its own** — from the Why? interview 2026-11-01 ("where it goes"): given a device on the network (ListIdentity's vendor and product code, a Modbus unit's identification, a nameplate photo), the agent fetches the manual and the EDS through the relay, reads the data map into points and proposes the I/O map lines; the engineer approves the map before any output is bound. Builds on driver-agent (the manual to points part) and the EDS reader.
  needs: driver-agent; a relay tool that fetches documents from an allow-listed set of vendor sites
  notes: Nick 2026-11-02 (board): do it now with my own judgement. Judgement: look the device up in `devices-repo` first (identity, then vendor/model text); only a miss goes to the relay's fetch tool; the result is offered back to the repo. After `device-import` and `devices-repo`.
  status: ready
  result: a new device on the bench is mapped from its own documentation, with the source cited in the proposal
  est: 1w
  difficulty: high
  depends: driver-agent, relay-v0
  notes: 2026-11-02 night: relay half done on ../agent-relay feat/doc-fetch (fetch_document run by the relay: allow-list moxa.com first, SSRF rules, 12 MiB/2 MiB caps, 40 fetches + 100 MiB a day per box in the ledger, library-first, profile helmsman/v6); box half in progress (tool → relay route → drop-in; default agent v6).
  notes: 2026-11-03: box half #205 merged (fetch_document, default agent v6); relay half on ../agent-relay feat/doc-fetch. Tick once the relay is on GitHub and deployed (relay-repo, relay-deploy).
  id: device-docs-lookup
  asked: 2026-11-01
  by: claude-session

- [ ] **A public device library: map a device once, for everyone** — interview 2026-11-02: a public repository (JimothyJohn/helmsman-devices) with one folder per device: the EDS or the Modbus map, the `enip-io` profile or map lines, a doc with the manual's citations, the identity (vendor, product code, model text). The box and the agent look a device up before generating anything: by identity first (ListIdentity's vendor and product code, a Modbus unit's identification), then by vendor/model text; semantic search via the relay only once there are enough entries to need it. An import in the Devices tab offers to publish the result (a PR to the repo from the relay, the engineer confirming).
  needs: Nick creates the public repo (a scratch script); the licence line per vendor file (the Moxa EDS carries no terms: a pointer and a hash, not the file, until Moxa answers)
  result: the second user of an E1213 gets its points from the library in one click; the agent's mapping work is never repeated
  est: 2d
  difficulty: med
  depends: device-import
  notes: 2026-11-02 night: github.com/JimothyJohn/helmsman-devices public (main + dev; Nick ran the script), E1213 first entry; PR #1 there adds index.json + scripts/build-index.py (dev → main is Nick's). Lookup in helmsman: PR #202 (merge queue): POST /api/devices/discover, GET /api/devices/library, On the network + Add to the map…, --devices-index. Identity first, text second; semantic later.
  notes: 2026-11-03: #202 merged (discovery + lookup in helmsman). Left: helmsman-devices PR #1 (index.json) into its dev, then dev → main there (Nick) so boxes find matches; tick then.
  id: devices-repo
  asked: 2026-11-02
  by: claude-session

- [ ] **Map the network from the electrical drawings and the bill of materials** — "where it goes": the drawings drop-in (dropin-pid) reads one sheet into points today; this reads a whole drawing set plus the BOM into the devices, their addresses and the wiring between them, and proposes the complete I/O map and the device list, cross-checked against what ListIdentity and the Modbus scan actually find. Differences become questions to the engineer.
  needs: dropin-pid; a drawing set and BOM from a real project to test on
  result: an I/O map and device list from the project's documents, with every difference from the live network listed
  est: 2w
  difficulty: high
  depends: dropin-pid, device-docs-lookup
  id: network-from-drawings
  asked: 2026-11-01
  by: claude-session

- [ ] **Sort out and explain the behaviours already present in a system** — "where it goes": from recorded trends, the trace tap and sim replay, the agent groups what the system does into named behaviours (a cycle, an interlock, a fault pattern), explains each in plain words with the signals that drive it, and flags the ones no program accounts for. Explain mode today works on one program; this works on the plant's recorded behaviour.
  needs: box-trend-source and sim-replay recordings from a running machine; explain-mode
  result: a behaviour list for a machine, each one explained and traceable to its signals
  est: 2w
  difficulty: high
  depends: explain-mode, sim-replay
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
  depends: relay-v0, bundle-format
  notes: 2026-11-02 night: the node half moved: helmsman-node runs the scan bench and Modbus (PRs #193, #199); its own API + site beacon and the Site view across nodes are what this card is now.
  id: fleet-console
  needs: a decision taken (interview 2026-11-02): no console host at all; see notes
  notes: Nick 2026-11-02 (board): "I like this because it also allows me to set it up like my ../github/supervictor project. And now I realize that I may not have enough cores to do what I need with the network, uplink, communication, and program / process that may get complicated." Interview 2026-11-02: the fleet lives on the ESP32-POE2 as "every node is its own box": each node runs the runtime, serves its own API and announces itself on the LAN (`--site`), the fleet view is the canvas's Site view across nodes from any laptop, history and uplink go node → relay. Nothing in the middle to host. This card becomes: the Site view across boxes and nodes, the relay as the uplink, OTA of the node firmware (signed, A/B slots) once the node runs; depends on the edge spike (`node-modbus`) for the node half.
  status: ready

- [ ] **Resume the edge node: faster scans, PSRAM, and the network receive path** — parked by Nick 2026-11-03 after the first runs on the boards (docs/EDGE.md Phase 1 and 2). Three open threads, in order: (1) the network: on the POE2 frames from the Studio never register (ARP replies unanswered, no UDP report sent) and the POE-ISO gets no DHCP lease; first flash supervictor's known-good POE-ISO firmware on the same board to tell this firmware from the board or switch, then compare the executor and interrupt setup (esp-rtos) and the main task's share of the CPU; (2) scan speed: 28 ms for the 4k-statement bench against < 5 ms; measure flash at 80 MHz QIO (it boots at 40 MHz DIO), the interpreter's i128 `Value::Int` on a 32-bit core, the no_std BTreeMap lookups, esp-alloc heap-stats, before any rewrite (a compact compiled form or compile-on-laptop is the bigger lever); (3) the PSRAM corruption: compiling the batch-mixer starter with PSRAM asks for 0x3F40D781 bytes (a rodata address read as a length).
  needs: Nick's call to resume; the POE-ISO (WROOM, /dev/cu.usbserial-1130 on 2026-11-03) for the network thread since USB can stay on with PoE; the POE2 for PSRAM
  result: a node that scans a realistic program inside its period and talks to the Studio, or a firm no with numbers for each thread
  notes: the Studio's en0 has 10.0.0.241 (scratch/04-studio-wired-10net.sh; `off` removes it, a reboot too); LOG_SINK=10.0.0.241:9514 puts the reports on the wired side. Builds: scripts/node-build.sh [net], NODE_BOARD=poe-iso; flash: scripts/node-flash.sh [bench|modbus], Nick's step.
  est: 1w
  difficulty: high
  depends: node-scan-bench
  id: node-resume
  asked: 2026-11-03
  by: claude-session

- [ ] **Vision sources** — deferred per interview; card kept so the signal model leaves room for camera-derived signals.
  est: 2w
  difficulty: high
  id: vision

## Roadmap M4: marketplace and the box

- [ ] **Community marketplace** — publish/browse/install bundles; floor per market-floor, then an AI review on install that explains risk in plain words and can only refuse.
  est: 3w
  difficulty: high
  depends: bundle-format, market-floor
  notes: walking skeleton done 2026-11-02 night, PR #196: bundle::index (signed canonical JSON), bundle::market fetch (public hosts only), CLI bundle index/fetch, POST /api/market/list and /api/market/install through the bundle import and approval policy, --market-allow-local. Card stays open for accounts, payments, the AI review.
  id: marketplace
  needs: the walking skeleton only, for now
  notes: Nick 2026-11-02 (board): do it now with my own judgement; interview: walking skeletons. Cut: a static, signed index of bundles (a JSON file and the bundles in a public repo, signed by trusted publishers), a box lists it and installs from it through the existing bundle path and approval policy; no accounts, no payments, no AI review yet. Est for the skeleton: 2d.
  status: ready

- [ ] **Box SKU** — reproducible Pi image, first-boot pairing with the relay, provisioning script, enclosure/BOM for the preloaded kit.
  est: 2w
  difficulty: high
  depends: relay-v0
  notes: walking skeleton done 2026-11-02 night, PR #198: scripts/pi-image.sh (pi-gen in Docker, pinned ref), helmsman-firstboot pairing files → LoadCredential, docs/BOX.md; not booted. Pairing by code needs a relay endpoint (relay card). Card stays open for enclosure/BOM and a booted image.
  id: box-sku
  needs: the walking skeleton only, for now
  notes: Nick 2026-11-02 (board): do it now with my own judgement; interview: walking skeletons. Cut: a reproducible Pi image build script (pi-gen or rpi-image-gen, the box binary and the canvas preinstalled, the systemd unit, first boot pairs with the relay from a one-time code) checked by booting it in QEMU in CI; no enclosure or BOM yet. Est for the skeleton: 2d.
  status: ready

- [x] **Open-core split** — decide which crates stay MIT and where relay/fleet/verified-market code lives (private repo), before any paid code lands here.
  est: 2h
  difficulty: low
  depends: relay-v0
  notes: done 2026-11-02 night, PR #186: LICENSING.md + NOTICE. This repo STAYS Apache-2.0 (commit 1f7a4de, 2026-11-01); relay MIT (on its feat/usage-metering branch), device library MIT, marketplace server side private. Flip this repo to MIT only if Nick says so (one Cargo line + LICENSE).
  id: open-core-split
  needs: nothing: decided
  notes: Nick 2026-11-02 (board): "relay can be MIT. Fleet is Apache, marketplace should be private. Add Stripe integration to possible charge upon uplink of datapoint and reading of data and ingestion by users or agents on a usage rate tied as close as possible to energy usage (that last part may be too abstract and separated from cost though)". Interview: meter now, charge later (`usage-metering`). To do here: LICENSE files (this repo and the relay MIT, the fleet/site code Apache-2.0 with a NOTICE, the marketplace private), a LICENSING.md naming which is which.
  status: ready

- [x] **Usage metering in the relay: a ledger now, Stripe later** — interview 2026-11-02 (open-core-split's billing half): the relay records usage events per box (datapoints uplinked, reads served, agent tokens in and out) in a durable ledger with a daily roll-up per box and per account; Stripe test-mode meters are wired to the roll-up behind a flag, no live charging and no prices set. Rates can be set later, energy-linked or not; the ledger is what any rate multiplies.
  needs: the relay deployed (`relay-deploy`); a Stripe test-mode account (Nick's step)
  result: every box's usage is counted and visible per day; turning billing on is a flag and a price, not a build
  est: 2d
  difficulty: med
  depends: relay-v0
  notes: done 2026-11-02 night on ../agent-relay feat/usage-metering (merges feat/images-metering): per-account roll-ups, datapoints/reads counters, Stripe test meters behind RELAY_STRIPE_METERS, key from Secrets Manager, no prices. Live only after relay-deploy; Nick: repo creation, deploy, Stripe test key, Docker integration run.
  id: usage-metering
  asked: 2026-11-02
  by: claude-session

- [ ] **Try the node canvas** — ComfyUI/Node-RED style editor in Rust (egui, wasm): nested function blocks, live scan sim, not wired to the PLC. Kept as a lab tool (2026-10-29); per canvas-edit-scope the operator canvas is the read-only ST view (st-graph-view), which reuses tcpethernet-graph and this renderer. Serve it: `make canvas-web`, then `python3 -m http.server 8790 --bind 0.0.0.0 --directory target/canvas-dist` and open http://10.0.0.41:8790/ from the Air (stop: `kill $(lsof -tiTCP:8790 -sTCP:LISTEN)`). In it: hold a start button, double-click a motor to go inside, select nodes + Cmd+G to make a block, `?` lists the controls.
  needs: your eyes on the UI
  result: a list of changes, or none
  notes: `make canvas-serve` (trunk, live reload) only answers on localhost on the Studio: macOS's firewall holds the unsigned trunk binary's incoming connections behind a GUI prompt; python3 is allowed
  est: 15m
  difficulty: low
  id: canvas-try

- [x] **helmsman-iec no_std + alloc** — a `std` feature (default on): HashMap/HashSet → BTreeMap/BTreeSet, Arc/Duration/Error/fmt from alloc and core, the parsed library passed in instead of a OnceLock, `Program::scan` behind `std`, `State` carrying a Duration instead of an Instant, and the twelve REAL builtins (SQRT, LN, SIN, ..., round/trunc) from `libm` as an optional dep only the no_std build enables (they have no core counterpart on rustc 1.99; checked). No logic moves, only imports.
  needs: nothing new; docs/EDGE.md "What the runtime needs from std today" is the item list
  result: G1 — `cargo build -p helmsman-iec --no-default-features --target thumbv7em-none-eabihf` in CI and scripts/lint.sh; every existing test, the ruSTy differential and the property tests green on std; the box build's dependency tree unchanged
  options: libm optional under not(std) | gate the REAL builtins behind std and refuse them on a node
  notes: docs/EDGE.md §Phases, Phase 0. Studio baseline 2026-11-01: trace_bench 57.8 µs/scan tracing off (M1 Max, release)
  est: 1d
  difficulty: low
  notes: done 2026-11-02 night, PR #188 (G1): std feature default on, BTreeMap, libm optional for no_std only, thumbv7em build in lint.sh and install-dev.sh. PR #189 (CI step) is a DRAFT for Nick: workflow edit.
  id: iec-no-std

- [x] **Scan bench on the ESP32-POE2** — `crates/helmsman-node` (not a default workspace member): supervictor's board/poe2.rs and Ethernet bring-up copied in, Embassy and esp-hal 1.0 as supervictor pins them, trace_bench and the batch-mixer starter (blocks concatenated) embedded and compiled at boot; min/mean/max/p99 per scan and esp-alloc peak after compile and after 10 000 scans over USB serial. On a WROVER repeat with the program heap in PSRAM.
  needs: the esp toolchain as supervictor pins it (espup, channel esp: on the Studio already); the POE2 on USB with ETHERNET UNPLUGGED (no isolation)
  notes: Nick 2026-11-02 (board): "Uh oh I think I triggered this already...anyway there's a PoE2 you can try out that could be programmed via the laptop-installed editor interface that I can test on." Interview 2026-11-02: the POE2 is on the Studio's USB (/dev/cu.usbserial-11220), Ethernet unplugged, and it is a WROVER (8 MB PSRAM), so G3 gets PSRAM as a second esp-alloc region for compile scratch. Flashing stays a hand-off (a scratch script Nick runs).
  status: ready
  result: G2 (< 5 ms per 4k-statement scan) and G3 (compile peak plus 10 000 scans fit the node's heap layout with ≥ 25 % headroom, SRAM-only figure recorded separately); numbers recorded either way, and a stop if either fails
  notes: docs/EDGE.md §Numbers, §Extrapolating, Phase 1. Add the counting-allocator heap harness as a second helmsman-iec example while here.
  est: 1d
  difficulty: med
  depends: iec-no-std
  notes: firmware done 2026-11-02 night, PR #193: crates/helmsman-node (outside the workspace, esp toolchain, make node-build), bench binary, PSRAM second round.
  notes: done 2026-11-03 on the board (#208 stack fix, #209/#210 numbers in docs/EDGE.md Phase 1): bench-4k 28.1 ms mean per scan (G2 fails, target < 5 ms, ~500x the Studio), the same with PSRAM; peak heap 198 KB of 229 KB internal (G3 fails on SRAM); the batch-mixer starter does not fit SRAM and its compile corrupts memory with PSRAM (an allocation of 0x3F40D781 bytes, a rodata address read as a length). Recorded and stopped, as the gate says; the follow-ups are in `node-resume`.
  id: node-scan-bench

- [ ] **Modbus TCP from the node** — an async twin of helmsman-io's `Transport` over embassy-net's TCP socket behind a feature, `driver::Error` without std::io::Error, DI read and DO write batched to two round trips per scan; 10 ms scan against the E1213 (or the simulator) with DO0-3 looped to DI0-3; comms-loss safe state on yank.
  needs: flash on USB with Ethernet unplugged, THEN unplug USB, THEN PoE; the E1213's comms-loss safe state configured; a UDP log sink on the Studio
  notes: Nick 2026-11-02 (board): do it now with my own judgement. After `node-scan-bench`; then the node's own API and site beacon (`fleet-console`'s node half).
  result: G4 (overruns < 1 % of scans over 10 min) and G5 (the block goes safe within its timeout on Ethernet loss)
  notes: docs/EDGE.md §Sync scan, async network, Phase 2
  est: 1d
  difficulty: med
  depends: node-scan-bench
  notes: firmware done 2026-11-02 night, PRs #197 (helmsman-modbus no_std codec; helmsman-io frames through it) and #199: 10 ms scan, FC2 read + FC15 write, chase program, UDP report.
  notes: run 2026-11-03, then PARKED by Nick: the Modbus loop works on the POE2 (lease, chase program on the E1213's DO-00..02); the UDP reports never arrive (the node never takes the Studio's ARP replies); the POE-ISO build (#211) links at 100 Mbit but gets no lease. Findings in docs/EDGE.md Phase 2; picked up again by `node-resume`. G4/G5 not measured (G4 fails anyway at 28 ms scans).
  status: blocked
  id: node-modbus
