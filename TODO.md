# TODO

Done cards were removed on 2026-11-06 (git history has them). Three sections up top: decisions only you can make, things only your hands can do, and what an agent builds next. The roadmap below is the long view.

## Open questions for Nick

- [ ] **Move Ctrl S, Ctrl F and Ctrl G off the browser's chords too?** #322 moved every canvas shortcut that a browser owns and kept six as the one exception: Ctrl S, Ctrl Z, Ctrl Shift Z, Ctrl F, Ctrl G, Ctrl Shift G. Every editor on the web takes these, and the browser's own default (save the page, find in the page) does no harm if it slips through. The question was left in the PR body and the handoff file and never reached the board. Recommended: keep them.
  needs: your call
  result: the exception in `hotkeys::BROWSER` stays, or the six move (a one-line change per chord, the test `no_shortcut_is_a_browsers` then covers them)
  options: keep them (recommended) | move all six | move Ctrl S only
  est: 5m
  difficulty: low
  id: hotkeys-editor-chords
  asked: 2026-11-10
  by: claude-session

## Needs your hands

- [ ] **Promote dev to master: 280 commits, the guided tour among them** — the last dev → master merge was #246 on 2026-11-06. #248 (the playground tour) landed one minute after it and #251 (the playground rebuilt with the tour) is merged into dev, so the only step left for /playground/?tour is the promotion. Everything since waits with it: the node (POE-ISO and P4), read-lock, Help mode, the hotkeys, the 2026-11-08 review fixes (#303 to #308). dev's last CI run is green (0d840f7, 2026-11-10).
  needs: your dev → master merge; `gh pr list --base dev` first (2026-11-10: none open, but fix/node-modbus-connect is in flight in a live session and will open one)
  result: helmsman.advin.io/playground/?tour runs the tour and the site matches dev
  est: 15m
  difficulty: low
  id: tour-live
  asked: 2026-11-06
  by: claude-session

- [ ] **Log in at the Studio to bring the try box back, then rebuild it** — the try box has been down since the desktop session logged out at 17:06 CST on 2026-11-10: no helmsman-plc process, nothing on :8081, `/dev/console` belongs to root (checked 19:47). `launchctl kickstart` from SSH answers 125 until someone logs in at the Studio's screen; RunAtLoad then starts the box by itself. It comes back on the build of 2026-11-08 (e7f1ae2, #300), 52 merges behind dev, so rebuild after.
  needs: a login at the Studio's own screen (not SSH); then `~/github/helmsman-try/scratch/16-rebuild.sh` from your terminal there (it builds at the dev tip, signs and kickstarts the LaunchAgent)
  result: https://10.0.0.41:8081/ answers at the dev tip with the E1213 up
  notes: the box takes the E1213's one Class 1 connection as soon as it starts, so a P4 `enip` run (card enip-task-bench-checks) has to happen before the login or with the box stopped (`launchctl bootout gui/501/io.advin.helmsman-try`). Check after: `curl -sk -H "Authorization: Bearer $(cat ~/github/helmsman-try/scratch/try-state/api-token)" https://127.0.0.1:8081/api/devices` shows iologik_e1213 up.
  est: 10m
  difficulty: low
  id: try-box-login
  asked: 2026-11-10
  by: claude-session

- [ ] **Finish the relay's OpenRouter hand-off (agent-relay PR #8)** — stopped on 2026-11-09 waiting for you, and nothing has moved since (checked 2026-11-10: #8 is still a draft, the three new cards are still an uncommitted change to TODO.md in ~/github/agent-relay/.claude/worktrees/openrouter because worktree-guard refused that session's commit twice). The helmsman side is done (#321 merged; boxes keep asking for v7). Verified live on 2026-11-09 with your OpenRouter key: test-live passes, eval-live 17 of 18, a real box ran two turns through the local relay.
  needs: in this order: (1) `! ~/github/agent-relay/scratch/22-push-openrouter-todo.sh` commits and pushes the three cards; (2) read #8's template.yaml (the parameters and the ReadUpstreamKey Sid), mark it ready and merge it; (3) your dev → main merge there; (4) `~/github/agent-relay/scratch/20-put-openrouter-key.sh`; (5) `~/github/agent-relay/scratch/21-deploy-relay-openrouter.sh`
  result: the deployed relay answers on OpenRouter (DeepSeek V4.1 Flash by default) and the model is a deployment setting
  notes: details in ~/github/agent-relay/.claude/handoff.md and the PR #8 comment. Seen on the way and not touched: that repo's main checkout sits on feat/agent-v7-strict-schemas, and three sibling worktrees (agent-relay-deploy, agent-relay-harness, agent-relay-tests) live outside .claude/worktrees/.
  est: 30m
  difficulty: low
  id: relay-openrouter-handoff
  asked: 2026-11-10
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
  notes: parked: the Pi is out of the loop (Nick, 2026-11-07); only when Nick asks for the Pi. connect from the Studio: `ssh -L 8080:127.0.0.1:8080 nick@192.168.3.20`, then http://127.0.0.1:8080/ ; from the Air: `ssh -J nick@10.0.0.41 -L 8080:127.0.0.1:8080 nick@192.168.3.20`, same URL. Not on projects yet: `scripts/pi-deploy-plc.sh nick@192.168.3.20` (projects is its default; RESTARTS helmsman-plc; keep PLC_API as the unit has it), then `scripts/pi-account.sh nick@192.168.3.20 nick engineer`.
  est: 15m
  difficulty: low
  status: blocked
  id: pi-ui-demo
  asked: 2026-10-31
  by: claude-session

- [ ] **Check the board backend on the Pi** — run the GPIO / I2C check against the pick PC: finds the header's GPIO chip (gpiochip0 or gpiochip4 on a Pi 5, by kernel), lists /dev/i2c-1, builds and runs `board_check` (helmsman-io's Linux backend, the code behind `helmsman-plc --gpio-chip`) reading `di:4` by default. Add `READ="di:4 i2c:1:0x48:0:i16"` for a sensor, and `LOOP="do:17=di:27"` only with a jumper from that output to that input (it drives the output).
  needs: on the Pi `sudo apt install gpiod i2c-tools`, `dtparam=i2c_arm=on` and a reboot, the SSH user in the gpio and i2c groups; Docker here
  result: the board backend proven on real GPIO (and I2C if a sensor is wired); then `--gpio-chip` goes into the deploy unit
  notes: parked: the Pi is out of the loop (Nick, 2026-11-07); only when Nick asks for the Pi. `! scripts/pi-gpio-check.sh nick@192.168.3.20` (runs here, not in the native terminal; read-only unless LOOP is set)
  est: 30m
  difficulty: low
  status: blocked
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

- [ ] **Try the node canvas** — ComfyUI/Node-RED style editor in Rust (egui, wasm): nested function blocks, live scan sim, not wired to the PLC. Kept as a lab tool (2026-10-29); per canvas-edit-scope the operator canvas is the read-only ST view (st-graph-view), which reuses tcpethernet-graph and this renderer. Serve it: `make canvas-web`, then `python3 -m http.server 8790 --bind 0.0.0.0 --directory target/canvas-dist` and open http://10.0.0.41:8790/ from the Air (stop: `kill $(lsof -tiTCP:8790 -sTCP:LISTEN)`). In it: hold a start button, double-click a motor to go inside, select nodes + Cmd+G to make a block, `?` lists the controls.
  needs: your eyes on the UI
  result: a list of changes, or none
  notes: `make canvas-serve` (trunk, live reload) only answers on localhost on the Studio: macOS's firewall holds the unsigned trunk binary's incoming connections behind a GUI prompt; python3 is allowed
  est: 15m
  difficulty: low
  id: canvas-try

- [ ] **Prune the merged branches and drop the October stash** — 2026-11-10: 16 local branches and 4 on origin (docs/relay-model-api, fix/hotkeys-browser-free, feat/code-wrap-toggle, docs/help-get-help) are all in origin/dev, each checked with `git merge-base --is-ancestor`. The stray docs/studio-runs-plc of 2026-11-07 is already gone. `stash@{0}` (2026-10-31, "WIP on dev: 0ad1686 ... #70") holds only `answer:` lines for board questions that were acted on long ago (relay-repo, dev-merge-safety, logix-point-syntax and their neighbours). Branch and stash deletion is always a hand-off.
  needs: `! ~/github/helmsman/scripts/prune-branches.sh` (`DRY_RUN=1` first lists the deletions; it keeps the live fix/node-modbus-connect worktree and the locked try box checkout), then `git stash drop 'stash@{0}'` if you agree it is spent (`git stash show -p 'stash@{0}'` shows it)
  result: `git branch -a` shows dev, master and branches with work in flight; `git stash list` is empty
  notes: untracked leftovers nobody needs any more, yours to delete: scratch/11-try-box-relay.sh.bak, scratch/claude-md-panel-width-rule.patch (landed as docs/panel-width-rule), scratch/todo-board-answers-20261107-uncommitted.patch and scratch/board-answers-20261108.patch with scratch/TODO-board-answers-20261108.md (consumed by #293), and ~/github/helmsman-try/SCRIPT_OUTPUT.md (a pasted rebuild log of 2026-11-07).
  est: 5m
  difficulty: low
  id: prune-merged-20261110
  asked: 2026-11-10
  by: claude-session

- [ ] **Bring up the Murrelektronik EtherNet/IP I/O block** — placeholder until the unit is on the bench (Nick offered it 2026-11-10). A second vendor's Class 1 target beside the E1213: every implicit-messaging finding so far is against one Moxa unit on firmware V3.1 and OpENer. Bring it up from its EDS alone (`from_eds`, the Devices tab import) with no hand-written profile, as the first device nobody mapped by hand; record what its Forward_Open accepts (Exclusive Owner, Input Only, how many connections at once) in docs/devices/ with the manual's citations; add its folder to the device library. With a second unit the P4 node and the try box each own one and stop contending for the E1213's single connection. Model, I/O count and whether it is an IO-Link master are unknown until the part number is in.
  needs: the block on 192.168.3.x with 24 VDC, and its part number (for the EDS and the manual)
  result: a second device doc, map and library entry; the EDS import proven on a device it was not written against; two EtherNet/IP owners on the bench at once
  est: 1d
  difficulty: med
  status: blocked
  id: murr-block-bringup
  asked: 2026-11-10
  by: claude-session

- [ ] **Bring up the Novanta IMS Liberty MDrive** — placeholder until the unit is on the bench (Nick offered it 2026-11-10). The first device that is not discrete I/O: position and velocity as 32-bit values, a status and fault word, and a comms-loss state that matters. Read its identity, map its assemblies or registers, then run a jog and a move to position from ST with the fault path and the comms-loss stop tested; the demo it gives is an axis moving from a spec. Which protocols this unit speaks (EtherNet/IP, Modbus TCP or both) is not checked: it depends on the part number. Motion asks "cell clear?" once per session (hardware-guard).
  needs: the MDrive on 192.168.3.x with its DC supply and a free shaft, and its part number (for the EDS or register map and the manual)
  result: a device doc with citations, a map and a library entry; an ST program that jogs and positions the axis and stops it on comms loss
  est: 2d
  difficulty: med
  status: blocked
  id: mdrive-bringup
  asked: 2026-11-10
  by: claude-session

## Next to build

- [ ] **Canvas: match box answers to requests, one buffer state machine** — from the 2026-11-08 interface review. Every wasm request to the box is a parallel fetch with no request id or generation; `BoxProjects::answer` matches by call name and reads live buffer state at answer time, and the native tests run requests one at a time so ordering is never exercised. The data-loss fixes of PR fix/box-projects-correlation patch the cases found (NewFile empty buffer, dropped save answers, deploy during save, Replace target); this card is the shape that stops the class: an in-flight table keyed by request id holding the sent text and the project generation, answers resolved against it, and one `Buf` state enum (Clean, Dirty, Saving{sent}, Conflict) shared by save, rename, create and deploy, with a test harness that answers requests out of order.
  result: a reordered or late answer can never make a buffer clean, empty or stuck; box_projects_tests has an out-of-order answer suite
  est: 2d
  difficulty: high
  id: canvas-request-correlation
  asked: 2026-11-08
  by: claude-session

- [ ] **Canvas: pump the live source every frame, independent of the screen** — review 2026-11-08. `BoxView::tick` runs only from the Editor and Monitor branches, so each new screen (Proposals, "Simulate in this browser", a future one) has to remember to drain the transport or the inboxes grow and the link goes stale. fix/remote-link adds the tick and coalesces events; this card moves the design: App owns one per-frame pump for the live source, screens only render, and the transport inbox keeps latest-per-(stream, module) by construction.
  result: no screen can leak or stall the box link; a test opens every screen and asserts the link stays current
  est: 1d
  difficulty: med
  id: canvas-pump-every-frame
  asked: 2026-11-08
  by: claude-session

- [ ] **Box streams carry a boot id and a sequence; the canvas reconnects by reason** — review 2026-11-08. The canvas compares times across the box scheduler clock, the trace scan counter, egui time and history fetches, and `Msg::Failed` carries no reason, so a deliberate 600 s stream end, a 503 slot refusal, a 401 and a dead network all look like "lost the box". fix/remote-link patches the worst cases (immediate reconnect after a healthy stream, a values watchdog, trend reset on clock reset). The shape: the values stream and `/api/modules` carry a boot id and a monotonic seq; `BoxState` keeps one epoch and per-stream last-seen marks from it; reconnection learns the status with a fetch probe, backs off with jitter, and resumes from seq.
  notes: the box API change is small (api/mod.rs values event, http.rs)
  result: a box restart, a slot refusal and a half-open socket each show as what they are, and the trend, Monitor and diagram never mix epochs
  est: 2d
  difficulty: med
  id: box-stream-epoch
  asked: 2026-11-08
  by: claude-session

- [ ] **Agent acceptance needs a signal the model does not write** — review 2026-11-08. The model writes the code and the scenarios that accept it, and the checker parses prose and Markdown (fences, `Why:`, `replaces`, an elision phrase list). fix/verify-gate closes the holes found (asserts must be exercised, red-before/green-after, a replacement needs the old test red, truncated answers fail). The shape beyond that: structured output from the model (a tool or JSON schema with why, file_text, scenarios[{name, requirement, source, replaces?}]) so the prose parse and the elision heuristics go away; mutation checks (the scenario must fail when the changed output is inverted); and edits anchored to a base hash instead of whole-file rewrites as the trust unit, with the diff shown before the write.
  needs: a decision on structured output for the relay profiles (agent-relay repo)
  result: a vacuous or truncated answer cannot be written; the engineer sees a diff, not a sentence
  est: 3d
  difficulty: high
  id: agent-acceptance-signal
  asked: 2026-11-08
  by: claude-session

- [ ] **One agent core for the page and the box; apply and undo as a transaction** — review 2026-11-08. `playground_view` and `box_view` each implement Run, Check, Apply and Undo with different settings and sizes, message caps, pre-save of dirty edits, undo revision source and accepted stop reasons, and they have drifted. `Run` has no base snapshot, file set or per-file hashes, so Undo restores the whole project (other files' later saves included) and the box path overwrites unsaved local edits. Shape: one `AgentRun` owning Run/Check/Apply/Undo; hosts implement `snapshot()`, `settings()`, an atomic multi-file `write_files(base, files)` and `restore(files)`; Apply refuses if a touched file moved since Send; Undo is the exact inverse over those files only.
  result: one code path, tested once against both hosts; Undo never touches a file the Agent did not write
  est: 2d
  difficulty: high
  id: agent-run-core
  asked: 2026-11-08
  by: claude-session

- [ ] **The page simulation and the sim project store share the box's rules** — review 2026-11-08. The page is a second runtime that drifted from `helmsman-plc` runtime.rs (cold deploy vs state-carrying hot swap, stop and fault leaving outputs vs zeroing, a 500-scan catch-up burst vs skipped periods, a fixed 10 ms vs the declared period, a 64 KB source cut vs the project limits), and `SimProjects` is a second project store that drifted from the box API (case rules, import overwrite, history keyed by name). fix/sim-semantics and fix/desk-and-sim-store patch the cases found. Shape: a shared library function for deploy, swap, stop and fault semantics both runtimes call, one project-rules trait both stores implement, and one conformance suite run against both.
  result: "works in the page, differs on the box" stops being a bug class; the conformance suite is the contract
  est: 3d
  difficulty: high
  id: sim-shares-box-rules
  asked: 2026-11-08
  by: claude-session

- [ ] **A Transport trait with a native in-memory implementation; web.rs tested** — review 2026-11-08. web.rs (about 800 lines of wasm glue) has no test coverage: three copies of the stream-read loop, the same char-boundary `String::truncate` panic twice, `get()` without no-store or a timeout, inconsistent error handling. Shape: one `Transport` trait implemented by web.rs and by a native in-memory transport that can inject stalls, 500s, non-UTF-8 and oversized bodies and reordered answers; one shared stream-reader helper; remote.rs's retry and silence logic then runs under `cargo test`.
  result: every wasm-only path has a native test; a transport bug is a failing test, not a field report
  est: 2d
  difficulty: med
  id: canvas-transport-trait
  asked: 2026-11-08
  by: claude-session

- [ ] **Canvas: hit keys as an enum, Chrome split, one Command dispatch** — review 2026-11-08. `Chrome` is the persisted layout format (a hand-rolled k=v string), the UI toggles, the test registry (`hits: Vec<(String, Rect)>`) and a cross-frame channel at once; the `std::mem::take(&mut self.chrome)` pattern hands a default Chrome to anything that reads it meanwhile. Hit keys are free strings the tests and the tour depend on, so a rename silently breaks them. Hotkeys, header glyphs and menu items are three dispatch paths into `App::act` with scattered screen and source guards (fix/shell-hotkeys-layout adds the missing gates). Shape: a versioned `Layout` (persisted), a `FrameOut` (hits, diagram rect), a `HitKey` enum, and one `Command` enum with `enabled(&App) -> Result<(), why>` that menus, glyphs and hotkeys all consult; a tour test that visits every step and asserts its anchor was drawn.
  result: a removed or renamed control is a compile error; one place says whether an action is allowed and why
  est: 2d
  difficulty: med
  id: canvas-chrome-split
  asked: 2026-11-08
  by: claude-session

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
  notes: agent-relay PR #7 merged on 2026-11-07. Whether a deployed relay serves helmsman/v8 is not checked (the deployed one serves v7; relay card relay-deploy-v2).

- [ ] **Modbus identification probe (FC 43/14) for discovery and identity** — left out by the device-by-identity and discovery workers (2026-11-02 night): `cip=` identities and the ListIdentity sweep cover EtherNet/IP; a Modbus-only device is found today only by `mac=` once the host has seen it. Add Read Device Identification (FC 43, MEI type 14: VendorName, ProductCode, MajorMinorRevision) to helmsman-io's modbus driver, a bounded probe of the ARP table's hosts on 502 in `POST /api/devices/discover` when asked (`?modbus=1`), a `modbus=VENDOR:PRODUCT` identity endpoint resolved by that probe, and the library lookup by it (`identity.modbus` in device.toml exists). Cite the Modbus Application Protocol spec section in docs/SOURCES.md.
  needs: a Modbus device that answers FC 43/14 to test on (does the E1213? check on the bench; the simulator answers it either way)
  result: a Modbus-only block is found and matched like an EtherNet/IP one
  est: 1d
  difficulty: med
  id: modbus-identification
  asked: 2026-11-02
  by: claude-session
  notes: box side merged in #219 (FC 43/14 codec and probe, modbus= endpoints, discover ?modbus=1, spec §6.21). Left: the Devices tab asking ?modbus=1 and showing the list; identity.modbus in helmsman-devices build-index.py; a Modbus-identity query on GET /api/devices/library; whether the E1213 answers FC 43/14 (unchecked: it was off the LAN 2026-11-04).

- [ ] **Help mode on the relay: a profile that reads the repo** — in ../agent-relay (separate repo and session; SAM deploy, deploy-chain edits as a draft PR): a new profile id (e.g. helmsman/help-v1; a served profile never changes) whose model gets read-only tools over JimothyJohn/helmsman@dev through the GitHub API: list a directory, read a file (size-capped), search code. The repo is PUBLIC (checked 2026-11-07), so no token is needed to start; a fine-grained read-only token in Secrets Manager raises the rate limit (60/h unauthenticated). The system prompt: diagnose for a third-party user, cite the files it read, end with the Markdown issue; docs/ARCHITECTURE.md and docs/help are the starting points. Tests against recorded GitHub responses; refuse paths outside the repo.
  needs: a session in ../agent-relay (this repo's sessions cannot write there); Nick's go for the deploy
  result: the relay serves the Help profile the box's Help mode asks for
  est: 2d
  difficulty: med
  id: help-mode-relay
  asked: 2026-11-07
  by: claude-session

- [ ] **Show the checker's warnings in the canvas and the check API** — #288 added the first non-blocking warnings (RETAIN where it cannot be kept yet: in a function block, a FUNCTION, VAR_TEMP, a located variable), as `Program::warnings()`. Today the box only logs them at start. Show them where errors show (the editor's marks, the Messages, `POST .../check`, the CLI `helmsman-plc test`), visibly different from errors, and never blocking a deploy. The ST lints decision (#272: all shown, two block) will use the same path.
  result: an engineer sees "RETAIN here is not kept yet" while typing, not only in a log
  est: 4h
  difficulty: low
  id: checker-warnings-shown
  asked: 2026-11-08
  by: claude-session

- [ ] **The desk is per person on a shared browser too** — after #280, when a second person signs in on a browser the first one used, and the box has nothing newer for them, the desk on screen (the first person's unsaved edits) is written to the second person's box desk. Slice 1 (#276) already shared the browser desk between everyone on one browser. Key the browser desk by the signed-in name, and on a sign-in by a different person start from that person's box desk or an empty one, never the previous person's.
  result: one person's draft never lands in another's desk
  est: 2h
  difficulty: low
  id: desk-person-switch
  asked: 2026-11-08
  by: claude-session

- [ ] **A project keeps the files this build does not know** — found by project-compat-fixtures (#283): reading a project skips a file whose name this build does not recognise without a word, a replace-upload then loses it, and an uploaded .zip holding one is refused outright. List such a file as a problem ("not used by this release: kept"), keep it through reads and uploads, and add it to the compat fixtures.
  result: a newer release's file survives a trip through an older box
  est: 4h
  difficulty: low
  id: project-unknown-files
  asked: 2026-11-08
  by: claude-session

- [ ] **A broken diagram.layout does not block the build** — found by #283: a layout that will not parse stops the build although the build never reads it, and its problem has no file and line of its own. Make it a listed problem on `diagram.layout` at its line, fall back to the automatic layout, and build.
  result: a bad layout costs the drawing, not the deploy
  est: 2h
  difficulty: low
  id: layout-not-blocking
  asked: 2026-11-08
  by: claude-session

- [ ] **A gateway's "unit not answering" marks the device down** — after #290, a Modbus TCP gateway that answers for a dead serial unit with exception 0x0A (gateway path unavailable) or 0x0B (target device failed to respond) counts as a point error, so the device stays "up" with every point bad. Treat those two as link errors for that unit only (the connection and the other units stay up), with the device's reason in plain words. Cite the Modbus Application Protocol spec's exception table in docs/SOURCES.md.
  result: a dead RTU behind a gateway shows as down, with why
  est: 2h
  difficulty: low
  id: gateway-unit-down
  asked: 2026-11-08
  by: claude-session

- [ ] **`build-help.sh --check` is not in CI** — the help pages were rebuilt with the Load a program page (2026-11-08), so they are current again, but only `make help-check` runs the check: nothing in .github/workflows does, which is how simulation.html drifted after #288. Add it to the CI gate (a workflow edit: draft PR).
  result: drift between docs/help/*.md and the committed .html fails CI
  est: 30m
  difficulty: low
  id: help-html-drift
  asked: 2026-11-08
  by: claude-session

- [ ] **Node firmware OTA: A/B slots, pull from a host, roll back on an unconfirmed boot** — #312 put program deploy over the node's API (PUT /api/modules/NAME with the bearer token); the firmware itself still goes over USB (scripts/node-flash.sh). supervictor's ota/{install,policy} already has the shape to reuse: the esp-bootloader-esp-idf partition table with two app slots, write the new image to the idle slot, mark it, reboot, and confirm it from the running image once the API answers; an image that never confirms rolls back on the next boot. The node pulls the image from a URL the canvas or a curl gives it (POST /api/firmware {url, sha256}) so the host serves, the node fetches; the token guards it like every write. Same code on the POE-ISO and the P4 (helmsman-node-p4). Test: a host-side test against the simulated flash (nodecore) for the slot state machine, and a bench run that flashes a build, confirms, then a deliberately broken build that rolls back.
  needs: the P4 is not required; the POE-ISO on the bench (poe-iso-rack-03, /dev/cu.usbserial-1130, 192.168.3.55) is enough
  result: a node on the plant network takes a new firmware from a laptop without a USB cable, and a bad image cannot brick it
  notes: sized 2026-11-09 (not started, needs the bench): (1) a new partition table, nvs / otadata / phy_init / ota_0 / ota_1 on supervictor/edge/partitions.csv's model, one USB flash per board (espflash --partition-table, --erase-parts otadata; it moves the nvs region the settings and program live in, so the node is commissioned again); (2) port supervictor/edge/src/ota/{install,policy,http,manifest}.rs into nodecore (Slot trait installer with sector buffering, digest and read-back; the checksummed RTC record with MAX_PENDING_BOOTS; the strict response-head parser; 2.4k lines with their proptests, tied to supervictor_common::firmware types to replace); (3) the fetch task on embassy-net (Range + If-Match) writing through esp-bootloader-esp-idf 0.5's OtaUpdater (next_partition, activate_next_partition, set_current_ota_state), the boot policy at start, GET /api/firmware, POST /api/firmware {url, sha256}, POST /api/firmware/confirm; (4) on the POE-ISO: a push, a confirm, a forced rollback.
  est: 2d
  difficulty: high
  id: node-ota
  asked: 2026-11-09
  by: claude-session

- [ ] **P4 node, after the bring-up: EtherNet/IP against the E1213, the PoE module, TLS** — the node image runs on the Waveshare ESP32-P4-ETH as on the POE-ISO since 2026-11-10 (docs/EDGE.md Phase 3 "On the board": press-5 at 192.168.3.56, token bench-token-2026, chase at 10 ms with 70 us of execution, a 431-line cell at 1 ms). What is left: (1) settled 2026-11-10: the E1213 (firmware V3.1) holds one Class 1 connection at a time of either kind, so Input Only beside the try box is refused with a bare 0x01 and the node's request is right as it is (docs/devices/moxa-iologik-e1213.md); the `enip` image then ran both ways with the unit free (Input Only and Exclusive Owner, no lost packets, the chase on the outputs over CIP; docs/EDGE.md); a bare 0x01 now prints with that explanation (`Refused`'s text); the node's Modbus link no longer comes up late at boot (2026-11-10: the I/O task waits for the link and retries a failed connect quickly before backing off; run the POE-ISO once with it when that board is back on the network); (2) whether firmware 4.0 lifts the limit (the EDS v2.0 describes it; the unit runs 3.1); (3) the PoE module's isolation measured before USB and PoE are ever together, then a run on PoE alone; (4) and (5) done 2026-11-10 (the panic restart seen, the PSRAM in `/api/node`, a cell's data in PSRAM costing nothing visible at a 1 ms scan; docs/EDGE.md); (6) mbedtls-rs at the fork's main with Homebrew clang for TLS; (7) drop `ethernet::CutThroughTx` when an esp-hal release ties transmit store-and-forward to checksum offload.
  needs: the PoE module on the board for (3); the try box off the E1213 (stopped, or its link pointed at Modbus) for (1)
  result: the P4 node owns the E1213 over EtherNet/IP, runs on PoE alone, and its memory numbers say what a scan in PSRAM costs
  est: 1d
  difficulty: med
  notes: flash from a session: `NODE_BOARD=p4-eth NODE_P4_NO_POE=1 scripts/node-flash.sh node` (port /dev/cu.usbmodem5C930154551; espflash is 4.7.0 since 2026-11-10, 3.3.0 could not talk to this chip revision; the POE-ISO answers 4.7's board-info, a flash of it with 4.7 is not yet tried). A program over the flash slot runs but is not kept (12 KB did not fit): the larger slot comes with node-ota's partition table.
  id: node-p4-followups
  asked: 2026-11-10
  by: claude-session

- [ ] **Monitor: show the EtherNet/IP adapter's address beside the devices** — the half of `read-lock` (decided 2026-11-08) that did not ship with it: the adapter stays on at boot, so the Monitor's Devices tab should say where it listens instead of staying silent about it. The API does not know the adapter's bind today (`api::Config.bind` is the API's own; main.rs starts the adapter from `o.server.bind` and `encap_port`). Shape: `api::Extras` carries the adapter's socket address (`None` when off), a new `GET /api/adapter` answers `{protocol: "EtherNet/IP", bind, on}` behind a session like every read, the canvas polls it with the devices (remote.rs `Fetch`) and the Devices tab shows one wrapped line under the tree ("EtherNet/IP adapter on 0.0.0.0:44818: other controllers read this box here"). Tests: the route in api.rs, a remote.rs parse test, and the Devices tab at the panel's default width (CLAUDE.md panel rule).
  needs: nothing
  result: a buyer's IT sees the one listener that stays open, named, where they look for devices
  est: 3h
  difficulty: low
  id: cip-adapter-visible
  asked: 2026-11-09
  by: claude-session

- [ ] **The P4's `enip` image after #333: the two runs that were skipped** — #333 moved the EtherNet/IP session into a task of its own and was checked on the board for the first open only (1.1 s after link-up, 0 late scans, was 199). Two runs were left out and are on no card: (1) the close after a lost link, through the task: pull the cable or power the E1213 off mid-run, see the outputs go to their safe state, the session close, and the reopen once the unit is back, with no late scans meanwhile; (2) Exclusive Owner with this change (the 640 of 640 packet run of 2026-11-10 was on the image before it). Write both into docs/EDGE.md Phase 3 "On the board".
  needs: the E1213's one Class 1 connection free: the try box down (it is, until the Studio login of card try-box-login) or stopped; no other session flashing the P4 (2026-11-10 19:47: a live session was flashing it from the worktree modbus-connect for the Modbus connect fix of node-p4-followups, so start after its PR lands)
  result: the enip task is proven for loss and for ownership, not only for the first open
  notes: flash: `NODE_BOARD=p4-eth NODE_P4_NO_POE=1 SERIAL_PORT=/dev/cu.usbmodem5C930154551 scripts/node-flash.sh enip`, then back to the `node` image with `NODE_SEED_CHASE=1`. Read the outputs back over Modbus with `/usr/bin/python3` (Local Network privacy).
  est: 2h
  difficulty: low
  id: enip-task-bench-checks
  asked: 2026-11-10
  by: claude-session

## The ST plan (INVESTIGATION.md, decided 2026-11-07 and 2026-11-08)

The investigation's "Suggested order" as Nick agreed it (`st-plan-order`), with his calls folded in: lints shown, two block, a pragma overrides (§1, 2026-11-07); the §4 dialect table with VAR_GLOBAL inside one module only; OSCAT or standard names for the library with plain words in the tooltips; no LSP or formatter until `version-control` ships; no SCL for the early release; IEC 61131-3 cited from secondary references marked as such (docs/SOURCES.md). Done already: the ruSTy 1.0.8 bump (#282) and `helmsman-plc test` with JUnit XML (#289). Each dialect "yes" adds oracle cases, property tests, the no_std build check and a new relay profile id (a served profile never changes).

- [ ] **ST corpus test and refusal ranking** — `tests/st_corpus.rs` in helmsman-iec plus `scripts/st-corpus.sh`, the `plcopen-corpus.sh` pattern: fetch real ST by pinned commit and SHA-256 into `target/st-corpus/`, never vendor it, and report per file whether it lexes, parses, checks and compiles, with a total and the refusals ranked by cause ("STRING: n files, VAR_IN_OUT: m, METHOD: k, ..."). Sources, in INVESTIGATION.md §3: `WengerAG/structured-text-utilities` (MIT) and `tkucic/UniTest` (MIT) can be fetched freely; the test directories of ruSTy (LGPL), matiec (GPL), iec-checker (LGPL) and blark (GPL-2.0) are fetch-by-hash only; OSCAT BASIC only after its license and download terms are read (unverified: skip it if that takes more than a look). Each source gets a docs/SOURCES.md entry under "Test corpora". The ranking is printed by the test and pasted into INVESTIGATION.md §3 so the dialect cards below can be reordered by it.
  result: the order of the dialect work is measured, not guessed
  est: 1d
  difficulty: low
  id: st-corpus
  asked: 2026-11-08
  by: claude-session

- [ ] **PLCopen guideline lints in the checker, the first 6-8** — in `crates/helmsman-iec/src/check.rs` as `Diagnostic`s with stable IDs (`PLCOPEN-CP<n>` where the guideline has a number, `HELMSMAN-L<n>` otherwise), on the warnings path `checker-warnings-shown` builds, fed into the agent's retry message in `verify::next` beside compile errors. The picks (INVESTIGATION.md §1): an FB instance called inside a conditional (the TON-inside-IF bug), an output written in more than one place, a variable read before it is assigned with no initial value, `=` or `<>` on REAL, a FOR variable or its bounds changed in the body, unused variables and unreachable code, recursion. Decided (Nick, 2026-11-07): every lint is shown and goes to the agent; the first two block "Use this code" and Deploy the way a red requirement test does; a blocking lint is overridden on its line by a pragma with a reason (`{lint: off CP-x "imported, the branch is the enable"}`: the lexer skips `{ }` pragmas today, so it has to keep this one), and the override is written to the audit trail with the reason. The others stay advisory until `st-corpus` shows how noisy they are. Source: the PLCopen Coding Guidelines v1.0 PDF, a free download from plcopen.org (checked 2026-11-08, no login: `https://www.plcopen.org/download_file/force/ff60e817-eee5-442d-b718-a357a7279c7e/342/`); a docs/SOURCES.md entry with the rule number and page per lint ID. iec-checker in docker as a dev-time cross-check (both tools flag the same corpus files) is optional.
  result: the agent's first retry already knows about the timer-in-IF and the double-written output; an engineer sees why a draft is held, and can override with a reason on record
  est: 3d
  difficulty: med
  depends: checker-warnings-shown
  id: st-lints
  asked: 2026-11-08
  by: claude-session

- [ ] **VAR_IN_OUT and VAR_OUTPUT on FUNCTION** — the two cheap "yes" rows of the §4 table. VAR_IN_OUT on a FUNCTION_BLOCK passes a struct or array by reference inside one module (the standard way every library takes a buffer); a FUNCTION gets VAR_OUTPUT beside its return value. Oracle cases in tests/oracle.rs (ruSTy accepts both), property tests, the no_std build, the playground SYSTEM prompt and a new relay profile id listing them. Refuse anything that would alias across modules (the isolation claim).
  result: imported FB libraries and agent-written helpers stop failing on the most common refusal
  est: 2d
  difficulty: med
  depends: st-corpus
  id: st-var-in-out
  asked: 2026-11-08
  by: claude-session

- [ ] **VAR_GLOBAL inside one module** — decided (Nick, 2026-11-08: `st-dialect-line`): VAR_GLOBAL is accepted at CONFIGURATION / RESOURCE scope inside one module and refused across modules, with a message that says signals are how modules talk. Imported CODESYS and Beremiz code uses it everywhere; the corpus will show how often. Oracle cases, property tests, the PLCopen importer's lowering of global variable lists, a relay profile id.
  result: a CODESYS or Beremiz project with globals imports into one module unchanged
  est: 1d
  difficulty: med
  depends: st-corpus
  id: st-var-global
  asked: 2026-11-08
  by: claude-session

- [ ] **STRING[n], then typed asserts** — fixed-length `STRING[n]` (default 80 as the standard), with concatenation, LEN, LEFT/RIGHT/MID, CONCAT, FIND, comparisons and the conversions, bounded so the scan stays bounded and the edge node still builds (no_std: no heap). WSTRING later. Then the TcUnit idea from §5: `ASSERT_EQ_<TYPE>(Expected, Actual, Message)` (one block per type, no generics) so a red requirement test reads "speed was 40, expected 50 at 3.2 s" in the Requirements tab and in the agent's retry message. Oracle cases, property tests (every length boundary, invalid UTF-8 refused at the lexer), relay profile id.
  result: messages, recipe names, device IDs and OPC UA / webhook payloads can be written in ST; a failed test says what it saw
  est: 4d
  difficulty: high
  depends: st-var-in-out
  id: st-string
  asked: 2026-11-08
  by: claude-session

- [ ] **Process-control library: 10-15 blocks with tests** — in `crates/helmsman-iec/src/library.st`, written here (OSCAT's license is unverified and it leans on pointers): `PID` with anti-windup and bumpless manual/auto transfer, `RAMP`, `HYST`, `SCALE` (raw to engineering units, using the signal's `unit`), `LIMIT_ALARM`, `FILTER_PT1`, `DEBOUNCE`, `PULSE_COUNT` and a rate, sequence helpers. Decided (Nick, 2026-11-08: `st-library-names`): OSCAT's pin and block names where they exist, the standard's otherwise, no aliases; the plain-words explanation of every block and pin goes in its tooltip and its help page, the way the UI rule "tooltips on everything" already works. Each block gets requirement tests of its own, the non-IEC ones are marked in the docs as `BLINK_PATTERN` is, and the agent's profile gets the list (a new id).
  result: an analog loop the agent writes calls a tested PID instead of a hand-rolled PI with its own windup bug
  est: 4d
  difficulty: med
  depends: st-var-in-out
  id: st-library
  asked: 2026-11-08
  by: claude-session

- [ ] **matiec as a second oracle** — the ruSTy pattern (docker/rusty, `HELMSMAN_RUSTY`, tests/oracle.rs) for `beremiz/matiec` (GPL-3.0, the compiler inside OpenPLC and Beremiz): `docker/matiec/`, `HELMSMAN_MATIEC`, a harness that wraps the generated C in a scan loop and prints the variables, and the oracle corpus run through both. Where helmsman and ruSTy disagree a third vote says which is wrong, and "your OpenPLC project runs here unchanged" becomes a tested claim (with the PLCopen corpus that exists). Executed only, never linked.
  result: a disagreement with ruSTy is settled by a vote, and OpenPLC agreement is measured
  est: 1d
  difficulty: med
  id: matiec-oracle
  asked: 2026-11-08
  by: claude-session

## From user feedback: plan for later

Long-term items FEEDBACK.md points to. Each waits on a card already on the board, or on a decision.

- [ ] **The agent writes tests for abnormal sequences by default** — engineers expect AI code to fail at edge cases and timing ("Bubba on 3rd shift can't be predicted"), and their condition for trusting it is that checking costs less than writing. Next relay profile: for every requirement the agent also proposes tests for a stuck sensor, both buttons at once, a timeout, a restart mid-sequence and comms loss, each as a plain-words requirement the engineer accepts or drops. A new profile id, since a served profile never changes.
  needs: the relay profile work; lints and these tests are the same retry loop (lint rule decided 2026-11-07, INVESTIGATION.md §1)
  result: a draft arrives with the abnormal cases already tested; the engineer reads requirements, not code
  est: 1d
  difficulty: med
  id: agent-abnormal-tests
  asked: 2026-11-07
  by: claude-session

- [ ] **Explain mode answers timing questions** — arpus4KM on PLCTalk wants to ask "How many milliseconds will this logic take to execute". With `scan-stats` the box knows each module's measured time, and the instruction budget gives a worst case. Let explain mode answer "how long does this take", "what is the longest path" and "when does this timer finish", with the numbers from the box.
  needs: scan-stats
  result: the timing questions engineers ask an AI get real numbers
  est: 2d
  difficulty: med
  depends: scan-stats
  id: explain-timing
  asked: 2026-11-07
  by: claude-session

- [ ] **One page that is the ST dialect** — every third-party tool (blark, iec-checker, UniTest, vscode-st, STweep) breaks on vendor dialects. A single, written description of what Helmsman's ST accepts and refuses, with the IEC section for each (once `iec-standard-source` lands), is useful to the agent, to an engineer importing code, and to anyone building a tool for it. Generate the "refused" list from the checker's messages so it cannot drift.
  needs: iec-standard-source answered
  result: docs/help/st-dialect, linked from every "not supported" message
  est: 4h
  difficulty: low
  id: dialect-page
  asked: 2026-11-07
  by: claude-session

- [ ] **A project pins the library version it was built with** — TwinCAT users report library mismatches that block going online "even with otherwise identical code", and OSCAT breaks on new CODESYS service packs. Once the process-control library (INVESTIGATION §6) exists, `library.st` will change between releases. A project records the library version it was approved with, keeps that behaviour until an engineer upgrades it as a proposal, and the diff shows which blocks changed.
  needs: the process-control library carded and started (`st-plan-order`)
  result: a project approved last year behaves the same after a box update
  est: 1d
  difficulty: med
  id: library-pin
  asked: 2026-11-07
  by: claude-session

- [ ] **Accounts as the default sign-off, PINs as the fallback** — the liability fear ("a tort lawyer will argue you don't know what you're doing") is answered by a named person signing off. With PINs only, the audit records a role ("engineer (PIN)"), not a person. Make first-run setup create a named engineer account, show the approver's name on every version in the history, and keep role PINs for sites that want them.
  needs: the first-boot flow (`box-sku`)
  result: every change in the audit trail names the person who approved it
  est: 1d
  difficulty: low
  id: named-signoff
  asked: 2026-11-07
  by: claude-session

- [ ] **TOD, DATE and DT** — the last "yes" row of INVESTIGATION.md §4: time-of-day and date types and their arithmetic, from the clock `scan_at` already has (the shift calendar uses it), so "after 22:00" can be written in ST. Oracle cases (ruSTy accepts them), property tests across midnight and the year end, no_std, a relay profile id. After `st-string`, which the formatting functions need.
  result: schedule logic is written in ST instead of worked around with the calendar
  est: 2d
  difficulty: med
  depends: st-string
  id: st-tod-date
  asked: 2026-11-08
  by: claude-session

- [ ] **TwinCAT .TcPOU / .TcDUT / .TcGVL import** — INVESTIGATION.md §8: XML wrapping ST in CDATA, so the ST parser does the work and the importer is small; the L5X pattern (convert, refuse what does not map, list every problem). Beckhoff code leans on METHOD / PROPERTY and VAR_GLOBAL, so wait for `st-corpus` to say how much of it would import before spending the day.
  result: a TwinCAT project's POUs open in the editor with every refusal listed
  est: 1d
  difficulty: low
  depends: st-corpus, st-var-global
  id: tcpou-import
  asked: 2026-11-08
  by: claude-session

- [ ] **A language server and a formatter on helmsman-iec** — decided (Nick, 2026-11-08: `st-edit-outside-canvas`): not yet. Until `version-control` puts a project in git the canvas is the only way into the code, so an LSP (diagnostics, hover types, go to definition, the lints) has no users, and the formatter needs a lossless token layer first (the lexer drops comments and folds case) and could only ever run as an explicit Format command (agent prompt Rule 2). Revisit once an engineer has cloned a project.
  result: VS Code, Neovim and Zed get Helmsman's dialect and lints; proposal diffs show only real changes
  est: 1w
  difficulty: high
  depends: version-control, st-lints
  id: st-lsp-formatter
  asked: 2026-11-08
  by: claude-session

- [ ] **Siemens SCL import** — decided (Nick, 2026-11-08: `st-scl-import`): not for the early release. SCL (TIA Portal) is an ST dialect (`#local`, `"DB".member`, REGION, IEC_TIMER instances, its own instructions); a source-level converter would follow the L5X pattern. Carded here so it is not lost: pick it up when a customer brings TIA code, and estimate then (likely 1-2 weeks).
  needs: a customer with TIA Portal code to import
  result: an SCL block converts to ST with every problem listed
  est: 2w
  difficulty: high
  status: blocked
  id: scl-import
  asked: 2026-11-08
  by: claude-session

- [ ] **Diagram edits 1: end positions in the parse tree** — first step of editing the program from the diagram (interview 2026-11-07: splice edits into the ST, ST stays the source of truth; V1 is rewire and values; sim and box through the normal Save; the ST updates live and the changed span flashes). `Expr` and `Stmt` carry only a start `Pos`; add `end: Pos` to both in helmsman-iec's parser (14 constructions in parser.rs, 1 in ladder/rung.rs; also check lower.rs and model.rs).
  result: every statement and expression knows its exact source range, so a diagram edit can replace just that text
  notes: property test: for every statement and expression in generated programs and the corpus, the source between start and end parses back to the same tree (ignoring positions). The tree drops comments and upper-cases names, which is why edits splice text instead of printing the tree.
  est: 4h
  difficulty: med
  id: diagram-edit-spans
  asked: 2026-11-07
  by: claude-session

- [ ] **Diagram edits 2: `helmsman_diagram::edit` with values, NOT and operators** — lowering records a source span per input pin (`(NodeId, inp)` → the operand's range; a read wired from an earlier write keeps the read's own text in this statement). New pure, wasm-safe `edit::apply(src, &Module, Edit) -> Result<Patch, Refusal>` (Patch = byte range + new text), depending only on helmsman-iec. First edits: SetConst (literals, TIME presets), ToggleNegation (the bubble on a pin or region gate), SwapOp (AND/OR, comparisons, + - * /).
  result: a diagram gesture becomes an exact, checked text edit; nothing outside the patch changes
  notes: brackets only where precedence needs them (`text::expr`'s levels); names written as declared, taken from the declaration's source text. Each edit is checked: splice, compile, re-lower, and `diff` must show exactly the intended block changed, else refused with the reason. Property tests over props.rs generators with comments mixed in: compiles, bytes outside the patch unchanged, the inverse edit restores the original bytes. `wasm_deps.rs` stays green.
  est: 1.5d
  difficulty: high
  depends: diagram-edit-spans
  id: diagram-edit-core
  asked: 2026-11-07
  by: claude-session

- [ ] **Diagram edits 3: rewire, FB arguments, delete, rename** — Rewire (a pin to a variable, a constant or an earlier write), SetFbArg (`PT := T#5s`), DeleteStmt (the whole statement by its range), Rename (reuses `rename_ident`).
  result: V1's full set of logic edits from the diagram
  notes: refusals, each in plain words: type mismatch (the compiled program's types, as the NOT bubble uses), a source not yet written at that point, library POUs (`system`), a diagram whose source no longer matches the buffer (hash), a keyword as a name. `rename_ident`'s `ident()` accepts keywords today (`IF` passes), so refuse that up front.
  est: 1d
  difficulty: med
  depends: diagram-edit-core
  id: diagram-edit-rewire
  asked: 2026-11-07
  by: claude-session

- [ ] **Diagram edits 4: editing on the canvas, simulation** — st_view.rs gestures: right-click a block or pin for Change operator ▸, Negate input, Connect to… (a searchable variable list, works on touch), Edit value…, Rename…, Delete statement; double-click a constant to edit it in place. Commit: push the old text into the same `TextEditState` undoer the editor uses (`add_undo`, its own entry), so Ctrl+Z undoes a diagram edit like typing; mark dirty; scroll the ST to the patch and highlight it about 1.5 s; rebuild the `Viewer` keeping the camera and the picked block.
  result: in the playground and browser projects an engineer changes logic on the diagram and the code follows
  notes: the edited block keeps its dragged position: the patch returns old key → new key for arrange.rs (blocks match by POU, label, text, ordinal, and the text changes). Tooltips on everything, 18 pt text, popups capped to the column; `every_tab_leaves_the_panels_their_width` stays green. Headless tests: gesture → buffer text, Undo restores it, the position carries over.
  est: 1.5d
  difficulty: high
  depends: diagram-edit-rewire
  id: diagram-edit-canvas
  asked: 2026-11-07
  by: claude-session

- [ ] **Diagram edits 5: on a box** — the same gestures on a box, on only for an engineer (`writable()`) and only while the diagram shows the open buffer, not the running module; otherwise off with a tooltip saying why. Nothing new on the box: Save and Deploy go through the existing policy, which sets the tier from the diff.
  result: diagram edits on a Pi land like typed edits, approval and audit unchanged
  notes: box_projects_tests: edit, Save, the stale-save 409, against the real API. A diagram edit runs on a box only after Deploy, the same as typing.
  est: 4h
  difficulty: low
  depends: diagram-edit-canvas
  id: diagram-edit-box
  asked: 2026-11-07
  by: claude-session

- [ ] **Diagram edits 6: docs** — amend `canvas-edit-scope` in docs/ARCHITECTURE.md (the live ST view section: the diagram now writes ST through checked splices), docs/UI.md, and a help page in the floor-engineer voice.
  result: the docs say what the diagram can change and how it reaches the code
  est: 2h
  difficulty: low
  depends: diagram-edit-box
  id: diagram-edit-docs
  asked: 2026-11-07
  by: claude-session

- [ ] **Diagram edits V2 and V3: add blocks, then regions** — V2: a palette to add a block, a new assignment or a new FB instance (that edits the VAR section too). V3: regions: wrap statements in an IF, edit a condition, add ELSIF or ELSE, add CASE arms.
  result: building new logic from the diagram, not only changing it
  notes: V2 about 3-4 days, V3 about 3 days; split into cards when V1 lands.
  est: 1w
  difficulty: high
  depends: diagram-edit-docs
  id: diagram-edit-v2-v3
  asked: 2026-11-07
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

- [ ] **Resume the edge node: faster scans and the POE2's PSRAM** — parked by Nick 2026-11-03 after the first runs on the boards (docs/EDGE.md Phase 1 and 2). Thread 1 (the network) is SOLVED in #312 (2026-11-08): the POE-ISO got no lease because helmsman-node enabled esp-hal's `psram` feature on the WROOM, whose PSRAM pins are the POE-ISO's RMII clock (now `--no-default-features --features poe-iso`); the Studio reached 10.0.0.x only over Wi-Fi (dual-homed, scratch/23 and 26 add the en0 aliases); and a dead unit's connect-every-scan starved smoltcp's neighbour cache (2 s back-off after a failed connect). Two threads stay: (2) scan speed: 28 ms for the 4k-statement bench against < 5 ms, and 30 ms with two synchronous Modbus round trips before the I/O task; measure flash at 80 MHz QIO (it boots at 40 MHz DIO), the interpreter's i128 `Value::Int` on a 32-bit core, the no_std BTreeMap lookups, esp-alloc heap-stats, before any rewrite (a compact compiled form or compile-on-laptop is the bigger lever); (3) the PSRAM corruption on the POE2: compiling the batch-mixer starter with PSRAM asks for 0x3F40D781 bytes (a rodata address read as a length); the POE2 at 10.0.0.50 still runs November's firmware.
  needs: Nick's call to resume (2026-11-06: stays parked); the P4 ran on 2026-11-10 (docs/EDGE.md Phase 3 "On the board": the chase in 70 us a scan, a 431-line cell in 1 ms), which may make both threads moot
  result: a node that scans a realistic program inside its period, or a firm no with numbers for each thread
  notes: the Studio's en0 has 10.0.0.241 and 192.168.127.10 (sudo ifconfig en0 -alias X undoes). Builds: scripts/node-build.sh [bench|modbus|node], NODE_BOARD=poe-iso|p4-eth; flash: scripts/node-flash.sh (the POE-ISO flashes from a session without a prompt).
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
