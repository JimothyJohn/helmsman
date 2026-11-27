# TODO

Done cards were removed on 2026-11-06 (git history has them). Three sections up top: decisions only you can make, things only your hands can do, and what an agent builds next. The roadmap below is the long view.

## Open questions for Nick

- [ ] **Add a "no video in git" line to the global CLAUDE.md?** — You said 2026-11-26 "I don't want to commit videos of any kind anywhere, they'll be stored on a file server somewhere". Today that rule lives only in Claude's memory and storyboard's CLAUDE.md (generated media), so a session in another repo can miss it. Proposed line under Code and tests: "Media (video especially) never goes in git; it lives on the file server, ~/Movies/<project> until that exists." Related: helmsman render.sh depends on untracked assets (assets/product/plug.mp4, assets/bench/tune.wav, assets/app/*.mp4) whose sources are only in .claude/handoff.md; say if you want them listed in marketing/helmsman-benefits/README.md too.
  options: add the line | memory is enough | add it and list the assets
  result: the rule holds in every repo and session
  id: global-no-video-line
  asked: 2026-11-26
  by: claude-session

- [x] **Local user settings and sign-in: read the proposal and answer its four questions** — docs/decisions/local-user-settings.md. Short version: a box already has local accounts, sessions and a per-person store on the device, so no new sign-in is needed; what is missing is preferences kept on the device (they are per browser today), one Settings window, and a way to make the first account without a shell. The four questions: a setup code or an open first-run page for the first account; whether signed-out screens may change settings; a longer session for an operator's floor screen; settings first and accounts in the UI second, or together.
  result: the settings card can be written and sized
  decided: Nick, interview 2026-11-23: a setup code for the first account; signed-out screens show the device defaults only; a longer session for operators; settings first, then accounts, back to back (cards settings-on-device, first-run-and-accounts)
  id: local-user-settings-answers
  asked: 2026-11-23
  by: session 85e550b0
- [x] **Relay: review and deploy agent-relay #67 (playground-v9)** — the public playground keeps answering "I can't do this one" until this profile is served. Ship the helmsman page first (`make ship` + dev to master): an older page shows the raw `Offer:` lines. Not run there: the integration tests, the live evals, the agent ladder.
  id: relay-67-deploy
  asked: 2026-11-23
  by: session 85e550b0
  answer: dev branch is 11 commits behind main.
  answered: 2026-11-25
  audit: 2026-11-25 done: agent-relay main has all of dev (#68, #72) and the stack was deployed 2026-11-24T23:38Z (scripts/live-state.sh). What is left is helmsman itself: master is behind dev until make staging and make ship run.

- [x] **Take the History tab out, now that Describe has the clock?** — the Agent tab card said "move the history clock here". The clock and its searchable list of versions are in the Describe tab (simulation); the History tab is still in the tab row, because taking a tab out changes saved layouts, the View menu and the tab shortcuts. On a box the versions are still only in its History tab.
  options: remove it in the simulation | remove it everywhere and give a box the clock too | keep both
  decided: Nick, interview 2026-11-23: keep the History tab for now, in both places
  id: history-tab-after-clock
  asked: 2026-11-23
  by: session 85e550b0
- [x] **"Real device: always stop and reset after a change": is a deploy enough?** — on a box a described change is saved into the project and nothing runs until Deploy, which starts the program from scratch. So nothing was changed for a box in the Agent tab card. If you meant the box should deploy (stop, reset, run) by itself when a described change lands, that is a different and bigger thing.
  options: a deploy is enough | deploy by itself after a described change
  decided: Nick, 2026-11-23: the box does not deploy by itself; a deploy is enough
  id: box-reset-after-change
  asked: 2026-11-23
  by: session 85e550b0
- [x] **Where are the brand's face and its wordmark file?** — the header now draws the stencil encoder mark (docs/brand, the `se` symbol) with the painter, and "Helmsman" in Inter, the canvas's own face. The brand page names Saira Stencil One for the wordmark, but no font file is in the repo and the canvas bundles only Inter and Plex Mono. Bundle Saira Stencil One (OFL, one more font in assets/fonts) and set the name in it, or keep Inter?
  options: bundle Saira Stencil One | keep Inter | a different face
  decided: Nick, 2026-11-23: Inter is fine if it is more portable, a graphic alone is fine too; the painted mark with the name in Inter stays
  id: brand-wordmark-font
  asked: 2026-11-23
  by: worker

- [x] **Keep the card "Rebuild the try box at the dev tip"?** — The try box stays off unless a task needs it (your rule of 2026-11-14), so a standing card to rebuild it never finishes.
  options: drop — the card goes; a task that needs the box rebuilds it then (recommended) | keep — it stays in Needs your hands
  recommend: drop — it contradicts the off-unless-needed rule
  stakes: low
  result: unblocks try-box-login
  id: try-box-card
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: drop
  answered: 2026-11-25
  decided: 2026-11-25 Nick: drop the card

- [x] **Windows: add a time-zone database, or say --time-zone is not supported there?** — On Windows the box has no time-zone data, so DATE, TOD and DT read UTC whatever --time-zone says. A database is a new dependency compiled into the box.
  options: document — one line under Platforms, no dependency (recommended) | dependency — a tz crate for Windows only, reviewed as a supply-chain decision
  recommend: document — nobody runs a Windows box in production
  stakes: low
  result: unblocks windows-open-points
  id: windows-time-zone
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: document
  answered: 2026-11-25
  decided: 2026-11-25 Nick: document it, no dependency. Already true: docs/ARCHITECTURE.md "Platforms" lists --time-zone among what is absent on Windows.

- [x] **Deeply nested ST: rewrite the compiler to be iterative, or lower the nesting limit?** — A program nested 200 deep compiles only when the caller gives a 4 MiB stack; in the browser the stack is 1 MiB. Lowering the limit is a product limit an engineer could meet.
  options: measure then rewrite — keep 200, measure the browser first, make operator chains iterative (recommended) | lower the limit — smaller change, programs past the new limit are refused with a message
  recommend: measure then rewrite — no real program is refused
  stakes: high
  result: unblocks compiler-own-stack
  id: compiler-nesting-limit
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: measure then rewrite
  answered: 2026-11-25
  decided: 2026-11-25 Nick: measure, then rewrite; the limit stays 200

- [x] **Named start states: inputs only, or kept values too?** — A start state sets a machine up before a simulation run. Inputs are simple to name and check. Kept values (a step number, a latch) let a test start mid-cycle but tie the state file to the program's internals.
  options: inputs only — a file of input = value lines (recommended) | inputs plus kept values — can start mid-cycle; a renamed variable breaks the state
  recommend: inputs only — add kept values when a test needs them
  stakes: low
  result: unblocks sim-start-states
  id: start-states-kept-values
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: inputs only
  answered: 2026-11-25
  decided: 2026-11-25 Nick: inputs only

- [x] **Is IO-Link on the Skipper still wanted?** — An IO-Link sensor's data sheet (its IODD) is already used as library metadata without talking IO-Link. Talking it from the Skipper needs a transceiver part and a driver.
  options: park — keep the metadata route, close iolink-node and iolink-transceiver for now (recommended) | build — order the transceiver; the software-only first step can start today
  recommend: park — no customer has asked and the metadata covers naming
  stakes: high
  result: unblocks iolink-node, iolink-transceiver
  id: iolink-still-wanted
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: park
  answered: 2026-11-25
  decided: 2026-11-25 Nick: park

- [x] **Device library: how does an entry say what was not proven on a real unit?** — Entries ship with unproven parts flagged (your decision of 2026-11-17). The flag needs a place in device.toml that the index and the site can count.
  options: proof table — a new optional [proof] table per entry (recommended) | reuse questions — put it in the existing [[questions]] list; no schema change, harder to count
  recommend: proof table — "proven 6 of 8" is one lookup
  stakes: low
  result: unblocks library-unproven-flags
  id: library-proof-shape
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: proof table
  answered: 2026-11-25
  decided: 2026-11-25 Nick: a [proof] table per entry

- [x] **How should the Agent hand back its answer so the page does not parse prose?** — Today the page reads fenced blocks out of the model's text and guesses when it is cut short. A structured answer needs a new relay profile and your deploy.
  options: tool call — the answer is a tool call with a schema (recommended) | JSON schema — a structured-output response format; depends on the model the relay runs | keep prose — no relay change, the heuristics stay
  recommend: tool call — every model the relay can run supports it
  stakes: high
  result: unblocks agent-acceptance-signal
  id: agent-structured-output
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: JSON schema
  answered: 2026-11-25
  decided: 2026-11-25 Nick: a JSON schema response format (not a tool call)

- [x] **A box with its device links off: a start flag only, or a switch in the UI as well?** — A Studio box beside the Skipper should not claim the bench devices. A flag is set when the box starts; a switch lets an engineer flip it while it runs.
  options: flag only — --links off at start (recommended) | flag and switch — also an engineer-only switch on the Devices tab
  recommend: flag only — a running switch is a way to drop a machine's I/O by accident
  stakes: low
  result: unblocks studio-box-no-io
  id: links-off-switch
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: flag only
  answered: 2026-11-25
  decided: 2026-11-25 Nick: flag only

- [x] **Is a new relay profile for the box worth it, so the box Agent answers with edits and can ask a question?** — The page on the free chat answers a long program with only the changed lines (7 s against 24 s) and can ask one question. The box still gets the whole file back and shows no question. Closing the gap needs a new served profile and your deploy.
  options: yes — one new profile id for the box, then the page-side work (recommended) | not now — the box keeps whole-file answers
  recommend: yes — engineers on a box have the long programs
  stakes: high
  result: unblocks agent-edits-everywhere
  id: box-edits-profile
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: yes
  answered: 2026-11-25
  decided: 2026-11-25 Nick: yes

- [x] **Where does an output's safe state live?** — When the Agent names the I/O first it can also ask what each output does on a fault or a lost device. That answer has to be saved with the project.
  options: io.map — a safe= word on the output's line (recommended) | new file — a separate safe-state file in the project
  recommend: io.map — it is where the point is already described and what the box loads
  stakes: low
  result: unblocks agent-names-io
  id: safe-state-home
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: io.map
  answered: 2026-11-25
  decided: 2026-11-25 Nick: io.map

- [x] **A part matched from a drawing: does its draft I/O line carry an address?** — Reading a DXF gives the part and its terminals, not its network address.
  options: leave blank — the line is drafted without an address and the review asks for it (recommended) | default address — the library's default address is filled in and can be changed
  recommend: leave blank — a guessed address that happens to answer is worse than a question
  stakes: low
  result: unblocks drawing-review
  id: drawing-draft-address
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: default address
  answered: 2026-11-25
  decided: 2026-11-25 Nick: the library's default address is filled in

- [x] **Where does the first-run setup code come from on a preloaded Skipper?** — A new box with no accounts asks for a one-time code before it makes the first account.
  options: file — a file beside the pairing files, also printed on the packing slip (recommended) | label — printed on the device only; needs a step at assembly
  recommend: file — no assembly step, and it can be reissued
  stakes: high
  result: unblocks first-run-and-accounts
  id: setup-code-source
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: label
  answered: 2026-11-25
  decided: 2026-11-25 Nick: a label on the device

- [x] **Text as a signal: a bounded string of 255 bytes, as in the language?** — STRING ports are skipped today, so a program's text output never reaches the screen, the trend or OPC UA.
  options: yes — one text type, 255 bytes, as STRING is in the language (recommended) | not yet — text stays inside programs
  recommend: yes — it matches the language and bounds memory
  stakes: low
  result: unblocks st-string-signals
  id: string-signal-type
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: yes
  answered: 2026-11-25
  decided: 2026-11-25 Nick: yes, 255 bytes

- [x] **Git on the controller: which library?** — A project's history as a real git repository on the box needs a git implementation compiled in.
  options: gix — pure Rust, no C, a large dependency tree (recommended) | libgit2 — mature, a C library to build and patch | export only — no library; the box writes a bundle an engineer imports elsewhere
  recommend: gix — keeps the box free of C and unsafe
  stakes: high
  result: unblocks version-control
  id: version-control-library
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: the box writes a bundle an engineer imports elsewhere
  answered: 2026-11-25
  decided: 2026-11-25 Nick: no git library: the box writes a bundle an engineer imports elsewhere

- [x] **Who holds the key that signs Skipper firmware?** — Signed updates mean a Skipper refuses an image that is not yours. The key has to live somewhere.
  options: yours — offline, you sign each release (recommended) | relay-held — the relay signs on request; convenient, and a relay compromise signs firmware
  recommend: yours — a firmware key should not sit on an internet service
  stakes: high
  result: unblocks fleet-console
  id: ota-signing-key
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: yours
  answered: 2026-11-25
  decided: 2026-11-25 Nick: Nick holds it, offline

- [x] **Where may the Agent keep an engineer's habits between sessions?** — The Agent notices habits (a naming style, a preferred pattern) but forgets them when the page closes.
  options: this browser — nothing leaves the machine (recommended) | the project — travels with the files, seen by everyone on it | the account — follows the person on a box
  recommend: this browser — no question of what is kept about a person
  stakes: high
  result: unblocks agent-patterns
  id: agent-habits-where
  asked: 2026-11-25
  by: session ab81ec57 (board audit)
  answer: the account
  answered: 2026-11-25
  decided: 2026-11-25 Nick: the account

## Needs your hands

- [ ] **Film the Skipper and the motor it controls for the benefits reel** — Why: reel v5 (~/Movies/helmsman-benefits-reel-v5.mp4, 34.5 s, PR #646, you said "Way better") shows one device at a time, so a viewer can't tell what controls what. Shots, landscape 1080p or higher, into ~/Movies/helmsman-benefits/footage/: (1) Skipper and motor in one frame, the cable going in, the LED turning green (replaces the H3 plug-in shot); (2) the motor playing the song with the Skipper in view, with sound (step 3, Control devices); (3) optional: an idle shot of both for step 2, Scan (otherwise the current scan take stays, per your answer). Files only, never committed.
  needs: the Skipper, the bench motor, a phone or camera
  result: a session cuts the footage into plug / control (and scan), re-renders, runs the gate, pushes #646 for your approval
  est: 20m
  difficulty: low
  id: reel-device-footage
  asked: 2026-11-26
  by: claude-session
  status: blocked

- [ ] **Watch the IMPACT67's LEDs while a session switches it, and wire one output to one input** — Why it matters: the block was driven on 2026-11-16 with nothing on its ports and nobody looking, so no channel is confirmed and nobody knows what its outputs do when the controller goes away (after a Forward_Close its output assembly still read the last values written). Steps: (1) put an M12 jumper from pin 4 of port 0 to pin 4 of port 1 (or tell the session which channels you joined); (2) tell a session "watch the Murr with me": it sets port 0 pin 4 as an output, switches it, and asks what the channel LEDs do, then closes the connection and stops sending in turn and asks again.
  needs: the block powered at its DHCP address (10.0.0.73 on 2026-11-16; a session finds it by ListIdentity)
  result: the library entry's channels confirmed on a unit, and its question about outputs after a lost connection answered
  est: 15m
  difficulty: low
  id: murr-outputs-watch
  asked: 2026-11-16
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for Nick: a jumper, eyes on the LEDs, the block powered.

- [x] **Run spec mode against a real model, in the simulation and on a box** — #471 and #475 are tested with scripted answers only; no model has yet read a scope of work and answered in the format (a `Why:` line, the PROGRAM with a comment on each input and output, one scenario a requirement, a `requirements` block). Paste a real scope of work into "From a spec…" in the Agent tab of the playground, then the same with a PDF on a box whose agent is online, and note what comes back: names, comments, how many requirements are tested, whether the retries are used up.
  needs: a scope of work or a product document you would really build from; a box with its agent reachable for the PDF half (the try box, started for this and stopped after)
  result: spec mode is known to work on real documents, or the first real failures are on the board
  est: 30m
  difficulty: low
  id: spec-mode-real-run
  asked: 2026-11-16
  by: claude-session
  audit: 2026-11-25 closed, stale: spec mode was removed in #603; documents go into Design a machine, which is shelved (f2af2084).

- [x] **Deploy the relay so the Agent's edits and its one question go live** — Why it matters: the page on dev answers a long program with only the lines that change and asks one question while it works, but only where the relay serves those agents; the deployed relay does not yet, so the published page behaves as before. agent-relay #40 (merged to its dev when green) adds the demo's `edits`, `ask` and `check` agents; nothing in template.yaml changes. Steps: (1) promote agent-relay dev to its default branch; (2) deploy as usual (`scripts/deploy.sh`) with `RELAY_MODEL=deepseek/deepseek-v4.1-flash-1017` (decided 2026-11-16 from the ladder: 45 of 45, fastest and cheapest of the four, a dated snapshot) and `DemoAddressTurnsPerHour` raised from 40 to about 120: one instruction is now up to three turns (the question, the change, the check), so the old limit closes the free chat for everyone about three times sooner; (3) `curl -s -H "Origin: <the page's origin>" <demo url>/demo/v1/status` should list `"agents":["ask","check","edits"]`; (4) tell a session, which then runs the ladder against the deployed demo and looks at the card in a browser.
  needs: agent-relay #40 merged, your promote and deploy
  result: the published playground answers a one-line change to a long program in about a third of the time, and asks its question
  notes: 2026-11-18: the same promote and deploy also serves helmsman/v12 and helmsman/playground-v6 (agent-relay #48, in its dev): the Agent writes one short comment above each part of a program, which the diagram shows as that section's name. A box asks for it with `--relay-agent v12` (it needs the v2 client, in dev since #506); the playground gets it when RELAY_DEMO_AGENTS maps `edits` to helmsman/playground-v6. v12 passed its live eval and v11 failed the same one as a control; playground-v6 has offline tests only.
  id: agent-edits-deploy
  asked: 2026-11-15
  by: claude-session
  audit: 2026-11-25 done: agent-relay #40 and #48 are on main and the live demo status lists ask, check, edits, start, summary. Not checked: the turns-per-hour setting.

- [ ] **Fit the P4's PoE module and measure its isolation** — Why it matters: a node in a cabinet is powered by its Ethernet cable alone, and today no run has proven that. The PoE module turns the cable's 48 V into the board's supply; if its output is not isolated from the cable side, a USB cable plugged in at the same time ties the laptop's ground to the PoE supply's return, and current can flow through the laptop's USB port. Nobody knows yet which it is, so `scripts/node-flash.sh` makes you pull Ethernet before every USB flash. One measurement settles it: the prompt goes away, or becomes a hard rule. the Waveshare ESP32-P4-ETH has run on USB and the bench switch without its PoE module (docs/EDGE.md Phase 3, "On the board"). Before USB and PoE are ever connected together: with the module fitted and the board powered from PoE only, measure for continuity and resistance between the RJ45 shield and PoE return on one side and USB ground on the other (unpowered first). Then a run on PoE alone, USB out: the node is at http://192.168.3.56:8080 (press-5).
  needs: the PoE module on the board, a meter, a PoE port
  result: "isolated" or "not isolated" written into `board.rs` and `scripts/node-flash.sh` (which then keeps or drops its Ethernet-unplugged prompt and `NODE_P4_NO_POE`), and the node seen running on PoE alone
  est: 30m
  difficulty: low
  id: p4-poe-isolation
  asked: 2026-11-10
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for Nick: the PoE module fitted, a meter, a PoE port.

- [x] **Rebuild the try box at the dev tip** — it came back when you logged in at the Studio (pid 86028 on 2026-11-13, :8081 answers, the E1213 up), but on the build of 2026-11-08 (e7f1ae2, #300), about 90 merges behind dev: no device profiles, no device configuration, no glyphs. Best done after `studio-box-no-io` lands, so the rebuilt box can be switched off the devices for P4 work.
  needs: `~/github/helmsman-try/scratch/16-rebuild.sh` from your terminal at the Studio (it builds at the dev tip, signs, and restarts the LaunchAgent)
  result: https://10.0.0.41:8081/ at the dev tip with the E1213 up
  notes: check after: `curl -sk -H "Authorization: Bearer $(cat ~/github/helmsman-try/scratch/try-state/api-token)" https://127.0.0.1:8081/api/devices` shows iologik_e1213 up.
  est: 10m
  difficulty: low
  id: try-box-login
  asked: 2026-11-10
  by: claude-session
  audit: 2026-11-25 waits for Nick: the rebuild from his terminal. See the question try-box-card.
  decided: 2026-11-25 Nick: dropped; a task that needs the try box rebuilds it then.

- [x] **A public device library: map a device once, for everyone** — interview 2026-11-02: a public repository (JimothyJohn/helmsman-devices) with one folder per device: the EDS or the Modbus map, the `enip-io` profile or map lines, a doc with the manual's citations, the identity (vendor, product code, model text). The box and the agent look a device up before generating anything: by identity first (ListIdentity's vendor and product code, a Modbus unit's identification), then by vendor/model text; semantic search via the relay only once there are enough entries to need it. An import in the Devices tab offers to publish the result (a PR to the repo from the relay, the engineer confirming).
  needs: Nick creates the public repo (a scratch script); the licence line per vendor file (the Moxa EDS carries no terms: a pointer and a hash, not the file, until Moxa answers)
  result: the second user of an E1213 gets its points from the library in one click; the agent's mapping work is never repeated
  est: 2d
  difficulty: med
  id: devices-repo
  asked: 2026-11-02
  by: claude-session
  notes: 2026-11-06: helmsman-devices PR #1 (index.json) is merged into its dev; main has no index.json yet. Left: dev → main there (yours), then tick.
  audit: 2026-11-25 done: JimothyJohn/helmsman-devices is public with index.json and 175 device.toml files; the box looks up by identity (helmsman-project/src/devlib.rs), adds by click (#365) and publishes (helmsman-agent/src/publish.rs).

- [ ] **Supply the datasheet image** — docs/helmsman-datasheet.pdf (docs/datasheet.html) has no picture yet. Nick 2026-11-01: card it, place it when the file exists. Drop the file under docs/brand/ (PNG or SVG; a photo at 1200 px wide or more, or a render) and say where it goes: in the top band right of the wordmark (about 45 mm wide, the lede moves down) or as a strip under the lede. The build (`make datasheet`) refuses a second page, so the slot comes out of the spec table's air.
  needs: the image file and its spot
  result: the datasheet carries the image; rendered, one page, re-committed
  est: 30m
  difficulty: low
  id: datasheet-image
  asked: 2026-11-01
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for Nick: the image file and its spot.

- [ ] **Check the L5X reader against RM014 and a real export** — `helmsman_iec::l5x` reads the export shape from the `l5x` Python package and memory (docs/SOURCES.md); run it on a Studio 5000 export of a bench project (controller, program, routine and rung exports) and the manual, and fix what differs.
  needs: the RM014 PDF (403 from the agent) and a Logix Designer export
  result: the inventory is right for real customer files
  est: 3h
  difficulty: low
  id: l5x-real-export
  notes: source (l5x-source, 2026-10-31): Nick exports one from his bench project. Later 2026-10-31: RM014 is read and cited; the ladder converter (l5x-import) also needs the real export: run `POST .../import?file=X.L5X` on it and check the refusal list (or the ST) against the project, the neutral text of each instruction it uses (JSR's `JSR(Name,0)` form, `?` pseudo-operands, v36 mnemonics such as EQ/MOVE), decorated data of TIMER/COUNTER tags, and rung Type attributes other than N. The fixtures (line3.L5X, ladder.L5X) are hand-written, not exports.
  status: blocked
  audit: 2026-11-25 waits for Nick: a Studio 5000 export and the RM014 PDF.

- [ ] **Try the node canvas** — ComfyUI/Node-RED style editor in Rust (egui, wasm): nested function blocks, live scan sim, not wired to the PLC. Kept as a lab tool (2026-10-29); per canvas-edit-scope the operator canvas is the read-only ST view (st-graph-view), which reuses tcpethernet-graph and this renderer. Serve it: `make canvas-web`, then `python3 -m http.server 8790 --bind 0.0.0.0 --directory target/canvas-dist` and open http://10.0.0.41:8790/ from the Air (stop: `kill $(lsof -tiTCP:8790 -sTCP:LISTEN)`). In it: hold a start button, double-click a motor to go inside, select nodes + Cmd+G to make a block, `?` lists the controls.
  needs: your eyes on the UI
  result: a list of changes, or none
  notes: `make canvas-serve` (trunk, live reload) only answers on localhost on the Studio: macOS's firewall holds the unsigned trunk binary's incoming connections behind a GUI prompt; python3 is allowed
  est: 15m
  difficulty: low
  id: canvas-try
  status: blocked
  audit: 2026-11-25 waits for Nick: his eyes on the UI.

- [x] **Open raw Ethernet to your user on the Studio (one sudo script)** — Why it matters: only once a device has to be reached over PROFINET, and that is no longer first (Nick, 2026-11-13: EtherNet/IP and Modbus TCP are what plants run). PROFINET's cyclic data is raw Ethernet frames, not IP, and on macOS a program sends those through `/dev/bpf*`, which only root can open. Parked with the PROFINET cards. PROFINET's cyclic data, alarms and station discovery are Ethernet frames, not IP, and on macOS a program sends them through `/dev/bpf*`, which is root-only here (`/dev/bpf0` is root:wheel 0600; you are already in `access_bpf`, so Wireshark's helper was installed once and is gone). Until this runs, everything PROFINET is proven against the simulated device only: the BPF code has compiled and parsed synthetic buffers and has never carried a frame.
  needs: a device that has to be reached over PROFINET; then `~/github/helmsman/scratch/27-bpf-access.sh` in the native terminal (sudo; again after each reboot), then `! ~/github/helmsman/scratch/28-profinet-scan.sh` here
  result: the scan lists the PROFINET stations on en0 (none is a fine answer with nothing plugged in: it proves frames go out and the listen works); a session can then run a box against a real device without sudo
  done: 2026-11-15: /dev/bpf* are root:access_bpf 0660 (Wireshark's ChmodBPF LaunchDaemon is installed, so it holds across reboots; no sudo step needed). `scratch/28-profinet-scan.sh en0` from a session, no sudo: raw Ethernet 0x8892 opens, DCP Identify sent, 0 stations (nothing PROFINET on the wire).
  est: 5m
  difficulty: low
  id: profinet-raw-access
  asked: 2026-11-10
  by: claude-session

- [x] **Bring up the Novanta IMS Liberty MDrive over EtherNet/IP or Modbus TCP** — Why it matters: the first device on the bench that is not discrete I/O. A motor drive is where the device profile's "kinds" get decided (step (g) of `device-profiles`: a drive's enable, speed, position and fault as the same four channels whatever the vendor), and where a comms loss has to stop something that moves. PROFINET first is out (Nick, 2026-11-13), so this starts on the protocols plants run. What its files say (docs/devices/novanta-mdrive.md, read 2026-11-10): one Ethernet interface speaks Modbus TCP (the factory default), EtherNet/IP, PROFINET IO or MCode/TCP, one at a time, picked with Novanta's TCP/IP Configuration Utility; default IP 192.168.33.1; its EDS is in the firmware zip. Steps: Modbus TCP as it ships (no vendor tool needed), read status and position, then a jog and a move to position from ST with the fault path and the comms-loss stop tested; then EtherNet/IP from its EDS, so one device compares two drivers; PROFINET later, if ever. Motion asks "cell clear?" once per session (hardware-guard).
  needs: the drive on the bench at 192.168.33.1 (the Studio reaches it through `sudo ifconfig en0 alias 192.168.33.10 255.255.255.0`, gone at a reboot)
  result: a device doc with citations, a profile and a library entry; an ST program that jogs and positions the axis and stops it on comms loss
  est: 2d
  difficulty: med
  notes: 2026-11-16 night, on the bench unit (LMDCE853C, EtherNet/IP mode, identity revision 2.36, `LMDCM 6.013`): EtherNet/IP came first because that is the mode it arrived in. Done: the connection (docs/devices/novanta-mdrive.md "On the bench": a run/idle header its EDS does not declare, which `enip-io` now corrects from the refusal; it sends 1.3 to 1.5 times slower than the RPI it grants; it resets the TCP session after 2 to 3 s and keeps exchanging I/O; UDP 2222 only), the vendor's EDS resolving and connecting, a 20-minute hold, a slew at 5000 steps/s and a stop from the cyclic image, the library entry (helmsman-devices `devices/novanta/lexium-mdrive-ethernet/`). Found: the output assembly is not laid out as the manual says, and a silent controller does not stop a slew (both are questions above). 2026-11-17: the unit's mapping is known (Nick's photo of the Configuration Utility; docs/devices/novanta-mdrive.md has it, and it explains the move at the first Forward_Open). Decided (Nick, 2026-11-17, on the board): that the drive keeps slewing when the controller goes silent is accepted on the bench for now; before it is shown or left running unattended it needs a stop of its own (the utility's Halt Motion setting read back with Get, an MCode program, or a hardware input). Not explained, for later: a small MA through bytes 28..31 and over MCode did not move the drive (errors 25, 30). Left: the profile and the kinds (a drive's enable, speed, position, fault), MA and MR, the ST program with the fault path, and Modbus TCP for the comparison (the drive speaks one protocol at a time).
  id: mdrive-bringup
  asked: 2026-11-10
  by: claude-session
  notes: 2026-11-17: Nick on the comms-loss stop: accept it on the bench for now. A silent controller does not stop a slew on this unit (it stops on a Forward_Close and on run/idle going to idle); what its Halt Motion setting really holds was not read. Open again before the drive runs anywhere but the bench: an MCode program on the drive or a hardware stop.

- [ ] **Bring up a Universal Robot over EtherNet/IP and Modbus TCP** — Why it matters: the first device with a large mixed image (floats, 32-bit registers, bit fields), so it tests what a profile can say about a value beyond on and off, and a robot is something customers already own. PROFINET first is out (Nick, 2026-11-13). Read 2026-11-10 (docs/devices/universal-robots.md): EtherNet/IP input assembly 100 (480 bytes), output assembly 112 (224 bytes), RPI from 2 ms (its EDS); Modbus TCP server on 502, function codes 1..6, 15, 16, general purpose registers 128..255, robot state from 256, every value an unsigned 16-bit word (angles in mrad, positions in tenths of a millimetre, signed values converted by hand), the unit id ignored. EtherNet/IP and PROFINET cannot be on together. URSim covers Modbus (2026-11-20: registers live and writable, contrary to the forum reports) but not EtherNet/IP. PROFINET (its GSDML is read, slots 1..10) only if a customer asks.
  needs: the robot on 192.168.3.x, its model and PolyScope version, EtherNet/IP enabled under Installation > Fieldbus; a cleared cell before anything moves
  result: docs/devices/universal-robots.md "On the bench" with what each interface accepted; a profile with both transports; an ST program that reads the robot's state and sets a general purpose register; the register quirks held as tests
  notes: 2026-11-20, Modbus TCP against URSim 5.26.0 and PolyScope X 10.13.0 (no robot): done, docs/devices/universal-robots-e-series.device (Modbus only), docs/devices/ur-modbus/ (ST: state, a joint angle put together, a heartbeat in register 130, run on a box against the sim), crates/helmsman-io/tests/universal_robots.rs (holes refused whole with exception 2, a joint angle split into 0..6283 mrad plus signed turns at 320.., signed TCP). Left: EtherNet/IP (the PolyScope X sim lacks the ur-ethernetip URCap; Nick is getting it, 2026-11-20), then the robot itself for both. Library entry: helmsman-devices#49 (devices/universal-robots/e-series/, flagged simulator-only, three questions for the robot).
  est: 2d
  difficulty: med
  status: doing
  id: ur-bringup
  asked: 2026-11-10
  by: claude-session
  audit: 2026-11-25 partly: Modbus against URSim landed (c63ccbac, 7b519ccf, f40533bc, helmsman-io tests/universal_robots.rs). Left: EtherNet/IP (waits on the URCap) and the real robot.

- [ ] **Bring up the Murrelektronik IMPACT67 E DI8 DO8 (55086)** — Why it matters: every implicit-messaging finding so far is against one Moxa unit on one old firmware; a second vendor's block is what shows whether the EtherNet/IP driver is right or only right for that Moxa. A second unit also ends the contention for the E1213's single connection: the P4 owns one, a Studio box the other. What is already done without the unit (helmsman-devices `devices/murrelektronik/`, from its EDS and handbook, 2026-11-12): the 55086 is EtherNet/IP only, vendor 640, product code 55086, input instance 100 (4 bytes), output 112 (2 bytes), RPI 10 ms by default; its folder has the doc, an enip map and a `.device` profile. It ships at rotary 000, DHCP/BOOTP, so it is also the first device `box-dhcp-server` can address with nobody touching it. Left for the bench: power it, let it take an address, Scan, add by click, and record what its Forward_Open accepts.
  needs: `murr-outputs-watch` (a jumper and your eyes) for the channels
  result: the block found, added and driven from the library's profile with no hand-written map; its doc gains "On the bench"
  est: 4h
  difficulty: med
  notes: 2026-11-16 night: the block that arrived is a **55089** (DIO8), not a 55086, at 10.0.0.73 by DHCP, revision 1.2. Done (docs/devices/murrelektronik-impact67-e.md, helmsman-devices `impact67-e-dio8-dio8`): identity and assemblies confirmed, the connection opened from the library's sizes and held 20 minutes at 10 ms, its setup sent as configuration data (`config=109 config_data=00ff00`, new in `enip-io`: out of its box it refuses instance 110 with 0x0119). Left: Scan and add by click from the canvas, a channel seen switching, what the outputs do on a lost connection.
  id: murr-block-bringup
  asked: 2026-11-10
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits on murr-outputs-watch and the bench.

- [ ] **Update the bench E1213 from firmware 3.1 to 3.3** — your answer (2026-11-13): 3.3 only, by a session if it can. It cannot, for two reasons: helmsman-devconf maps the console's Firmware Update page (`06_4.htm` to `06_4_1.htm`, helmsman-devices `devices/moxa/iologik-e1200-web-console.md`) and does not drive it, and a scripted upload that goes wrong leaves the bench's only EtherNet/IP unit unusable until Moxa recovers it; and the 3.2 and 3.3 files were not found on Moxa's site (2026-11-13 search: the product page offers the latest release, 4.0.1). Why it matters: v3.3 is the release Moxa says fixes "Ethernet/IP cannot establish second input-only session", so after it a P4 node and a Studio box could both hold a cyclic connection to the one unit instead of taking turns. Steps: (1) in the canvas, Devices > iologik_e1213 > Configuration > Save as project file, so its settings can be restored; (2) get the E1213 firmware v3.2 and v3.3 files from Moxa (support site, ioLogik E1213, software packages; if only 4.0.1 is listed, ask Moxa support for the two older builds); (3) http://192.168.3.60 > Firmware Update, upload 3.2, wait for the unit to come back (ping, then the overview page shows V3.2); (4) the same with 3.3; (5) check on the console that EtherNet/IP is still enabled, then tell a session "E1213 is on 3.3". The session then runs `scratch/enip-fo-probe.py` and `scratch/enip-hold-probe.py` (Input Only beside an Exclusive Owner, the port-2222 change) and rewrites docs/devices/moxa-iologik-e1213.md.
  needs: your hands on the unit's web console, and the two firmware files
  result: the unit on V3.3 with its settings unchanged; a measured yes or no on two cyclic connections at once
  est: 1h
  difficulty: low
  id: e1213-firmware-update
  asked: 2026-11-10
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for Nick: the web console and the two firmware files.

## Next to build

- [x] **Scan asks from every interface, and one address on request** — Why it matters: on 2026-11-24 the MDrive (192.168.33.1) was invisible to Scan because Wi-Fi was the Studio's default route and a broadcast leaves on that interface only. Measured from the Studio (en1 Wi-Fi 10.0.0.41 default; en0 192.168.3.10 with an alias 192.168.33.10): an unbound socket sending to 255.255.255.255 got the E1213 only; a socket bound to 192.168.3.10 or 192.168.33.10 got the E1213 and the MDrive; a directed broadcast to 192.168.33.255 got nobody; a ListIdentity straight to 192.168.33.1 was answered. The sweep now asks once from each local non-loopback IPv4 address (addresses read from `ip -o -4 addr`, else `ifconfig`; none readable falls back to the single send), and `POST /api/devices/discover?address=A` plus an Ask field beside Scan asks one device.
  result: a device on a wire that is not the default route is listed by Scan, or found and added by its address
  needs: a run on the bench from the try box to see the MDrive listed (cargo-built binaries cannot reach the LAN from a session; not run)
  difficulty: med
  id: scan-every-interface
  asked: 2026-11-24
  by: worker
  status: done

- [ ] **Windows: what the green CI job does not prove yet** — the box builds, lints and passes its tests on windows-latest since #565 (2026-11-23), with these left open. (1) Names Windows treats as devices (`con`, `nul`, `aux`, `com1`, `lpt1`, with or without `.st`) as project and file names: not tested; a write to `nul.st` may vanish without an error. (2) Shutdown: no test sends Ctrl-C or a terminate on Windows, so RETAIN on exit is unproven there (the SIGTERM test is Unix-only). (3) `--time-zone` and the zone tests are Unix-only (no tz database on Windows). (4) The Windows job checks out with LF; a project with CRLF files is not tested on any platform. (5) EtherNet/IP and Modbus TCP pass on loopback there, never against a device. (6) `hammering_the_api_does_not_hold_up_the_scan` allows an 8x fall on Windows against 4x elsewhere (154 ticks against 680 on the runner): measure on a real Windows machine before calling the scan timing good there. (7) `box (windows-latest)`, `box (macos-latest)` and `box (ubuntu-24.04-arm)` are not required checks.
  needs: a Windows machine for (5) and (6); the rest runs in CI
  result: each point has a test or a line in docs/ARCHITECTURE.md "Platforms" saying it is not supported
  est: 1d
  difficulty: med
  id: windows-open-points
  asked: 2026-11-23
  by: claude-session 734e70dc
  depends: windows-time-zone
  audit: 2026-11-25 plan: 1) a test that con, nul, aux and com1 are refused as project and file names; 2) a Windows terminate test for RETAIN on exit; 3) a CRLF fixture on every platform; 4) the time-zone answer (question windows-time-zone); 5) points 5 and 6 need a Windows machine.
  decided: 2026-11-25 Nick: no time-zone database on Windows; point 4 is closed by the Platforms section as it stands.

- [ ] **Windows job: find the remaining intermittent failures** — `box (windows-latest)` still fails now and then for reasons other than the reset #571 fixed (2026-11-23). (1) In run 45194902994 two tests that start the binary timed out and passed in the next run: `a_killed_box_resumes_its_retain_counter_and_says_what_changed` (tests/retain.rs, 20 s waiting for the first save) and `a_module_output_is_published_by_the_core` (tests/sink_bin.rs, 10 s waiting for an event). The cause is unknown and the failures do not print the box's stderr, so start by making both say what the box wrote when they time out, then read the next failure. (2) The 503 a full box sends from the accept loop (api/mod.rs) closes with the request unread, the same reset hazard the 408 had; it cannot read on there without holding up accepts, so it needs another way (hand the refusal to a short-lived thread, or read what has already arrived before closing) and a test like `a_client_cut_off_at_the_deadline_reads_the_408_however_late_it_stops_sending`. (3) 2026-11-23: Windows sometimes refuses the box a socket (`os error 10013`), in different tests each time, while the same commit passes on a parallel run (runs 45288348848, 45289778227); look at the runner's excluded port ranges first.
  needs: nothing; (1) waits on a failure with the box's stderr in it
  result: both timeouts have a named cause and a fix or a bound that says why, and a busy box's 503 reaches a Windows client
  est: 4h
  difficulty: high
  id: windows-intermittent-failures
  asked: 2026-11-23
  by: claude-session 734e70dc
  audit: 2026-11-25 plan: 1) the retain and sink_bin timeouts print the box's stderr (small, do first); 2) the 503 is handed to a short thread that drains the request, with a late-sender test; 3) read the next 10013 failure and bind with retry.

- [ ] **The compiler's stack use stops depending on who calls it** — a program nested to the limit needs more stack than a 2 MiB thread has in a debug build with full debug info: 190 chained `+` took 2 to 2.5 MiB, 64 parentheses 1.5 to 2 MiB (macOS arm64, 2026-11-22; `nesting_up_to_the_limit_runs` overflowed in the new macOS CI job). Release fits (under 768 KiB at worst), so what ships is safe on a 2 MiB thread; a debug box compiling on one can be crashed by a hostile program, and the browser build (1 MiB main stack by default) is not measured. Nick, 2026-11-22: give the test 4 MiB now (done), fix it properly here. Ways: compile on a thread of the compiler's own with a set stack (not in the browser), handle long operator chains without recursion, or both.
  needs: nothing; start by measuring the wasm build at the limit
  result: the deepest allowed program compiles on a 1 MiB stack in every build, the test goes back to a small stack and says so
  est: 1d
  difficulty: high
  id: compiler-own-stack
  asked: 2026-11-22
  by: claude-session 734e70dc
  depends: compiler-nesting-limit
  audit: 2026-11-25 plan: 1) measure the wasm build at the limit; 2) parse and compile chained binary operators iteratively; 3) natively, compile on a thread with a set stack; 4) put the test back on a small stack.
  decided: 2026-11-25 Nick: keep MAX_NESTING at 200; measure the browser build first, then make operator chains iterative.

- [ ] **Named start states for the simulation: set a machine up once, pick it again** — Nick, 2026-11-16: "the user should be able to rapidly configure the state of the machine for various scenarios manually and with the agent before starting." Built first (#493): an input set by hand while the program is stopped is what it starts with, the values outlive Stop, and a quiet line says nothing reacts yet. Decided by interview that day: this card follows, simulation only; a stopped box only reminds. Left: a start state gets a name ("part seated, guard open") and is saved in the project as a text file the diff shows, one `input = value` a line; the I/O tab lists them above Inputs and a click sets every input of one while the program is stopped (running: asked first, since it moves the machine); the Agent writes one on request, checked against the program's inputs and their types before it is offered. Open in the design: whether a start state also names kept values (a step number, a latch), which the panel cannot set today. The Agent's first half is built (2026-11-17, PR feat/agent-start-state with agent-relay `start-v1`): a field above the Inputs takes the state in words, the Agent proposes input values, and nothing is set until Set these; it works once the relay is deployed with the `start` agent, and it is not yet a saved, named state.
  needs: #492 and #493 in dev; the Agent's part needs a relay profile with the new block (a new id, Nick's deploy)
  result: a list of named start states in the I/O tab, saved with the project, that the Agent can add to
  est: 8h
  difficulty: med
  id: sim-start-states
  asked: 2026-11-16
  by: claude-session 4a05a85d
  depends: start-states-kept-values
  audit: 2026-11-25 plan: 1) a *.state text file, one input = value per line, parsed and checked against the program's inputs; 2) a list above Inputs in the I/O tab, click to set; 3) Save as, in the project's files and diff; 4) the Agent writes one (a relay profile, Nick's deploy).
  decided: 2026-11-25 Nick: a start state names inputs only; kept values wait for a test that needs them.

- [x] **A spec run into the project that is open** — spec mode (#471, #475) always makes a new project; Nick's ask was to "parse and add it". Left out because a kept file (`spec.md`) cannot be saved one at a time: `Projects::save` on a box and `SimProjects::add_file` in the page take only names `file_name` knows, and a box has no atomic write of several files short of a whole-project zip. Shape: the same window with a project open offers "Add to this project"; the spec goes in as an instruction on the open program file (`Scope::File`), the review shows what changes (I/O added, requirements added or replaced), and Accept writes the file, the tests and the new section of `spec.md` together.
  needs: nothing
  result: more documentation dumped in later lands in the same project, with its spec kept beside the first
  est: 2d
  difficulty: med
  id: spec-into-open-project
  asked: 2026-11-16
  by: claude-session
  audit: 2026-11-25 closed, stale: spec mode was removed in #603.

- [x] **PDFs in the playground's spec window** — on a box a spec can be a PDF (#475, a drop-in on `/api/chat`); in the page it is pasted text, `.md` or `.txt` (#471). The page's free route carries no documents at all (the relay's demo function refuses them, 256 KiB a request), so this is for a visitor's own key and the local proxy: `claude.rs` builds `content` as a string today and would send a `document` block.
  needs: nothing
  result: a PDF scope of work builds a project in the page with a key, and the window says why not on the free route
  est: 1d
  difficulty: med
  id: spec-pdf-in-page
  asked: 2026-11-16
  by: claude-session
  audit: 2026-11-25 closed, stale: the spec window and the box PDF path were removed in #603.

- [x] **History says "From a spec" for a project made on a box** — the page's revision is labelled "From a spec: <document names>"; a box's shows "uploaded a zip", because Accept is `POST /api/projects/{name}/zip` and that route takes no label. Give the zip route an optional label (bounded, one line, audited as any save) and have spec mode send it.
  needs: nothing
  result: a spec-made project's first revision names its documents on a box as in the page
  est: 3h
  difficulty: low
  id: spec-zip-label
  asked: 2026-11-16
  by: claude-session
  audit: 2026-11-25 closed, stale: nothing says "From a spec" any more (#603). A zip made by Design is still recorded as "uploaded a zip"; a label for it is a new card if wanted.

- [ ] **A starter io.map a box loads: no screen tile for a number input** — `sim_projects::example_io_map` writes `in NAME int screen NAME` for a numeric input, and a box refuses that map (`IoMap::validate`: "a screen tile is something the box shows: an out signal"). Nothing loads io.map in the page, so it shows only when such a project reaches a box: an example with a numeric input copied by "Use this device", a download and upload. Spec mode on a box avoids it (`sim_projects::box_io_map` writes no line and the review lists the inputs that need a device); the page's examples and its spec mode still write the bad line.
  needs: nothing
  result: every io.map the page writes loads on a box, checked in a test with `helmsman_io::text::parse` and `validate`
  est: 3h
  difficulty: low
  id: starter-io-map-loads
  asked: 2026-11-16
  by: claude-session
  status: doing
  audit: 2026-11-25 low-hanging, in progress: worker on fix/starter-io-map-loads.

- [ ] **helmsman-adapter: `output_comes_back_as_input` read zeros where it had waited for the pattern** — Why it matters: the test waits until the adapter echoes the 32 output bytes back as input and then asserts on what it got, so a failure there is either a wait that returns before the data matches or an adapter that answers a match and hands back another packet; one of the two is wrong, and a scanner relying on that wait would act on stale input. Seen once, on CI's `box (macos-latest)` for #614 (2026-11-24, run 45465568201, crates/helmsman-adapter/tests/bin.rs:60): `left` was 32 zeros, `right` the pattern 1..=32, in 3.06 s. #614 does not touch the adapter or helmsman-proto, the job is not a required check, and the PR merged. Not reproduced locally (the full gate passed on the same commit, twice).
  needs: nothing
  result: the cause named (the wait in `IoConnection::wait_for_input_eq` or the adapter's echo), a test that fails on it every time, and the fix
  est: 2h
  difficulty: med
  id: adapter-echo-flake
  asked: 2026-11-24
  by: claude-session

- [x] **helmsman-dhcp: a client moves off its running lease (found by its own property test)** — Why it matters: the DHCP server's promise is that an address handed out in an ACK stays that client's while the lease runs; a block that is moved to another address mid-lease drops its fieldbus connection. On 2026-11-15 `cargo test --workspace` failed once in `crates/helmsman-dhcp/tests/hostile.rs`, `no_request_sequence_breaks_the_invariants`: "00-07-46-00-00-03 moved off its running lease", left 192.168.3.102, right 192.168.3.101. It is rare (the next three runs of that test passed), so CI can fail on it at random on any PR. To reproduce every time, add this line to `crates/helmsman-dhcp/tests/hostile.proptest-regressions`: `cc 32024c1c7cf224596c6941b14f9b5192e7b4dba694ca7a7b2831f9d34cc24c4b`. The shrunk case begins with a DHCPREQUEST from that client with ciaddr 192.168.3.100 and ends with its DHCPDISCOVER. Not fixed here: it was found while gating an unrelated change (the Devices tab's wired line), the crate is another task's, and the seed was left out of that branch so dev stays green until the fix lands with it, regression first.
  needs: nothing
  result: the seed is in the regressions file, the test passes with it, and the lease rule holds
  id: dhcp-lease-moves
  by: claude-session (iodd-meta)
  audit: 2026-11-25 done: 9253b69d (#559), with its test in helmsman-dhcp tests/server.rs.

- [ ] **A sensor's IODD as its metadata: from the library to the point it is wired to** — Why it matters: Nick, 2026-11-14: "ioLink is a core part of this but may fundamentally be different so let's focus on it for digital input and output device metadata more than anything else", and of sensors that day: "Their output? It should be deeply known ahead of time." A prox or a valve sits on a digital channel of a remote I/O block and is never spoken to; its vendor's IODD (every IO-Link device has one) says who makes it, its variants, how each connector is wired, whether it switches without a master, and what its signals and their states are called. Built: helmsman-devices `scripts/iodd-draft.py` drafts a library record from an IODD (`[identity.iolink]`, the order numbers, `[wired]` with each connector's wires, `[[signals]]` with `on` / `off` names, `[[settings]]` with factory values such as PNP and high active; helmsman-devices #33, #38), with five real records as drafts (ifm IF6123, O5D100, PN7092, KQ6005, SICK WTB4FP; IODDs from IODDfinder through `docs-fetch.py`). In Helmsman a map line names what is on a point, `in part_present bool rio di:3 wired=ifm/if6123`: the box keeps it, the Agent's `list_devices` gives it, and `devlib` finds the record by it, by an order number or by the IO-Link identity. The records are on the library's `main` (checked 2026-11-15), which the box and the playground read. Left, in order: (1) `wired.switching` for each record (which signal the switching wire carries with no master, normally open or closed): an IODD does not say and the Smart Sensor Profile leaves it to the vendor, so it comes from each data sheet with its page; none could be fetched on 2026-11-14 (ifm's pages are rendered by script, its asset service answered 404 and 403), so this needs the PDFs by hand or a vendor source that can be fetched; (2) done: the library builds `record.json` beside each wired device's `device.toml` (helmsman-devices #39), `helmsman_project::devrecord` reads it, the box serves its brief from `GET /api/devices/library?wired=VENDOR/MODEL`, and the Agent's `list_devices` brings it for every wired point; (3) done: the Devices and I/O tabs say what is wired under the point (vendor, model, kind; on/off names, factory settings and the draft note in the tooltip), on a box and in the playground (which reads the index and `record.json` itself), and "What is wired here…" on a point searches the library by the number on the label and writes or clears `wired=` on its map line (canvas `wired.rs`), tested at the panes' default widths; (4) `agent-names-io` uses it: a sensor picked from the library comes with what it is and what on means, and only its function is asked; the box has the tool for it, `list_library_devices` (search by vendor, model or label number; each match's `wired` value and brief), which MCP clients can call today; the relay's profile for it is `helmsman/v11` (agent-relay #38: v10 plus the tool and a prompt section on `wired=`, `wired_devices`, `switching_wire_carries` and recording it through `propose_io_map`; evals `wired-sensor-by-label` and `wired-sensor-not-in-library` pass live in every regime). Left of it: Nick's relay deploy, then a box opts in with `--relay-agent v11` (the box's default is still v7; moving it is a box change after the deploy); (5) the simulation's plant takes the sensor's kind and states from the record; (6) more records, sorted by what Nick's benches and customers use.
  notes: 2026-11-15, (1), a session's search (not yet checked page by page). ifm IF6123/IF6124/IG6615, KQ6005 and PN7092: the data sheets give pin 4 = OUT (and PN7092 pin 2 = OUT2) but only "NO/NC, PNP/NPN configurable" with no factory value, so the default has to come from the IODD's parameter defaults (already in the records' `settings`) or the operating instructions' factory table. ifm O5D100 instructions (11457730/00, p. 6): pin 4 = OUT1 normally open, pin 2 = OUT2 normally closed; its 2013 data sheet says DC PNP. SICK 1105505 is WTB4FP-32161120A00; its sheet (2024-03-28, p. 4) says pin 4 = QL1 light switching (HIGH with an object), pin 2 = dark switching, push-pull. 1122374 (same type family as 1222998) says the same; 1223017, 1123751 and 1123757 were not found by their own type code, and other WTB4FP variants differ on pin 2, so each needs its own sheet. Readable hosts: media.ifm.com/dam, automatyka24.pl, assets.alliedelec.com; ifm.com itself answers 403/404 to scripts.
  needs: the vendors' data sheets for (1); the relay deploy of helmsman/v11 for (4)
  result: a wired sensor is picked from the library by name, and the program, the I/O list and the Agent know what it is and what its signal means without anyone typing it
  id: iodd-metadata
  by: claude-session (iolink)
  audit: 2026-11-25 partly: (2) and (3) are done (record.json in helmsman-devices, canvas devices.rs). Left: (1) wired.switching per record, (4) the box default relay agent is still v7, (5) and (6).

- [x] **Order an IO-Link master transceiver board and one sensor** — Parked (Nick, 2026-11-14, later the same day: IO-Link is for the metadata of digital input and output devices more than anything else; see `iodd-metadata`). Nothing to order until a device has to be spoken to over IO-Link. Why it would matter then: the IO-Link master is built and proven against its own simulated device only (helmsman-iolink: wake-up, COM1/2/3 detection, startup, cyclic Process Data, retries, loss). Nothing has been on a wire, and the P4 needs a master transceiver between its pins and the sensor's C/Q line (24 V, the wake-up current pulse). Two candidates, facts from distributor pages on 2026-11-14 (the vendors' own pages timed out, so neither datasheet has been read): (1) Analog Devices MAX14819 / MAX14819A, a dual-channel master transceiver with L+ supply controllers, SPI, "integrated framers" or external UARTs, 9 to 36 V, 48-pin TQFN, evaluation kit MAX14819EVKIT; (2) ST L6360, a single master port (COM1, COM2, COM3), configured over I2C with the data on a USART, 18 to 32.5 V, evaluation board STEVAL-IOM001V1 (Arduino UNO R3 headers). My pick is the MAX14819 kit: two ports on one board and the framer in the chip takes the bit timing off the P4, which runs the scan and the network beside it. Not checked for either part: how the wake-up pulse is made, and the 3.3 V logic levels against the P4. Any IO-Link sensor with a published IODD will do as the first device; one with 16 bits of input or fewer avoids the interleaved mode the master does not have yet.
  needs: a 24 V supply on the bench; the part ordered; its datasheet read by a session before the driver is written (first step of `iolink-p4-line`)
  result: a transceiver board and a sensor on the bench, named on this card with the pins they are wired to
  options: MAX14819EVKIT | STEVAL-IOM001V1 | another part
  id: iolink-transceiver
  asked: 2026-11-14
  by: claude-session (iolink)
  audit: 2026-11-25 waits for Nick: a purchase. See the question iolink-still-wanted.
  decided: 2026-11-25 Nick: parked with iolink-node; nothing is ordered.

- [x] **IO-Link on the node: IODD import, the `iolink` device line, then the P4's line driver** — Parked behind `iodd-metadata` (Nick, 2026-11-14, later the same day: the sensor is wired to a plain digital channel and its IODD is its metadata; speaking IO-Link to it comes after, if at all). What is built stays on dev, tested, with nothing calling it. Why it mattered: Nick, 2026-11-14: everything should be reachable, IO-Link sensors included, through a master of our own on the edge node. Built so far (docs/ARCHITECTURE.md `helmsman-iolink`): the message codec, one master port and a simulated device as state machines without I/O, with the timing rules and a noisy line as tests. Also built: `iodd` reads a device's IODD file for its vendor and device id and its Process Data layout by name, and `pd` reads a value out of the octets by that layout (all 22 of the community's example IODDs read; no vendor's file tried yet). Also built in helmsman-nodecore's `iolink`: the `device NAME iolink port=N ...` line and its `pd:OFFSET[:uN|iN]` points, a `Port` that runs one on the master and gives the inputs by point and takes the outputs, and `Iodd::map_lines`, which writes those lines from an IODD; proven from an IODD to values on the simulated device. No firmware calls it, and the node's own `iomap::parse` still refuses an `iolink` line, since no node has a port. Left, in order: (1) try the reader on the IODD of the sensor that gets ordered, from IODDfinder; (2) `iolink-node-image`: a place for `int` points in the node's program image (today it is 4 bytes of bool bits a direction, for the Modbus block's points, so an IO-Link number has nowhere to land), and a map with no Modbus block (today a node's map needs exactly one); (3) `iolink-p4-line`: the transceiver driver on the P4 behind `Step` (wake-up, exchange with a timeout), `iomap::parse` taking `iolink` lines on a board with ports, the io task polling each `Port`, after `iolink-transceiver`; (4) the first real sensor, its capture kept as a test, and only then any wording a user reads (`iolink-membership`). Owed before a device with more than 2 octets of Process Data and M-sequence code 0: the interleaved TYPE_1_1 / TYPE_1_2 mode, which a master shall support (Table A.10 NOTE 3). Later cards: ISDUs (parameters, identity strings), Events, data storage, a second port, a box reading a node's ports.
  needs: nothing for (2); the sensor chosen for (1); `iolink-transceiver` for (3) and (4)
  result: a node's program reads an IO-Link sensor's Process Data by name from a deployed map
  id: iolink-node
  by: claude-session (iolink)
  depends: iolink-still-wanted
  audit: 2026-11-25 plan: 1) int points in the node image and a map with no Modbus block (software only); 2) iolink lines in iomap::parse on a board with ports; 3) the transceiver driver (needs the part); 4) a first real sensor, its capture kept as a test.
  decided: 2026-11-25 Nick: parked; the IODD-as-metadata route stands. Reopen when a customer asks.

- [ ] **Devices, not protocols: the device profile layer** — Nick, 2026-11-11: "the comms protocol isn't critically important since we can talk to everything. It's most important for the user to easily find, connect, and talk to the I/O on devices"; a simpler, more limited interface, not for power users. Decided in the interview that day: (1) the protocol is never shown or chosen: the box picks from the profile's order and falls back, and names it only in a failure and in details; (2) a device not in the library: the agent builds its profile from the vendor's file or manual, the user checks the channel names and can publish it; (3) channels first (named, typed, with units and a safe value), the standard face per kind (a drive's enable, speed, position, fault) when the MDrive is on the bench; (4) addresses and names: the box suggests, the user confirms (the I/O network may be IT's); (5) today's add-a-device form and typed points stay behind an Advanced switch; (6) timing is fully automatic, from the program's scan period and the profile's limits: the user sees healthy, slow or lost; (7) a profile is copied into the project when the device is added, updates are offered; (8) the Moxa E1213 proves it first: the same channels over Modbus TCP and EtherNet/IP. Sequence: (a) the profile format (identity per protocol, transports in order with their connection settings, channels with an address per transport, kind, limits) and its resolution to today's map lines, E1213 first, in helmsman-io with the library entry; (b) one Scan across protocols (ListIdentity, Modbus identification, PROFINET DCP) into one list; (c) add by click: the device named, its channels there with names, the profile in the project; (d) automatic timing and the healthy / slow / lost status; (e) the agent's build-a-profile path and publish; (f) suggested addressing; (g) kinds, with the MDrive.
  notes: 2026-11-11: the overnight plan for steps (a)..(e), a PR each, is PROTOCOLS.md at the repo root ("Run PROTOCOLS.md").
  notes: 2026-11-11: first slice of (a) built: `helmsman_io::profile` (the format, its checks, `Profile::map` per transport) and docs/devices/moxa-iologik-e1213.device, which agrees with both hand-written maps and drives the simulated unit by channel name. The device library already has a `device.toml` per device whose points are prose; the profile is the machine-readable form of that. Left of (a): the map binds a channel (`ch:DI-04`) instead of an address, behind a driver that opens the profile's transports in order and falls back by itself; the profile as a project file and a library entry.
  notes: 2026-11-12 later: (c) is #365 (add by click, the Advanced switch, the library route serving the profile; the library's half is helmsman-devices#2, which that repo cannot auto-merge: a manual merge, or repo-baseline on it first). Until #2 merges, a real box with the default library sees no profile on the E1213 and offers Advanced instead of Add. Left of the plan: (e) the relay half merged (agent-relay #20), its box half is card `device-profile-propose-box`; the health word on the Devices tab row and in the Monitor (the hover says it).
  notes: 2026-11-12: (a) landed as #363 (`device:NAME` lines, `ch:` points, `ProfileDriver` falling back between transports, `NAME.device` a project file, `/api/devices` `transport`); (b) as #361 (one Scan, `found`); (d)'s box side as #364 (`cycle_ms=A..B` limits, the link paced by its modules, healthy / slow / lost on `/api/devices`). Left: (c) add by click and the words on the Devices tab and the Monitor, waiting while another agent is in the canvas; (e) the agent's build-a-profile path: its box half is the card `device-profile-propose-box`; (f) and (g) as before. Assumed without asking: pacing applies to `device:` devices only (plain lines keep `poll_ms` or 50 ms).
  result: a user finds a device, clicks it, names it and uses its channels by name; no protocol, address, byte order, cycle time or vendor file is asked of them
  est: 3w
  difficulty: high
  id: device-profiles
  asked: 2026-11-11
  by: claude-session
  audit: 2026-11-25 partly: (a) to (e) landed (#363, #361, #365, #364, publish.rs). Left: (f) suggested addressing, (g) the standard face per kind.

- [x] **Device configuration: the agent's read-only tools** — the agent (and MCP) read a device's configuration, never change it: a `read_device_config` tool (facts, the export, the drift from the saved `.devconf`) through `api::devconf::config_view`, wrapped in the `<box-data>` envelope like every plant text. No set, restart, import or password tool, and the password is in no tool result (`ConfigView` has no field for it). The box's engineer gate stays the only way a unit is changed; the agent may propose a setting in words.
  needs: nothing
  result: "why does this Moxa drop its outputs when the controller stops?" is answered from the unit's real watchdog and safe values
  est: 1d
  difficulty: low
  id: devconf-agent-tools
  asked: 2026-11-13
  by: claude-session
  notes: 2026-11-13: built as #374 (draft). Its last CI run failed in helmsman-plc tests/agent.rs:2152 `an_unreachable_relay_still_gives_the_boxs_explanation_then_an_error`; not looked into yet. Then `gh pr ready 374 && gh pr merge 374 --auto --merge` (classifier-denied once).
  audit: 2026-11-25 done: #374 merged; read_device_config in helmsman-agent/src/tools.rs.

- [ ] **The library says what has not been proven on a unit, entry by entry** — Nick, 2026-11-17: an entry is finalized by the first user who implements the device, so until then the library has to flag that it has not been proven out and exactly what has not. Today that is spread over three places a reader has to know: no `checked_on` means "from documents", a point's `source` may end "confirmed on a unit", and `[[questions]]` and the README's "Not run" hold the rest. The two entries of 2026-11-16 show the gap: the IMPACT67 55089 has `checked_on` although no channel was ever seen switching, and the Lexium MDrive has it although its output layout as delivered is unchecked. Shape to build (my recommendation, yours to change): a `[proof]` table in `device.toml` with one line per thing a unit can prove (identity, connection, each point group, outputs on a lost connection, each protocol), each `proven` with date and firmware or `unproven` with why; `checked_on` stays as the date of the last unit contact and stops meaning "done"; `build-index.py` carries a summary (`proven: 3 of 7`) and the list of what is open into `index.json`; `build-review.py` already walks every claim and gets the same marks; the site's library table and check page show "proven on a unit / not yet: channels, outputs on a lost connection" instead of one date; AGENTS.md step 5 tells the first implementer's agent to turn `unproven` lines into `proven` ones in the pull request that reports them. Backfill: all 45 entries, most of them entirely unproven, generated from what each file already says.
  needs: later (Nick, 2026-11-17); nothing else
  result: anyone picking a device sees at a glance what a unit has shown and what is still only on paper, and the first implementer's pull request closes those lines one by one
  est: 2d
  difficulty: med
  id: library-unproven-flags
  asked: 2026-11-17
  by: claude-session
  depends: library-proof-shape
  audit: 2026-11-25 plan (in helmsman-devices): 1) an optional [proof] table in the schema, checked by build-index.py --check; 2) "proven n of m" and the open list in index.json; 3) build-review.py carries the marks; 4) backfill the entries from what each file says; 5) the site table; 6) AGENTS.md.
  decided: 2026-11-25 Nick: an optional [proof] table in device.toml.

- [ ] **Configure a device by talking to an agent, not through the vendor's tool** — Nick, 2026-11-17: come up with ways to configure devices agentically in the device library, casually and conversationally, so nobody has to use the vendor's software. What exists: helmsman-devconf reads and writes a Moxa E1200's settings through its web console, with the box's engineer gate as the only way a unit is changed (`devconf-agent-tools` is its read-only agent side). What the bench of 2026-11-16 showed is reachable without any vendor tool: the MDrive answers MCode on TCP 503 beside EtherNet/IP (every register read; `SL 0` stopped it), and its EtherNet/IP assembly mapping, protocol choice and "Halt Motion" setting live behind Novanta's Configuration Utility, which talks to that same port; the IMPACT67 takes its channel setup as configuration data in the Forward_Open (built: `config_data=`), and its address mode and parameterization mode through its web server; every EtherNet/IP device has the TCP/IP Interface object for its address. Shape to explore first, as an interview: a `configure` recipe per library entry (which channel reaches the setting: a CIP attribute, a web form, a vendor text port; what each setting is called in the vendor's words and in plain words; what is safe to change live and what needs a restart), read by one generic agent tool that proposes a change in words, shows the before and after, and applies it only through the engineer gate; settings the recipe cannot reach are listed as "needs the vendor's tool" so the gap is visible. First targets, because the bench has them: the MDrive's mapping page over MCode (capture what the utility sends, a hand-off `tcpdump`), the IMPACT67's parameterization mode, any device's IP address over CIP.
  needs: later (Nick, 2026-11-17); an interview on the shape before anything is built; the MDrive and the IMPACT67 on the bench
  result: "make pin 4 of every port an output" or "put the drive's mapping back to the default" is said to the agent and done, with the change shown first and recorded
  est: 1w
  difficulty: high
  depends: devconf-agent-tools
  id: agentic-device-config
  asked: 2026-11-17
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for Nick: the interview on its shape, then the MDrive and the IMPACT67 on the bench. Its dependency devconf-agent-tools is done.

- [ ] **`enip-io`: pick which of an EDS's exclusive owners to open** — Why it matters: from an EDS the driver opens the first exclusive owner. The IMPACT67 55089's EDS has four (without and with diagnosis, each for two configuration instances), and the first has no diagnosis bytes, so `eds=` alone cannot reach the short-circuit and overload bits; the library's profile works around it by giving the sizes itself. Shape: `connection=N` (the EDS's `ConnectionN`) beside `exclusive-owner` and `input-only`, refused when N is not an owner of the kind asked for, with the Devices tab's import offering the EDS's own connection names.
  needs: later (Nick, 2026-11-17); nothing else
  result: a device with several owning connections is driven from its EDS alone
  est: 4h
  difficulty: low
  id: enip-eds-owner-choice
  asked: 2026-11-17
  by: claude-session
  audit: 2026-11-25 low-hanging, next wave: parse connection=N in helmsman-io/src/enip.rs (around line 671) and test with the IMPACT67 EDS, which has four owners.

- [ ] **A DHCP server on the box's device port: a block out of the box needs no vendor tool** — Nick, 2026-11-13: users won't preconfigure devices themselves, and PROFINET-first is out (EtherNet/IP and Modbus TCP are what plants run). Turck TBEN blocks ship at rotary position 600 (PGM-DHCP, a DHCP client until given an address) and Murrelektronik IMPACT67 E at 000 (DHCP/BOOTP, helmsman-devices `devices/murrelektronik/impact67-e-series.md`), so a box that answers DHCP on its own I/O network gets them addressed with nobody touching the device. Shape: a DHCPv4 server (RFC 2131 subset: DISCOVER/OFFER/REQUEST/ACK/NAK/RELEASE, plus BOOTP replies for blocks that only send BOOTP), std only like the other protocol crates, a pure state machine with the socket outside it (the `helmsman-profinet` pattern), a fixed lease per MAC kept in the state dir, addresses from a pool the project settings name. A new device's lease shows up as `found` in the Scan (ListIdentity from that address, matched to the library by `cip=`), and the map line the box suggests is `mac=` (`helmsman_io::resolve`), so the lease can move without the project changing. Off by default and per interface: decision (4) of `device-profiles` says the I/O network may be IT's, and a second DHCP server on a plant network is an outage. Before it answers anything it listens for another server's OFFER (sends one DISCOVER of its own) and refuses to start if one exists, says so on the Devices tab, and stops if one appears later. Tests: hostile packets (truncated, option overflow, wrong magic cookie, a flood of DISCOVERs with random MACs against pool exhaustion), the rogue-server check, lease persistence across restarts, and one bench script that gives a TBEN out of the box its address (auto mode refuses LAN listeners: the bench run is Nick's).
  notes: 2026-11-13: the server itself is crate `helmsman-dhcp` (feat/dhcp-server): the message, the server state machine, the lease file and `guard` (the probe and the other-server check), with property tests; the property test found the pool's take-back handing out another client's reservation, fixed with its regression. Left: the box half (UDP 67 on the named interface, the probe before serving, the lease file in the state dir, the settings and the Devices tab's word for a refused start), then the Scan's `found` from a lease and the `mac=` suggestion.
  notes: 2026-11-13: Nick: a box setting, not a project's ("a personal preference that will be global to all devices"). The box half is `--dhcp IFACE` (`--dhcp-pool A-B`, default .100-.199 of a /24; `--dhcp-lease S`, default a day): helmsman-plc `dhcp`, both sockets tied to IFACE (helmsman-l2 `bind_to_interface`), a 4 s probe before serving, a stop when another server is heard later, leases in `<state-dir>/dhcp/leases`, `GET /api/dhcp`; tested on loopback. Left: (1) deploy/helmsman-plc.service has `CapabilityBoundingSet=` empty and says every port is above 1024, so `--dhcp` on the Pi needs `CAP_NET_BIND_SERVICE` granted there (Nick's deploy line); (2) the bench run against a TBEN out of the box (Nick: auto mode refuses LAN listeners); (3) the Devices tab's word for a refused or failed server, and leases in the Scan's `found` with a `mac=` line suggested; (4) reservations from the running project's `mac=` device lines.
  needs: nothing for (3) and (4); the bench run needs a block out of the box (the Murrelektronik 55086 ships on DHCP/BOOTP, card `murr-block-bringup`; a TBEN-S2-8DXP does too) and is yours (auto mode refuses LAN listeners)
  result: a Turck block wired to the box's device port gets its address, shows up in the Scan and is added by click, with no vendor tool, no laptop and no rotary switch touched
  est: 3d
  difficulty: med
  id: box-dhcp-server
  asked: 2026-11-13
  by: claude-session
  audit: 2026-11-25 partly: the crate, --dhcp, GET /api/dhcp and leases landed (#385, #559). Left: Devices-tab text for a refused server, leases shown in the Scan, reservations (main.rs passes an empty map), and the Pi service line and bench run, which are Nick's.

- [ ] **Publish a checked device profile to the library: the token, the deploy, the first real one** — built 2026-11-18 (helmsman PR for the box route and the Devices tab's "Publish to the library…", agent-relay PR #51 for the tool `publish_device_profile` and profile `helmsman/v13`), both drafts; nothing has met the real GitHub, only a scripted one. What it does: an engineer saves a profile, the tab offers the button, the box sends the saved file to the relay, the relay opens a DRAFT pull request into helmsman-devices `dev` (`NAME.device` beside the device's `device.toml`, which gains `profile = "NAME.device"`, and `index.json` gaining `profile`, byte for byte what `build-index.py` writes; drafts because the library's auto-merge merges non-draft PRs from you). Only for a device the library's index already lists (44 of 45 entries have no profile yet); one it lacks is refused with the pointer to a "New device:" issue. Left: (1) your steps, in order: review and merge the relay PR (a new template parameter `GithubSecretName` and one IAM statement reading that one secret), `./scripts/deploy.sh` there, run `scratch/31-github-publish-token.sh` (prints the token settings to make, takes it at a hidden prompt, stores it with `scripts/put-github-key.sh`, checks it sees helmsman-devices and not another repository), merge this PR and start a box with `--relay-publish-agent` left at its default `helmsman/v13` (or `--relay-agent v13`); until the token is stored every publish answers "publishing is not set up" and nothing else changes; (2) the first real publish, from the E1213 or an E1210 profile, to see the PR, its CI (`build-index.py --check`) and the draft; fix what the real GitHub shows and keep it as a test in the relay; (3) a device the library lacks: a minimal folder (`device.toml` with vendor, model, kind and profile only) is a small step on the relay, a decision on what the library accepts without an EDS or citations (its AGENTS.md says every field is read or seen); (4) the tool is listed to the model too (the route needs the profile to list it, and a model's call does nothing); a tool flag that keeps it out of the model's list is a relay-core change.
  needs: you: the steps in (1); a unit-checked profile is not required (the PR says no unit has proven it)
  result: the library grows by a pull request each time a user maps a device it has no profile for
  est: 2h of yours, then 2h for what the first real publish shows
  difficulty: low
  id: device-profile-publish
  asked: 2026-11-13
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for Nick: merge and deploy the relay PR, run the token script, make the first real publish.

- [ ] **PROFINET from the node: a real device, then the node image** — Why it matters: only once a device has to be reached over PROFINET. Parked with `profinet-raw-access` (Nick, 2026-11-13: EtherNet/IP and Modbus TCP first; PROFINET when a customer or a device needs it). the P4 runs the box's PROFINET controller (docs/EDGE.md "PROFINET on the P4"): raw frames tapped beside embassy-net in the Ethernet driver wrapper, the RPCs on a UDP socket, a `profinet` image that brings one device up and reports. On the board it boots, looks for its station every second and answers ping meanwhile; the node image with the same wrapper kept its API and its Modbus link. Nothing answered it: no PROFINET device is on the bench, and a capture to see its frames needs sudo. Left: (1) the first connection to a real device from the P4 (`NODE_BOARD=p4-eth PN_STATION=... PN_SLOTS=... scripts/node-flash.sh profinet`), with the cycle's jitter and late frames measured as the EtherNet/IP run was; (2) a `profinet` device line in the node's I/O map (helmsman-nodecore's `iomap`) so the `node` image masters a PROFINET device from a deployed map, as it does a Modbus block.
  needs: a device that has to be reached over PROFINET
  result: a node masters a PROFINET device at an 8 ms cycle with its late frames counted, from a map deployed over its API
  est: 3d
  difficulty: med
  id: node-profinet
  asked: 2026-11-11
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for a real PROFINET device on the bench.

- [ ] **PROFINET IO controller: from the simulated device to real ones** — Why it matters: only once a device has to be reached over PROFINET. Parked with `profinet-raw-access` (Nick, 2026-11-13: EtherNet/IP and Modbus TCP first; PROFINET when a customer or a device needs it). built 2026-11-10 night (docs/ARCHITECTURE.md `profinet`; crates helmsman-profinet, helmsman-l2, helmsman-io's `profinet-io`): DCP, the Connect / Write / PrmEnd / application-ready / Release RPCs, cyclic frames with a watchdog both ways, alarms acknowledged, a `slots=` device line, bench tools (`profinet_scan`, `profinet_watch`, a pcap of a startup). Proven against our own simulated device (startup, loss, refusals, hostile bytes as properties) and read back field for field by scapy's dissector. Not proven: one frame on a real wire. What is left, in the order the bench will ask for it: (1) the first real startup (`mdrive-bringup`), fixing what it shows and keeping its capture as a test; (2) GSDML import, so `slots=` comes from the vendor's file as `eds=` does for EtherNet/IP (the Devices tab's Import device…); (3) alarms and the device's diagnosis shown on the signal and in the Monitor instead of only kept; (4) record reads (I&M0 for the device's identity page); (5) RPC fragmentation, for devices with many submodules; (6) the default build: `helmsman-plc`'s `profinet-io` feature goes into `default` once a device has passed; (7) a Linux run (packet socket; the Pi or CI with a veth pair).
  needs: a device that has to be reached over PROFINET; then `profinet-raw-access`
  result: a box masters a real PROFINET device from a map an engineer wrote from the GSDML, and says in words what is wrong when it does not come up
  depends: profinet-raw-access
  est: 1w
  difficulty: high
  id: profinet-controller
  asked: 2026-11-10
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for a real PROFINET device; GSDML import is not built and is best done once a device shows what it needs.

- [ ] **Canvas: match box answers to requests, one buffer state machine** — from the 2026-11-08 interface review. Every wasm request to the box is a parallel fetch with no request id or generation; `BoxProjects::answer` matches by call name and reads live buffer state at answer time, and the native tests run requests one at a time so ordering is never exercised. The data-loss fixes of PR fix/box-projects-correlation patch the cases found (NewFile empty buffer, dropped save answers, deploy during save, Replace target); this card is the shape that stops the class: an in-flight table keyed by request id holding the sent text and the project generation, answers resolved against it, and one `Buf` state enum (Clean, Dirty, Saving{sent}, Conflict) shared by save, rename, create and deploy, with a test harness that answers requests out of order.
  result: a reordered or late answer can never make a buffer clean, empty or stuck; box_projects_tests has an out-of-order answer suite
  est: 2d
  difficulty: high
  id: canvas-request-correlation
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan: 1) a Buf enum (Clean, Dirty, Saving, Conflict) behind the existing flows; 2) a request-id table with a project generation; 3) save, rename, create and deploy go through it; 4) an out-of-order answer test harness.

- [ ] **Canvas: pump the live source every frame, independent of the screen** — review 2026-11-08. `BoxView::tick` runs only from the Editor and Monitor branches, so each new screen (Proposals, "Simulate in this browser", a future one) has to remember to drain the transport or the inboxes grow and the link goes stale. fix/remote-link adds the tick and coalesces events; this card moves the design: App owns one per-frame pump for the live source, screens only render, and the transport inbox keeps latest-per-(stream, module) by construction.
  result: no screen can leak or stall the box link; a test opens every screen and asserts the link stays current
  est: 1d
  difficulty: med
  id: canvas-pump-every-frame
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan: 1) one App::pump per frame for the live source, screens only draw; 2) the inbox keeps the latest event per stream and module; 3) a test that opens every screen and asserts the link stays current. About a day; it touches the frame loop.

- [ ] **Box streams carry a boot id and a sequence; the canvas reconnects by reason** — review 2026-11-08. The canvas compares times across the box scheduler clock, the trace scan counter, egui time and history fetches, and `Msg::Failed` carries no reason, so a deliberate 600 s stream end, a 503 slot refusal, a 401 and a dead network all look like "lost the box". fix/remote-link patches the worst cases (immediate reconnect after a healthy stream, a values watchdog, trend reset on clock reset). The shape: the values stream and `/api/modules` carry a boot id and a monotonic seq; `BoxState` keeps one epoch and per-stream last-seen marks from it; reconnection learns the status with a fetch probe, backs off with jitter, and resumes from seq.
  notes: the box API change is small (api/mod.rs values event, http.rs)
  result: a box restart, a slot refusal and a half-open socket each show as what they are, and the trend, Monitor and diagram never mix epochs
  est: 2d
  difficulty: med
  id: box-stream-epoch
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan (additive API): 1) a boot id and sequence on the values event and /api/modules; 2) Failed(Stream, Reason) with 401, 503, ended, network; 3) a probe, backoff with jitter, resume from the sequence; 4) the page's epoch from the boot id.

- [ ] **Agent acceptance needs a signal the model does not write** — review 2026-11-08. The model writes the code and the scenarios that accept it, and the checker parses prose and Markdown (fences, `Why:`, `replaces`, an elision phrase list). fix/verify-gate closes the holes found (asserts must be exercised, red-before/green-after, a replacement needs the old test red, truncated answers fail). The shape beyond that: structured output from the model (a tool or JSON schema with why, file_text, scenarios[{name, requirement, source, replaces?}]) so the prose parse and the elision heuristics go away; mutation checks (the scenario must fail when the changed output is inverted); and edits anchored to a base hash instead of whole-file rewrites as the trust unit, with the diff shown before the write.
  needs: a decision on structured output for the relay profiles (agent-relay repo)
  result: a vacuous or truncated answer cannot be written; the engineer sees a diff, not a sentence
  est: 3d
  difficulty: high
  id: agent-acceptance-signal
  asked: 2026-11-08
  by: claude-session
  depends: agent-structured-output
  audit: 2026-11-25 plan: 1) a relay profile with a schema (agent-relay); 2) the box and the page parse it; 3) drop the prose heuristics; 4) edits anchored on a base hash, diff shown before the write; 5) a mutation check of scenarios.
  decided: 2026-11-25 Nick: the answer comes back under a JSON schema (structured output). Step 1 therefore starts by checking that the model the relay runs honours a schema, and names the fallback if it does not.

- [ ] **One agent core for the page and the box; apply and undo as a transaction** — review 2026-11-08. `playground_view` and `box_view` each implement Run, Check, Apply and Undo with different settings and sizes, message caps, pre-save of dirty edits, undo revision source and accepted stop reasons, and they have drifted. `Run` has no base snapshot, file set or per-file hashes, so Undo restores the whole project (other files' later saves included) and the box path overwrites unsaved local edits. Shape: one `AgentRun` owning Run/Check/Apply/Undo; hosts implement `snapshot()`, `settings()`, an atomic multi-file `write_files(base, files)` and `restore(files)`; Apply refuses if a touched file moved since Send; Undo is the exact inverse over those files only.
  result: one code path, tested once against both hosts; Undo never touches a file the Agent did not write
  est: 2d
  difficulty: high
  id: agent-run-core
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan: 1) an AgentRun and a host trait (snapshot, settings, write_files, restore); 2) port the page; 3) port the box; 4) Apply refuses when a touched file moved and Undo covers only what it wrote; 5) one test suite run against both hosts.

- [ ] **The page simulation and the sim project store share the box's rules** — review 2026-11-08. The page is a second runtime that drifted from `helmsman-plc` runtime.rs (cold deploy vs state-carrying hot swap, stop and fault leaving outputs vs zeroing, a 500-scan catch-up burst vs skipped periods, a fixed 10 ms vs the declared period, a 64 KB source cut vs the project limits), and `SimProjects` is a second project store that drifted from the box API (case rules, import overwrite, history keyed by name). fix/sim-semantics and fix/desk-and-sim-store patch the cases found. Shape: a shared library function for deploy, swap, stop and fault semantics both runtimes call, one project-rules trait both stores implement, and one conformance suite run against both.
  result: "works in the page, differs on the box" stops being a bug class; the conformance suite is the contract
  est: 3d
  difficulty: high
  id: sim-shares-box-rules
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan: 1) a conformance suite of what the box already does (hot swap, stop zeroing, period skipping); 2) pull the shared runtime rules into one crate; 3) move the page onto it; 4) a rules trait for both project stores.

- [ ] **A Transport trait with a native in-memory implementation; web.rs tested** — review 2026-11-08. web.rs (about 800 lines of wasm glue) has no test coverage: three copies of the stream-read loop, the same char-boundary `String::truncate` panic twice, `get()` without no-store or a timeout, inconsistent error handling. Shape: one `Transport` trait implemented by web.rs and by a native in-memory transport that can inject stalls, 500s, non-UTF-8 and oversized bodies and reordered answers; one shared stream-reader helper; remote.rs's retry and silence logic then runs under `cargo test`.
  result: every wasm-only path has a native test; a transport bug is a failing test, not a field report
  est: 2d
  difficulty: med
  id: canvas-transport-trait
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan: 1) a shared stream-reader helper with the truncate fix and a native test; 2) a Transport trait that web.rs implements; 3) an in-memory transport with stalls, 500s, bad UTF-8 and reorder; 4) remote.rs retry tests on it.

- [ ] **Canvas: hit keys as an enum, Chrome split, one Command dispatch** — review 2026-11-08. `Chrome` is the persisted layout format (a hand-rolled k=v string), the UI toggles, the test registry (`hits: Vec<(String, Rect)>`) and a cross-frame channel at once; the `std::mem::take(&mut self.chrome)` pattern hands a default Chrome to anything that reads it meanwhile. Hit keys are free strings the tests and the tour depend on, so a rename silently breaks them. Hotkeys, header glyphs and menu items are three dispatch paths into `App::act` with scattered screen and source guards (fix/shell-hotkeys-layout adds the missing gates). Shape: a versioned `Layout` (persisted), a `FrameOut` (hits, diagram rect), a `HitKey` enum, and one `Command` enum with `enabled(&App) -> Result<(), why>` that menus, glyphs and hotkeys all consult; a tour test that visits every step and asserts its anchor was drawn.
  result: a removed or renamed control is a compile error; one place says whether an action is allowed and why
  est: 2d
  difficulty: med
  id: canvas-chrome-split
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan: 1) a HitKey enum in place of free strings in tests and the tour; 2) a versioned Layout split out of Chrome; 3) a Command enum with enabled(&App); 4) hotkeys, glyphs and menus through it; 5) a tour test that every anchor was drawn.

- [x] **The Agent on a box** — left out of agent-scoped-edits (#207): a box's canvas has no Agent tab (BoxView::tabs() is I/O and Requirements; a ui_test pins it), its page's CSP is connect-src 'self', and the box's agent runs through the relay as proposals (POST /api/chat). Give the box an Agent tab that talks to /api/chat, with the pages (#187), iterations (#201), the requirement gate (red_tests) and the edit scope (`scope::apply` takes plain files, so it is reusable) on the box side.
  needs: the relay deployed (relay-deploy) to try it against a real model; the rest builds against the relay fixtures
  result: the same agent workflow on a box as in the playground, every change still a proposal through the approval policy
  est: 3d
  difficulty: high
  id: agent-on-box
  asked: 2026-11-03
  by: claude-session
  notes: first cut in #217 (Agent tab over POST /api/chat, streamed, proposals); the box asks for v7 (#218), which the relay serves. Left: pages and iterations on the box, scope::apply box side.
  audit: 2026-11-25 done: the box page has the Agent tab (box_agent.rs, box_view.rs; #217, #218).

- [ ] **Box client for relay protocol v2 (helmsman/v8)** — the relay now keeps the conversation (agent-relay PR #7; docs/relay-protocol-v2.md and docs/harness.md there): the box opens a session (`POST /v2/sessions {"agent":"helmsman/v8"}`), sends only new input per step (`user`, `tool_results`, `continue`), runs the client tool calls a step's `harness_yield` names with the existing executor and `<box-data>` envelope, and renders text from the forwarded stream. The relay compacts the context (EoCC), runs doc_grep/doc_read/history_* and fetch_document itself (a fetched EDS arrives as `harness_document`; GET /v2/docs/{id}/raw saves it to the project), so the box drops its 900 KiB history cap and its per-turn model budgets. Keep v7 on v1 as the fallback until v8 has run on the bench; the offline fallback and relay-health watcher stay. Vendor docs/relay-protocol-v2.md beside the v1 file.
  needs: agent-relay PR #7 merged and deployed (relay cards relay-eval-live, relay-deploy-v2)
  result: `--relay-agent v8` runs chat on relay-held sessions; long manuals and long sessions stop hitting body and history limits
  est: 2d
  difficulty: med
  depends: agent-on-box
  notes: 2026-11-17, built (PR on feat/relay-v2-client): the box's v2 client (helmsman-agent session.rs and the v2 loop in agent.rs; docs/relay-protocol-v2.md vendored from agent-relay 26cd6dc9) and `--relay-agent v8`..`v11` on relay-held sessions; the chat route asks `GET /v1/agents/{agent}` once for the profile's `protocol` (a bare `v8` is retried as `helmsman/v8`), v7 and older stay on v1 and the default is still v7. Documents the relay stored are saved as drop-ins; a lost session is reopened once; the owed step goes first on the next message. Tested against a loopback relay scripted from the protocol doc (tests/session.rs, tests/agent.rs): no real relay has answered this client. Left: a bench run against the deployed relay (Nick's, relay card relay-deploy-v2), then v8 as the default; the model is not told a saved document's drop-in id; the MCP `fetch_document` route is still v1's; sessions are not deleted when a chat is dropped (7 days on the relay). agent-relay PR #7 merged 2026-11-07; spec mode (#475) went through v7's chat.
  id: relay-v2-client
  asked: 2026-11-05
  by: claude-session
  status: blocked
  audit: 2026-11-25 built (#506); waits for Nick's bench run against the deployed relay before v8 becomes the default.

- [ ] **Modbus identification probe (FC 43/14) for discovery and identity** — left out by the device-by-identity and discovery workers (2026-11-02 night): `cip=` identities and the ListIdentity sweep cover EtherNet/IP; a Modbus-only device is found today only by `mac=` once the host has seen it. Add Read Device Identification (FC 43, MEI type 14: VendorName, ProductCode, MajorMinorRevision) to helmsman-io's modbus driver, a bounded probe of the ARP table's hosts on 502 in `POST /api/devices/discover` when asked (`?modbus=1`), a `modbus=VENDOR:PRODUCT` identity endpoint resolved by that probe, and the library lookup by it (`identity.modbus` in device.toml exists). Cite the Modbus Application Protocol spec section in docs/SOURCES.md.
  needs: a Modbus device that answers FC 43/14 to test on (does the E1213? check on the bench; the simulator answers it either way)
  result: a Modbus-only block is found and matched like an EtherNet/IP one
  est: 1d
  difficulty: med
  id: modbus-identification
  asked: 2026-11-02
  by: claude-session
  notes: box side merged in #219 (FC 43/14 codec and probe, modbus= endpoints, discover ?modbus=1, spec §6.21). Left: the Devices tab asking ?modbus=1 and showing the list; identity.modbus in helmsman-devices build-index.py; a Modbus-identity query on GET /api/devices/library; whether the E1213 answers FC 43/14 (unchecked: it was off the LAN 2026-11-04).
  audit: 2026-11-25 partly: the probe, the modbus= identity and the library lookup landed. Left: helmsman-devices build-index.py writes only identity.enip, and whether the E1213 answers FC 43/14 (bench).

- [ ] **Help mode on the relay: a profile that reads the repo** — in ../agent-relay (SAM deploy, deploy-chain edits as a draft PR): helmsman/help-v1 (agent-relay #55, 2026-11-18) answers from the box data and help pages alone; this is the next id (helmsman/help-v2, and the box's `--relay-help-agent` default moved to it), whose model also gets read-only tools over JimothyJohn/helmsman@dev through the GitHub API: list a directory, read a file (size-capped), search code. The repo is PUBLIC (checked 2026-11-07), so no token is needed to start; a fine-grained read-only token in Secrets Manager raises the rate limit (60/h unauthenticated). The system prompt: diagnose for a third-party user, cite the files it read, end with the Markdown issue; docs/ARCHITECTURE.md and docs/help are the starting points. Tests against recorded GitHub responses; refuse paths outside the repo.
  needs: a session in ../agent-relay (this repo's sessions cannot write there); Nick's go for the deploy
  result: the relay serves the Help profile the box's Help mode asks for
  est: 2d
  difficulty: med
  id: help-mode-relay
  asked: 2026-11-07
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for a session in ../agent-relay (help-v2 does not exist) and Nick's deploy.

- [ ] **Warnings on a box: while typing, and in the two answers that leave them out** — #346 shows the checker's warnings and the build's non-blocking problems in the check answer, the CLI and the canvas. Three places were left: (1) on a box the canvas shows them only after a check or a Deploy, where the browser simulation shows them as you type (the box view makes no check call per edit, and one per keystroke would fill the audit log: check on a pause in typing, or a check that is not audited); (2) `POST /api/proposals/check` (a draft's check) does not list them; (3) the deploy route's own answer does not, only the check before it. Also pin with a test what #345 and #346 do together: a kept file's "not used by this release" entry arrives in the check answer's `warnings` and as a row in the canvas.
  result: an engineer on a box sees "RETAIN here is not kept yet" while typing, and a proposal's check says the same as the project's
  est: 3h
  difficulty: low
  id: warnings-box-gaps
  asked: 2026-11-10
  by: claude-session
  status: doing
  audit: 2026-11-25 low-hanging, in progress: worker on feat/warnings-box-gaps does (2), (3) and the pinning test. (1), a check on a typing pause, stays open.

- [ ] **Look at a project with kept files in the canvas; fix the box's start line** — two small things seen during the 2026-11-10 run and checked by nobody. (1) #345 keeps a file this release has no use for (pump-curves.json, a newer release's file); the canvas code filters on Structured Text files wherever it was read, but the Files list of a box project that holds one was never looked at on screen: open one (`BOX=1 make canvas-smoke` with a kept file in the project, or a ui_test), and decide whether the file is listed, greyed with its "kept, not used" reason, or left out. `PUT` of a single file still refuses an unknown name, which is right while the canvas cannot edit one. (2) The box prints "API on http://...  (reads open; changes need the token)" at start; since read-lock (#323) every read needs a session, so the line is wrong.
  result: a kept file is shown the way that was chosen, with a test; the start line says reads need a sign-in
  est: 2h
  difficulty: low
  id: kept-files-in-canvas
  asked: 2026-11-10
  by: claude-session
  status: doing
  audit: 2026-11-25 low-hanging, in progress: worker on feat/kept-files-in-canvas (listed, greyed, with the reason; start line reworded).

- [ ] **A Studio box with the device links off, beside the P4** — your answer to `try-box-keep-or-retire` (2026-11-13): the Studio is for app and interface work, the P4 for runtime and embedded work, and the two should not both be on the devices; but the Studio box should still be programmable with everything except device comms, so it can run while the P4 has them. Today a box opens every link its project's map names, and the only way off the E1213 is to stop the box. Build a box setting (`--links off`, and the same as a switch an engineer can flip): the program, the canvas, the agent, proposals, history and the Monitor all work; no link opens; each input reads as the simulation would give it (the same plant model the page's simulation runs) and outputs go nowhere; the Devices tab and the Monitor say "device links off on this box" so nobody reads simulated values as the plant's. Then the try box runs that way during P4 work, and CLAUDE.md's try box entry says so.
  needs: nothing
  result: a P4 run against the E1213 no longer needs the try box stopped; the Studio box stays up for interface work meanwhile
  est: 1d
  difficulty: med
  id: studio-box-no-io
  asked: 2026-11-13
  by: claude-session
  depends: links-off-switch
  audit: 2026-11-25 plan: 1) --links off, the box opens no links; 2) inputs from the simulation plant model, outputs go nowhere; 3) a banner on the Devices tab and the Monitor; 4) the CLAUDE.md try-box entry.
  decided: 2026-11-25 Nick: --links off at start, no switch in the UI.

- [ ] **Devices tab: glyph buttons with their names, I/O grouped and collapsed, a search** — your answer to `glyphs-devices-buttons` (2026-11-13): "let the user choose through a subset of icons but still show their names. Collapse the I/O registers by default and group ins, outs, and datatypes so that it's easier for the user to organize. A simple pattern search feature for an input in real-time would be nice as well." Read as: (1) Scan, Edit, Delete and Use this device become `glyphs::action` buttons that keep their word beside the glyph (not glyph only, as the Agent tab's are); (2) a device's points start collapsed, and open grouped as Inputs, Outputs, then by data type inside each (BOOL, INT, REAL, ...), each group with its count; (3) a filter field at the top of the tab that narrows the list as you type, matching point names, channels and devices (plain substring and `*` wildcards). Assumed and to check with you on the PR: "choose through a subset of icons" means the glyph library's existing set, not a picker where the user assigns icons to devices. Panel width rules apply (`every_tab_leaves_the_panels_their_width`, a test at the default width).
  needs: nothing
  result: a device with 40 points opens as two short groups, and typing "DI-0" shows only those points
  est: 1d
  difficulty: low
  id: devices-tab-glyphs-io
  asked: 2026-11-13
  by: claude-session
  audit: 2026-11-25 plan, one PR each with a test at the default panel width: 1) glyph buttons with their words (smallest); 2) points collapsed and grouped as Inputs and Outputs, then by type, with counts; 3) a filter with * wildcards.

- [ ] **The live free chat: the leftovers** — the free chat is on master (#359, promoted 2026-11-13; the published playground's page allows the relay's demo address). Left from its card: (1) your first look at the live page (a page served from another origin is refused by design, so it was never seen against the live relay); (2) the survey's buttons name model vendors, which the voice rule forbids; (3) the help pictures (docs/help/img) may still show the key field: `make help-shots`; (4) the key route can come out of the page (claude.rs Route::Direct, key_ui, the model picker) as its own change; (5) not exercised yet: the alarm filters on a real close, and the survey route against DynamoDB.
  needs: your look at https://helmsman.advin.io/playground/ for (1); the rest needs nothing
  result: the published chat has no key field, no vendor names, and pictures that match it
  est: 3h
  difficulty: low
  id: free-chat-followups
  asked: 2026-11-13
  by: claude-session
  audit: 2026-11-25 low-hanging, next wave: (2) neutral survey choices, kept consistent with the relay's accepted values; (3) retake the help pictures. (4) removing the key route is a removal and waits for Nick's word.

- [x] **P4 node, after the bring-up: PoE alone, two connections on the E1213, G4 and G5** — Why it matters: the P4 is the runtime board (Nick, 2026-11-13), and these are the runs between "it scans on the bench" and "it can be left in a cabinet": powered by its cable only, sharing a device with a Studio box, and measured against the two goals docs/EDGE.md set for a node (G4: under 1 % late scans over 10 minutes; G5: the block's outputs go safe when the node's cable is pulled). Done already (2026-11-10, docs/EDGE.md Phase 3 "On the board"): the node image at 192.168.3.56 (press-5, token bench-token-2026), the chase at 10 ms with 51 to 72 us of work, a 431-line cell at 1 ms, the `enip` image against the E1213 both ways, the panic restart, PSRAM in `/api/node`, the late Modbus link at boot fixed. G4 and G5 were never run on any board; the ESP32 cards that held them are closed (Nick, 2026-11-13), so they are the P4's. Left: (1) a run on PoE alone, after `p4-poe-isolation`; (2) after `e1213-firmware-update`: Input Only from the P4 beside the Studio box's connection (`scratch/enip-fo-probe.py`); (3) G4 and G5 on the P4, 10 minutes against the E1213 and a pulled cable with the unit's watchdog and safe values set (the Devices tab's Configuration can set both now); (4) drop `ethernet::CutThroughTx` when an esp-hal release ties transmit store-and-forward to checksum offload. TLS is not on it any more: you want it for publishing data from a second Ethernet port, kept apart from control (card `node-data-tls`).
  needs: `p4-poe-isolation` for (1), `e1213-firmware-update` for (2); (3) needs nothing but the bench; (4) waits for an esp-hal release
  result: the P4 node runs on PoE alone, shares the E1213, and has its G4 and G5 numbers in docs/EDGE.md
  est: 1d
  difficulty: med
  notes: flash from a session: `NODE_BOARD=p4-eth NODE_P4_NO_POE=1 scripts/node-flash.sh node` (port /dev/cu.usbmodem5C930154551; espflash 4.7.0). A program over the flash slot runs but is not kept (12 KB did not fit): node-ota's partition table kept the settings area at its 24 KiB, so a larger program slot is still open (it moves the settings, so the node is commissioned again).
  depends: p4-poe-isolation, e1213-firmware-update
  id: node-p4-followups
  asked: 2026-11-10
  by: claude-session

- [ ] **A Skipper that drives several devices at once: run it on a board** — Why it matters: a Skipper could drive one Modbus block with on/off points only; Nick wants devices of several kinds on one node (2026-11-24). What the PR does (branch `feat/node-multi-device`, docs/EDGE.md "Sixteen devices on one node"): the node's I/O map takes up to 16 `modbus-tcp` and `enip-io` devices with bool, int and real points in the box's own format (a line a node cannot run is refused with its line number); every one-device bool map keeps the addresses it had (`di:N` is `%IX(N/8).(N%8)`), bools of device k sit in a 4-byte window at byte 4k and numbers take slots in map order from byte 64; the P4 firmware has one I/O task a device and one cyclic task for the EtherNet/IP ones, each device with its own state, error and reconnect count in `/api/devices`; the `HEAP_SIZE` went from 320 to 288 KiB so the main stack stays over 32 KiB (47.5 KB, was 56.5). Proven on the host (parser, layout, codecs held to the box's drivers, the simulated Modbus device) and by the P4 build and clippy. On the board 2026-11-24 (docs/EDGE.md, "On the board, 2026-11-24"): the map on flash booted unchanged; the E1213 on Modbus, a Modbus server on the Studio with numbers both ways and an absent address ran together; a server stopped and started mid-run left the others and the scan alone; the E1213 on EtherNet/IP ran beside the Modbus server with its outputs read back from the unit. That run found the node answering nothing with an absent device in its map (an aborted socket held smoltcp's one neighbour request a second), fixed in `dial::give_up`. Left: (1) the MDrive on EtherNet/IP, after it has an address on the node's subnet; (2) two EtherNet/IP devices at once (the E1213 and the MDrive: one UDP socket, T->O routed by source address), the E1213 holds one Class 1 connection so not on Modbus in the same run; (3) pull one device's cable mid-run, the others and the scan unaffected, the lost device's outputs back after the reopen; (4) heap, peak and PSRAM use with 16 devices and the 12 KB cell against the dev tip, since the smaller heap makes a large program spill sooner; (5) 16 addresses with half absent, for the one-dial-per-2-s rule (`dial.rs`). Not done and named: the canvas does not show a point's `%I` address yet (`GET /api/io_map` lists it in `image`); a rollback to a firmware older than this reads a map over 2048 bytes as no program.
  needs: the MDrive re-addressed onto 192.168.3.x; the P4 (press-5, 192.168.3.56) and the E1213 as they are
  result: a node runs several devices of two kinds on the bench, with numbers in `docs/EDGE.md` for G4 and the heap
  est: 3h
  difficulty: med
  id: node-multi-device
  asked: 2026-11-24
  by: worker (feat/node-multi-device)

- [ ] **A Skipper saves a map only after its devices answered it** — Why it matters: the machine that scans and writes the map reaching a device does not mean the Skipper can (the MDrive answers the Studio and is on a subnet the Skipper is not on), and a map with a silent device used to be found out only after the download (Nick, 2026-11-24: "verify that there's not a comms issue before downloading the map"; blocked until all answer). Built on `feat/node-comms-check` (docs/EDGE.md, "A map is saved once its devices answered the node"): `POST /api/io_map/check` and its `GET`, `PUT /api/io_map` and the page's form saving only the text whose check passed in the last ten minutes, the P4's check task (a Modbus read or an EtherNet/IP session, nothing written, no I/O connection), a device on another subnet failed with the address it needs. On the board 2026-11-24: two devices answered, an absent one timed out, the MDrive was named off-subnet, and the save was refused before and after. Left: (1) a passing check followed by the save, on the board (the P4 went off USB mid-run); (2) the programming side: the canvas page that watches a Skipper (`?box=`) now edits its map in a window opened from the Devices tab, checks the devices (`POST` then `GET /api/io_map/check`, a line per device) and offers Download (`PUT /api/io_map`) only for the exact text a check passed (`feat/canvas-skipper-map`, stacked on this branch; not yet seen against a real board, and a Skipper that answers 404 to the check route gets neither button). Still open under (2): a box project's map flattened for a Skipper, a design to agree with Nick first.
  needs: the P4 back on USB or PoE for (1); an interview with Nick for (2)
  result: from the Devices tab, a map is checked from the Skipper, the answers are shown per device, and Download is offered once all answered
  est: 1d
  difficulty: med
  id: node-comms-check
  asked: 2026-11-24
  by: claude-session

- [ ] **The P4's `enip` image after #333: the two runs that were skipped** — Why it matters: #333 moved the node's EtherNet/IP session onto its own task so a slow device cannot make the scan late, and that was checked only for the first connection. The two runs here are the ones a plant sees: the device goes away and comes back, and the node holds the connection as its only owner. #333 moved the EtherNet/IP session into a task of its own and was checked on the board for the first open only (1.1 s after link-up, 0 late scans, was 199). Two runs were left out and are on no card: (1) the close after a lost link, through the task: pull the cable or power the E1213 off mid-run, see the outputs go to their safe state, the session close, and the reopen once the unit is back, with no late scans meanwhile; (2) Exclusive Owner with this change (the 640 of 640 packet run of 2026-11-10 was on the image before it). Write both into docs/EDGE.md Phase 3 "On the board".
  needs: the E1213's one Class 1 connection free: the try box off the unit (`studio-box-no-io`, or `launchctl bootout gui/501/io.advin.helmsman-try` from your terminal until that lands); no other session flashing the P4
  result: the enip task is proven for loss and for ownership, not only for the first open
  notes: flash: `NODE_BOARD=p4-eth NODE_P4_NO_POE=1 SERIAL_PORT=/dev/cu.usbmodem5C930154551 scripts/node-flash.sh enip`, then back to the `node` image with `NODE_SEED_CHASE=1`. Read the outputs back over Modbus with `/usr/bin/python3` (Local Network privacy).
  est: 2h
  difficulty: low
  id: enip-task-bench-checks
  asked: 2026-11-10
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for the bench: the E1213's connection free and a cable pulled.

- [ ] **The design step: a strong model designs the system, the everyday agent builds it** — Nick, 2026-11-15: "I need Opus 5.5 equivalent jumping in for hardcore system designs in one-shot for you to build from. Something so absurd that more abstraction will need to be added." Interview the same day: a design step the engineer asks for (not an escalation the cheap model decides, not the whole chat on the strong model); Opus 5.5 on the Anthropic key behind the relay's `best` tier; reachable from boxes with a token and, capped, from the playground. The session's own call: the playground runs it on the free chat's own capped key, never the Anthropic key, so the hard bound on public spend stays. Built: the relay profile `helmsman/design-v1` (agent-relay #39). It answers with no program: Machine, Signals, Parts (each with its ST interface and state table), Faults, numbered testable Requirements, a Build order whose steps name the requirements they must pass, Open questions with the value assumed. The dialect, the project, its I/O and its requirement tests go in the request as `<dialect>`, `<project>`, `<io_devices>`, `<requirement_tests>`. A big design takes several turns: after an answer that stopped at max_tokens the client sends the one word `Continue` with the assistant turn (its thinking block included) and the answer carries on from the exact character. Measured on a bottling line: 472 s for the first turn, 230 s for the second, about 78,000 output tokens, 11 parts, 81 requirements, 19 build steps. Since built: the page's half of the contract (`crates/helmsman-canvas/src/design.rs`, #465: the request, Continue and join, the reader, the brief for one build step, tested against the recorded bottling line and lamp), the box route (`POST /api/design`, #467) and the relay's tier grant (agent-relay #41: a token needs `--tier-daily-tokens N --tiers best` to be served a profile off the default tier, refused past that many tokens a day; `agent-relay-admin grant <client> ...` amends a live token in place). Left: the Agent tab entry (`design-step-canvas`), the playground (`design-step-playground`), the deploy (`design-step-deploy`).
  result: an engineer describes a whole machine, reads its design, and the Agent builds it step by step against the design's requirements
  est: 4d
  difficulty: high
  id: design-step
  asked: 2026-11-15
  by: design-step session
  audit: 2026-11-25 partly: page, relay and box halves are built, then the entry was shelved on 2026-11-25 (Designer::shelved, f2af2084). Left: Nick's word to bring it back.

- [x] **Design step on a box: the box asks the relay for the design profile** — `POST /api/chat` reaches one agent profile today (`--relay-agent`, default v7). A design request goes to `helmsman/design-v1` with the box's dialect, its project files, `list_devices` output (wired devices included) and its requirement tests as the tagged blocks; the stream is forwarded as it is, and a `Continue` is one more request. An unknown-profile answer from the relay (it has no `best` tier) reaches the page as "the design step is not available on this relay", not as an error. The relay's function allows 600 s a turn: the box's own request timeout must not be shorter.
  needs: nothing to build; the relay deployed with a `best` tier to try it
  result: `--relay-design-agent design-v1` (or the default once deployed) and a design route the box page uses
  depends: design-step
  est: 1d
  difficulty: med
  id: design-step-box
  asked: 2026-11-15
  by: design-step session

- [ ] **The playground's design entry, on a key of its own** — decided 2026-11-16: the design entry on the playground runs on a second capped key ($3 a day: two large designs or many small ones), checked by `put-demo-key.sh` like the chat's, with an allowance of 6 design turns an hour per address. When the design key is spent only the design entry says so and the chat carries on. In the relay: the `design` demo agent on that key, its own request shape (a continued design sends the model's reasoning back, which the chat's text-only requests cannot carry) and body cap, a Timeout a design turn fits (one ran 472 s against the chat's 120 s), status that reports the design entry by itself. On the page: the Agent tab's design entry talks to it (`design-step-canvas`) and is hidden when the relay does not serve it. Yours afterwards: create the second key with its cap, run `put-demo-key.sh` for it, deploy.
  needs: the relay's draft PR reviewed and deployed with the second key before it can be tried live
  result: a visitor asks for a design on the playground and the free chat stays open for everyone else
  depends: design-step
  est: 2d
  difficulty: high
  id: design-step-playground
  asked: 2026-11-16
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for Nick: the second capped key, put-demo-key.sh, a relay deploy, and whether Design comes back at all (it is shelved).

- [x] **Deploy the relay with a `best` tier** — the live relay (stack `agent-relay`, checked 2026-11-15 with `aws cloudformation describe-stacks`) has one model, `deepseek/deepseek-v4.1-flash`, and no `Tiers` parameter; its `main` is 29 commits behind `dev` (tiers, #24 on, are not on it). The Anthropic key is in Secrets Manager already (`agent-relay/anthropic-api-key`, 2026-11-04). Order: agent-relay #39 lands on dev; your merge of the relay's dev into main; then from the deploy checkout `SECOND_UPSTREAM_SECRET=agent-relay/anthropic-api-key RELAY_TIERS='{"upstreams":{"or":{"kind":"openrouter"},"ant":{"kind":"anthropic"}},"tiers":{"default":{"upstream":"or","model":"deepseek/deepseek-v4.1-flash-1017"},"best":{"upstream":"ant","model":"claude-opus-5-5"}}}' ./scripts/deploy.sh`. Then each box that should have the design step gets the grant on its live token, no new token: `RELAY_TABLE=agent-relay cargo run -q -p agent-relay-aws --bin agent-relay-admin -- grant <client> --tier-daily-tokens 2000000 --tiers best` (agent-relay #41; 2,000,000 tokens a day is about 25 large designs, at most about $40). Without the tiers the same build still starts and leaves design-v1 out (it says so in its log). First live turn: watch that the stream does not go silent while the model thinks (summarized thinking is there to prevent it; the trials ran through OpenRouter, never Anthropic's own API), and the DynamoDB path of `grant` has only been read, not run (no Docker for `make test-integration` that night). A design turn ran 472 s against the function's 600 s Timeout; the free chat's function allows 120 s, which no design turn fits, so its Timeout rises with the playground's design profile (a draft PR in the relay, deploy chain). Cost seen: about 78,000 output tokens for a large design, $20 a million on Opus 5.5.
  needs: your merge of agent-relay dev into main, then the deploy (a hand-off script will carry the one line)
  result: `GET /v1/agents` on the live relay lists helmsman/design-v1 served by claude-opus-5-5
  depends: design-step
  est: 1h
  difficulty: low
  id: design-step-deploy
  asked: 2026-11-15
  by: design-step session

- [ ] **The Agent's edits and its question, where they are not yet** — built for the page on the free chat (2026-11-15: `crate::edits`, `crate::ask`, agent-relay #40). Measured on the ladder: a one-line change to a 250-line program in 7.3 s and one call, was 23.7 s and two; no difference on short programs; 48 of 48 either way. Asking without extended thinking was measured too and left out (`HELMSMAN_LADDER_QUICK` in agent_ladder_tests.rs says why). Left: (1) a box: `instruct::BOX_RULES` still asks for the whole file and the box's Agent tab shows no question (after `agent-progress-box`; needs a served relay profile for each, which is a new id and a deploy); (2) a visitor's own key and the local proxy still get whole files (`claude::SYSTEM`); (3) while an answer streams as edit blocks the Code tab keeps showing the file as it is, and the tab says "Writing main.st" with no draft: show the edits in place; (4) the question card has not been seen in a browser (the smoke run has no relay that serves `ask`); (5) the ladder has no rung for the question (no question on a clear instruction, the answer honoured on an open one): the page-level tests in ask_tests.rs script the relay.
  needs: `agent-edits-deploy` for (4); `agent-progress-box` for (1)
  result: the same speed and the same question on a box and with a key
  depends: agent-progress-box, box-edits-profile
  est: 3d
  difficulty: high
  id: agent-edits-everywhere
  asked: 2026-11-15
  by: claude-session
  status: ready
  audit: 2026-11-25 plan: 1) a ladder rung for ask; 2) a relay profile for the box that speaks edits and ask (Nick's deploy); 3) BOX_RULES rewritten for edits, crate::edits in the box answer path; 4) the question card in box_view; 5) the same for the key path. Part (3) of the card is moot (no streamed draft, 2026-11-22).
  decided: 2026-11-25 Nick: yes, one new served profile for the box that speaks edits and ask. The profile is a session in ../agent-relay and then Nick's deploy.

- [x] **The box's Agent tab shows its progress too** — the page shows the time, the latest thought and the draft as it streams (feat/agent-progress); the box's Agent tab goes through `POST /api/chat` and still shows only a spinner (`instruct::View::progress` is `None` there). Needs the thinking and text deltas through the box's chat stream to the canvas, then the same `Progress`.
  needs: nothing
  result: an engineer on a box sees the Agent think and write, as in the playground
  est: 1d
  difficulty: med
  id: agent-progress-box
  asked: 2026-11-13
  by: claude-session
  audit: 2026-11-25 closed, stale: Nick asked on 2026-11-22 for no streamed thinking or draft. Left over: the box Agent tab shows a spinner with no clock (box_view.rs passes progress: None), a few lines.

- [ ] **The Agent watches: on a box, and beside the failure** — Nick, 2026-11-14: "the agent should feel like a fundamental part of the environment. It should be watching every time it breaks to point out the stupid thing you did to show how it should be done better." Built for the simulation (`crates/helmsman-canvas/src/watch.rs`): a compile error, a fault, a program held by a lost input and a red requirement test are each said unasked in a card over the diagram with the better way and a Fix it button that hands the failure to the Agent. The words come from rules, so watching costs no model turn (decided 2026-11-14: only the click spends one). Left: the same on a box's page (`box_view`: its diagnostics, the modules' held and faulted states, a lost device); the line drawn beside the failure itself (the editor's line, the test's row) as well as over the diagram; more rules as real mistakes show which are missing; a value out of its limits.
  needs: nothing
  result: on a box too, a failure is followed by its explanation and its fix without the engineer asking
  est: 3d
  difficulty: med
  id: agent-watches
  asked: 2026-11-14
  by: claude-session
  audit: 2026-11-25 plan, one PR each: 1) box_view feeds watch::Seen (diagnostics, held and faulted modules, a lost device) and shows the card; 2) a marker in the editor beside the failure; 3) a marker on the requirement-test row; 4) the out-of-limits rule.

- [ ] **Name the sensors first: the device's side, the safe state, the plant** — Nick, 2026-11-14: "Those sensors? It should guide you to name them and their function. Their output? It should be deeply known ahead of time." Built for a new program in the simulation (`crates/helmsman-canvas/src/io_names.rs`): a fold in the Agent tab where each point gets a name, input or output, its type and what it is; the list goes to the Agent ahead of the instruction and the program is written in those names. Left: a device added from the library fills the list from its profile's channels (names, types, units, limits), so nothing about the device is typed (`device-profiles`); each output's safe state when its device is lost, asked once and carried into the map; the simulated plant (`plant.sim`) drafted from the same answers, so the faults thrown at the program are the named sensors' own; the same list for an existing program (its ports and their comments), and on a box.
  needs: nothing
  result: an engineer states what each sensor is once; nothing else about the I/O is asked
  est: 3d
  difficulty: med
  id: agent-names-io
  asked: 2026-11-14
  by: claude-session
  depends: safe-state-home
  audit: 2026-11-25 plan: 1) fill the list from a library device's channels; 2) a safe state per output, carried into the map; 3) a draft plant.sim from the answers; 4) the same list for an existing program and on a box.
  decided: 2026-11-25 Nick: an output's safe state is a safe= word on its io.map line.

- [ ] **The Agent sees your patterns: across sessions, and beyond compile errors** — Nick, 2026-11-14: "It should be seeing your patterns and fixing them. It should be putting the relevant cognitive load on you and helping you build quickly, not forcing you to take baby steps." Built in the simulation (`crates/helmsman-canvas/src/watch.rs`): the same kind of compile error in several places is said once with one Fix it for all of them, and a kind that comes up three times in a session is said to be a habit; the count lives in the page's memory only. Left: habits that last past a reload (what is kept about an engineer, where and for how long is yours to decide: this browser only, the project, or their account on a box); patterns beyond compile errors (the same edit repeated across a program, a requirement always written without its fault case); a habit turned into a standing fix the Agent applies by itself.
  needs: your decision on what may be kept about an engineer, and where
  result: a repeated mistake is pointed out once and fixed everywhere, and stays learned
  est: 4d
  difficulty: high
  id: agent-patterns
  asked: 2026-11-14
  by: claude-session
  audit: 2026-11-25 waits for the question agent-habits-where.
  depends: settings-on-device
  decided: 2026-11-25 Nick: habits are kept with the person's account on a box. Plan: 1) habits as a section of the per-account settings record (settings-on-device); 2) watch.rs loads and saves them through it; 3) a "what is kept about me" list with a clear button in Settings. On the public page with no account they stay in memory only.

- [ ] **Device configuration follow-ups, from a recovered snapshot** — work that sat uncommitted in a worktree since 2026-11-12, now the branch `wip/recovered-devconf-canvas-20261114` (not built or tested, based on a tree from that day). In it: a failed re-read of a device's configuration keeps the last one on screen beside the error instead of replacing it; a new watchdog draft starts at 60 s and the sentence says seconds; a browser smoke of the Configuration section against the simulated E1200 console (`scripts/canvas-smoke-devconf.py`, `helmsman-devconf/examples/sim_console.rs`). Rebase it on dev, finish its tests, and land it, or delete the branch if the Configuration section has moved on.
  needs: nothing
  result: the follow-ups landed, or the branch gone
  est: 4h
  difficulty: low
  id: devconf-recovered
  asked: 2026-11-14
  by: claude-session
  audit: 2026-11-25 low-hanging, next wave: cherry-pick f858272c from origin/wip/recovered-devconf-canvas-20261114 onto dev, resolve device_config.rs, land it or delete the branch.

- [ ] **The browser smoke run shows a found device the library does not know** — Why it matters: the Devices tab's row for such a device, with its "Build from vendor file…" button, the notes beside it and the Add it offers after a profile is saved (#452, 2026-11-15), has only ever been drawn by the headless UI tests, which check geometry and text and not what a browser shows. `scripts/canvas-smoke.py` never scans a device the library lacks, so no screenshot holds that row. Give the box page's smoke run (`make canvas-smoke BOX=1`) a found device with no library profile (a scripted scan answer or a loopback device the smoke box can find, never a bench device), and take a screenshot of the row, of the note after a file is picked, and of the row once a profile is saved.
  result: the vendor-file path is looked at in a real browser on every smoke run, at the panel's default width
  est: 3h
  difficulty: low
  id: smoke-unknown-device
  asked: 2026-11-15
  by: claude-session (board run)
  status: doing
  audit: 2026-11-25 low-hanging, in progress: worker on test/smoke-unknown-device.

- [x] **Feature parity, dev against live, and `make ship`** — Nick, 2026-11-20: a vertical test layer beside `make compat` (which is horizontal: the four repositories against each other's contracts), so a promotion is one step and every feature that works on dev is shown to work live. Decided by interview that day: `make ship` is one command Nick runs (the dev→master merge stays his); "live" is the site's playground, the deployed relay and a fresh loopback box built from master (never the try box); journeys go through the box API and the relay, with a thin browser pass that reaches every tab; a features manifest lists every API route, canvas tab and default relay profile with its journey, and a test fails when a surface has none; device journeys run on simulators only (Moxa sim, OpENer), never the E1213; candidate runs use the stub model, live runs spend one short real turn per agent route (chat, design, help, explain, tests); a journey green on candidate and red live after a ship gets a report and a draft revert PR on master, never a reset. Milestones: (1) manifest + coverage test, (2) `helmsman-compat parity --target candidate|live` with target/parity/report.md, (3) browser pass over every manifest tab, (4) `make ship` (candidate parity + compat, promote, relay deploy, live-state, live parity, revert draft), (5) docs/PARITY.md. All PRs drafts (they reach prod).
  result: one command promotes all four repositories and reports, per feature, whether live matches dev
  est: 3d
  difficulty: high
  id: parity-ship
  asked: 2026-11-20
  by: claude-session 44a269b3
  audit: 2026-11-25 done: docs/PARITY.md, make parity, make ship. #637 splits it into make staging (dev to staging) and make ship (staging to production).

- [x] **The DHCP server moves a client off a lease it ACKed** — `helmsman-dhcp` tests/hostile.rs `no_request_sequence_breaks_the_invariants` found it on 2026-11-20 and fails on every run once the case is in its regressions file. Minimal case: client 00-07-46-00-00-03 sends a REQUEST (option 53 = 3) with ciaddr 192.168.3.103 and no lease on record; the server ACKs .103. Its next DISCOVER (53 = 1) is offered .102, moving it off a lease that is still running. RFC 2131 has a server answer a REQUEST from a client it holds no binding for with a NAK or silence (§ 4.3.2 from memory, not yet checked; cite it in docs/SOURCES.md once read), and offer an existing binding on DISCOVER. The full shrunk input and its `cc` seed line are in scratch/dhcp-lease-moved-2026-11-20.txt (main checkout). Not committed anywhere: committing the seed alone turns every PR red. The fix starts by committing that line to tests/hostile.proptest-regressions and watching it fail.
  result: a client keeps the lease it was given; the property holds with the seed checked in
  est: 2h
  difficulty: med
  id: dhcp-lease-moved
  asked: 2026-11-20
  by: claude-session 44a269b3
  audit: 2026-11-25 done: the same fix, 9253b69d (#559). The seed in scratch/dhcp-lease-moved-2026-11-20.txt was not added to hostile.proptest-regressions and was not re-run.

- [ ] **Let a box reach a device console on another port, so device settings can be checked on a simulator** — the parity run's `device-config` journey is pending because the box reaches a unit's web console on port 80 only (`api::devconf::Setup::in_state_dir` hardcodes `http_port: 80`), and the console simulator (`helmsman-devconf` `sim`) cannot bind 80 unprivileged. Shape: the io.map device line takes an optional `console=PORT` (default 80) that `Setup` reads, which is also true of a unit behind a NAT or a port forward. Then crates/helmsman-compat/src/parity/devices.rs starts the sim and turns `device-config` on (read, set a harmless setting, snapshot).
  result: device settings is walked end to end on every ship, on the simulator
  depends: parity-ship
  est: 3h
  difficulty: low
  id: parity-device-config
  asked: 2026-11-20
  by: claude-session 44a269b3
  audit: 2026-11-25 low-hanging, next wave: an optional console=PORT on the device line (api/devconf.rs hardcodes 80), then enable the device-config journey in parity/devices.rs against the devconf simulator.

- [ ] **SATURDAY NIGHT 2026-11-28, Fable overnight on the Studio with the bench: how big a machine can one Skipper run (plan in PERFORMANCE.md)** — A local session, not a cloud run, because it measures on the Skipper. Nick starts it around 21:00 CT (a reminder fires then): in tmux, `cd ~/github/helmsman && claude --model claude-fable-5-2`, then the start line at the top of PERFORMANCE.md. It follows PERFORMANCE.md "Brief for the Saturday run" and opens a DRAFT PR `docs: Skipper capacity plan (strategist)` into dev by Sunday morning. Why it matters: Nick asked (2026-11-26) whether one Skipper could run 16 servo axes, 32 sensors and 25 valves, and where the realistic ceiling is. Today it cannot: the map caps (16 devices, 96 points, 128 bytes of numbers a direction, 4 KiB of map text) and the 8 KiB program slot refuse that machine before RAM or scan time matter, and the slot already refuses the 431-line cell that ran on the P4. PERFORMANCE.md holds the problem statement for a strategist to read cold: the limits with their file and line, the target machine against them, what was measured on the board, the guarantees a solution must keep, the levers known so far, and what nobody has measured. One session's reading is in it, marked as such, to be argued with.
  needs: Nick starts the session Saturday night with the Skipper on USB and the bench free; then his review of its draft PR
  when: Saturday 2026-11-28, from about 21:00 CT, overnight, local
  result: a ranked plan written into PERFORMANCE.md (each change, what it buys in axes, points or program lines, its cost, how the bench proves it, the order) and the rule for splitting a big machine across Skippers or onto a box; the build cards it implies added under this one
  est: 3h
  difficulty: high
  model: fable
  id: node-capacity
  asked: 2026-11-26
  by: claude-session 50f6a24e

- [ ] **Point names per device, for when every device has an `enabled`** — Since feat/short-point-names a point the I/O tab adds is named by its channel alone (`di_00`, `enabled`); the device's name goes in front only when that name is taken. Nick, 2026-11-26: names like `enabled` and `di` will be common to many devices on one network, so this will need doing properly. The likely shape: the program reads a point through its device (`motor.enabled`), one structure per device, so a device rename changes one name and points stay short. That changes how the box, the Skipper (32-character names) and the compiler bind points, so it is a design before it is code.
  result: two devices with the same channel names sit in one project without prefixed names, and a device rename touches one name in the program
  difficulty: high
  id: point-names-per-device
  asked: 2026-11-26
  by: session e16d1c3b

- [ ] **Merge the Devices tab into the I/O tab?** — Nick, 2026-11-26: "Now that I look at it I don't even know if the Devices page is necessary... there's got to be a way to instead merge it with the I/O so that you can easily add additional inputs per device rather than looking at the device level. Something to think about, but maybe not build out quite yet." Today the Devices tab lists the I/O map's devices with their points and adds a device (+), Add I/O per device, Edit and rename; the I/O tab lists the program's inputs and outputs as cards with their values, wiring, trend and diagram glyphs. A merged tab could group the I/O cards under their device (with a + Add I/O on each group and a + device at the end), keep the simulated points in their own group, and fold a device's settings into its group's header. Questions for a design pass: what the Skipper map window (Edit › Edit map…) becomes, where a device with no points yet shows, and how a point on no device (a placeholder) reads.
  result: one tab where a device's inputs and outputs are added and watched together, and the Devices tab is gone
  difficulty: high
  id: merge-devices-into-io
  asked: 2026-11-26
  by: session e16d1c3b

- [x] **Update the guided tour to the new layout and flow** — Nick, 2026-11-26: once the bench rounds of 2026-11-26 land (#656 and feat/bench-round-2: Messages as the bottom panel's Log tab, the Help tab, the History arrows and H, the I/O card glyphs, the fit glyph and Z, the new hotkeys, the side-pane ribbon, the zoom ladder and lamps gone), walk crates/helmsman-canvas/src/tour.rs step by step against the page and rewrite every step that points at something moved or removed.
  result: every tour step points at what is on the page and says what it does now
  difficulty: med
  id: tour-after-bench-rounds
  asked: 2026-11-26
  by: session e16d1c3b
  status: done

## Canvas overhaul (interview 2026-11-21)

Nick's notes, 2026-11-21: turn the node map into 3D with height as abstraction, dive into blocks, bloom, fast snap navigation, device-library graphics on connections, and convert technical drawings (panel layouts, CAD, DXF, schematics) into matched components, "starting now". Decided in the interview that day: (1) one 3D stack for every level: plant and devices on top, programs and POUs in the middle, the ST diagram's blocks at the bottom; a dive goes down a level; (2) rendered by egui on glow with our own GL pass through a paint callback (camera, picking, two-pass bloom), no engine crate and no backend change; (3) for engineers and demos first: 3D on a desktop GPU, the 2D view kept reachable and tested; (4) two tracks at once: drawings now, and the 3D canvas, then navigation, then device graphics; (5) dive: a fly-through by default and a free orbit over the whole stack as a second mode, both offered; (6) drawings start with ASCII DXF panel layouts, read by a small reader of our own (no new crate, wasm-safe), built against DXFs vendors publish for parts already in the library; (7) a drawing gives devices matched to the library with a draft io.map, their placement in the 3D map and its wiring as connections; (8) a part not in the library or matching more than one record is shown and the engineer decides: never a guessed model; (9) device graphics are line art per device family in the canvas's own style (the library has no images, and vendor photos are not ours to publish); (10) first milestone: the playground's example in 3D behind a toggle, 2D still the default.

Nick, 2026-11-22, after four rounds of a 3D view in the playground (#549 in dev and reverted, draft #552 closed; the last whole state is the branch `feat/canvas-3d-mvp`): "just stick with the 2D and eliminate 3D", "let's go minimalistic", and why: "3D really was supposed to be more immersive and helpful, not just a pretty version of 2D". Decisions 1, 2, 3, 5 and 10 above are withdrawn and their cards (`canvas-3d-mvp`, `canvas-3d-nav`, `canvas-3d-devices`) removed: the canvas stays the 2D diagram. What the rounds showed, for whoever tries again: one program on one board has nothing for height to mean, so a 3D view of it is the 2D layout tilted; the idea needs the levels (plant, devices, programs, blocks) to exist first. The drawings track (decisions 6, 7, 8) stands. Nick, 2026-11-23, on what the 3D device level was to carry: device glyphs come to the 2D canvas where a device already shows (the Devices and I/O tabs, a point's read and write blocks in the diagram), and there is no placement view: decision 7's placement and wiring half is dropped with its card (`drawing-place-wire`), and a drawing gives devices and a draft io.map only.

- [ ] **Device graphics: line art per device family in the tabs and the diagram** — Decision 9, in 2D (Nick, 2026-11-23). The library (helmsman-devices) has no images and vendor photos cannot go in a public repository, so a glyph per family (remote I/O block, drive, proximity sensor, photoelectric sensor, valve, switch, ...) drawn in the canvas's style, picked from a record's kind, with a generic one for an unknown part. It shows where a device already shows: each device's row in the Devices tab, each point in the I/O tab, and a point's read and write blocks in the diagram. No new view. The canvas already has `glyphs.rs` and `assets/glyphs`: start from what is there. A glyph in a tab row must not widen the side panel (a test at the panel's default width, and `every_tab_leaves_the_panels_their_width` green).
  needs: a `kind` on each library record that lacks one
  result: every device and wired point shows what it is at a glance in the tabs and the diagram, with no image fetched from a vendor
  est: 4d
  difficulty: med
  id: device-glyphs
  asked: 2026-11-21
  by: session canvas-overhaul interview
  audit: 2026-11-25 partly: 17 glyphs drawn in the Devices rows, the I/O tab and diagram blocks. Left: a glyph per family chosen from a record's kind (for_device returns only box, hmi or module).

- [x] **Read a DXF panel layout: a reader of our own, and its parts matched to the library** — Decisions 6 and 8; A new crate `helmsman-drawing` with no dependencies (it must build for wasm32): ASCII DXF's group-code pairs, BLOCKS and INSERT with ATTRIB, TEXT and MTEXT, LINE and LWPOLYLINE; binary DXF and anything else is refused by name, not skipped silently. Each inserted part's tag, part number and position come out; a part is matched to a library record by order number or model text (`devrecord`, the index the box and the playground already read) as matched, ambiguous or unknown. Fixtures: the DXFs vendors publish for parts in the library (Moxa E1213, Murrelektronik IMPACT67, ifm sensors), fetched with the library's `docs-fetch.py` and kept with their source and licence; a file whose licence forbids keeping is fetched in the test's cache, not committed. Hostile input by default: truncated files, a block that inserts itself, huge counts.
  needs: nothing
  result: a DXF in, a list of parts out with a position and a matched / ambiguous / unknown status each, as a library call and a test over real vendor files
  est: 1w
  difficulty: med
  id: drawing-dxf-read
  asked: 2026-11-21
  by: session canvas-overhaul interview
  audit: 2026-11-25 done: crates/helmsman-drawing (65ea6d1a), with tests/reader.rs and tests/vendor.rs.

- [ ] **A drawing in the canvas: review its parts, get the devices and a draft io.map** — Decisions 7 and 8. Upload a DXF in the Devices tab (on a box and in the playground): the review lists every part with its status; the engineer picks among ambiguous matches and an unknown part stays as a generic device with its tag. Confirming adds the devices and writes draft map lines (`wired=` where a record matched) through the review the Agent's proposals already use.
  needs: drawing-dxf-read
  result: a panel's devices in the project from its drawing, nothing added that the engineer did not see
  depends: drawing-dxf-read, drawing-draft-address
  est: 1w
  difficulty: med
  id: drawing-review
  asked: 2026-11-21
  by: session canvas-overhaul interview
  audit: 2026-11-25 plan: 1) a pure model in the canvas: parts to a review list and decisions, to draft io.map lines; 2) upload and review window on the Devices tab in the playground; 3) the same on a box, with the wasm dependency check; 4) the FEATURES row.
  decided: 2026-11-25 Nick: a matched part's draft line carries the library's default address, shown in the review and changeable there.

- [ ] **Later inputs: schematics as PDF, then native CAD** — Not first (decision 6). A schematic as PDF or a scan needs a model reading the page, a confirm step for every wire, and a relay profile (Nick's deploy). DWG, EPLAN and STEP need a vendor's library or a converter: a dependency and licence decision to bring to Nick before any code.
  needs: drawing-review in use on real files; a real customer schematic
  result: a decision per format, with what it costs
  depends: drawing-review
  est: 3h
  difficulty: low
  id: drawing-schematics
  asked: 2026-11-21
  by: session canvas-overhaul interview
  status: blocked
  audit: 2026-11-25 waits for Nick: a dependency and licence decision per format, and a real schematic.

## UI feedback (Nick, 2026-11-23)

Nick's list, 2026-11-23: minimal UI, glyphs instead of words, nothing jumpy; pull context from everywhere (logs, errors, warnings, map, code) before acting. Decided in the interview that day: the in-flight work first (relay profile, then the diagram edits), then this list one PR per tab in sequence, Agent tab first; the Trend legend wraps up to a cap and then scrolls. Earlier the same day (edit_error.png): Describe offers the nearest thing instead of refusing (#576, draft).

- [x] **Relay: playground-v9 with the offer rule** — the public playground keeps answering "I can't do this one" until the relay serves a profile whose refusal rule is the offer rule of #576 (`Offer: rename a b`, `Offer: instead ...`), same words as `claude::SYSTEM`. Ship the page before the relay: an older page shows the raw Offer lines.
  needs: Nick's relay promote and deploy
  result: a draft PR in agent-relay with playground-v9 and its pins
  est: 1h
  difficulty: low
  id: relay-offer-profile
- [x] **Diagram: rename and set values by hand** — on the diagram a right click has Rename…, a double click edits a name or a set value (a constant, a timer preset) where it stands, and the code follows; never the logic (Nick: "I don't want to manipulate the logic"). Rename… on the I/O rows too. Each edit is saved as one revision with a History entry and Undo, as a described change is. The label is the variable's name for now. Reverses the 2026-11-18 principle in docs/UI.md for names and set values only; docs/decisions/diagram-edits.md has the removed engine (SetConst, Rename at 4a6d37aa). Fix with it: `rename_plan` renames a PROGRAM whose name equals the variable.
  result: names and set values change on the diagram and in the code together
  est: 2d
  difficulty: med
  id: diagram-names-values
- [ ] **Labels above the names** — Nick, 2026-11-23: a label apart from the variable's name, "at a higher, abstracted layer eventually". Not now: the label is the name until this is designed.
  depends: diagram-names-values
  difficulty: med
  id: label-layer
  status: blocked
  audit: 2026-11-25 waits for Nick: the design of what a label is.
- [x] **Agent tab** — (1) no "Stop and reset after a change" checkbox: a real device always stops and resets, the simulation takes live changes by default, the choice lives in the user settings. (2) The input grows as typed to about 4 lines then scrolls inside, anchored at the bottom and growing upward so the output does not move, height eased over about 100 ms, back to one prompt's height on Enter. (3) Output stays green and red, and can be selected and copied. (4) The history arrows restore both the prompt and its output for that step (today the prompt stays blank and the output stays on the latest step). (5) Bug: stepping through history writes to the message log and raises the error count; it must write nothing. (6) The history clock moves here: a click turns the lower area into a searchable index of the commit messages, beside back and forward.
  depends: diagram-names-values
  result: one PR for the Agent tab
  est: 2d
  difficulty: med
  id: fb-agent-tab
- [x] **Global chrome** — the brand wordmark top left (encoder mark and Helmsman in the agreed font) in place of the text; one hamburger menu in place of File / Edit / View / Help; glyphs over text labels throughout; tooltips with the hotkey first and two lines at most, longer explanations in Help (a test holds the limit); a clearer glyph for folding a tab.
  depends: fb-agent-tab
  result: the hamburger menu, the fold glyph and the tooltip ratchet are in; the wordmark draws the brand's mark in the canvas's own face, and the brand face is the open question brand-wordmark-font
  est: 2d
  difficulty: med
  id: fb-global-chrome
- [x] **Code tab** — Cmd/Ctrl+F opens the app's own find, not the browser's; a magnifier beside the file name with Find, Replace, Replace All; compile status hidden while the code compiles, a red X popup and a marker on the failing line when it does not, the grey indicator gone; Deploy at the panel's top right, bigger, filled green with a black arrow; word wrap stays.
  depends: fb-global-chrome
  est: 2d
  difficulty: med
  id: fb-code-tab
- [x] **IO tab** — keep the grid and icon layouts, the toggles and jump-to-node; loosen what is too collapsed; no effects on the larger tiles (a quick snapshot); "Describe a start state" does nothing today: make it an input to Helmsman that sets up IO, states and labels, with renames going into the code.
  depends: diagram-names-values
  est: 2d
  difficulty: med
  id: fb-io-tab
- [x] **Files tab** — a header naming the file system; under programs, Examples and My Files as sections that fold.
  est: 4h
  difficulty: low
  id: fb-files-tab
- [x] **Messages** — a summary of Helmsman's output in the log for the operator (the full context still goes to the model); errors shown by default, info and warnings hidden; keep Copy All, add Clear.
  est: 1d
  difficulty: med
  id: fb-messages
- [x] **Trend legend wraps** — the signal legend on rows instead of one crowded row, the variables' names kept (red_N). Decided 2026-11-23: wrap up to about a third of the tab's height, then scroll inside the legend, because rows without a cap left the plot no height (#561).
  est: 3h
  difficulty: low
  id: fb-trend-legend
- [x] **Canvas: no semantic zoom, corner controls** — the abstraction layers by zoom (structured text zoomed out, function blocks mid-zoom) off for now: they look glitchy and like another interface. Switched off at the view, the folding kept: later, zooming out merges nodes into labelled groups in the same style with smooth transitions. Background drag stays box select, Space and drag stays pan. Bottom right: a pan (hand) toggle, zoom in, zoom out.
  est: 1d
  difficulty: med
  id: fb-canvas-zoom
- [x] **Local user settings and sign-in on a Skipper: the design** — a Skipper runs on the device, so preferences must be kept on the device, which implies a light local user and sign-in model. Nick: propose a design before building. Start from what a box has (accounts, roles, sessions) and say where the playground keeps its own.
  result: a design document for Nick to read, nothing built
  est: 4h
  difficulty: high
  id: local-user-settings

- [ ] **Settings kept on the Skipper, and a Settings window** — step A of docs/decisions/local-user-settings.md, decided 2026-11-23. A typed settings record per signed-in person on the device (beside the desk), `GET`/`PUT /api/settings`, the device's defaults for a signed-out screen (an integrator sets them; a change made signed out stays in that browser), the browser as the copy when the box does not answer and in the simulation. One Settings window in the menu; "Reset after a described change" moves there from the View menu. The pane arrangement stays per browser.
  result: preferences follow a person from one screen to another on a Skipper
  est: 1w
  difficulty: med
  id: settings-on-device
  audit: 2026-11-25 plan: 1) a typed settings record with GET and PUT /api/settings on the box, per account, device defaults when signed out; 2) the page reads and writes it, browser copy as fallback; 3) a Settings window; 4) FEATURES rows.
- [ ] **First account from the device with a setup code, and accounts in the UI** — step B of docs/decisions/local-user-settings.md, decided 2026-11-23. A box with no accounts and no PINs serves a first-run page that asks for a one-time setup code shipped with the device, then makes the first account (integrator). An integrator adds, disables and resets people in the UI; a person changes their own password. An operator's session lasts a shift (about 12 h, no idle end); engineers and integrators keep 1 h and 15 min idle. Draft PR: it touches sign-in.
  needs: where the setup code comes from on a preloaded Skipper (a label, or a file beside the pairing files)
  depends: settings-on-device, setup-code-source
  est: 2w
  difficulty: high
  id: first-run-and-accounts
  status: ready
  audit: 2026-11-25 plan (draft PRs, it touches sign-in): 1) the setup-code source and first-run page; 2) add, disable and reset people in the UI; 3) a person changes their own password; 4) the operator's 12 h session.
  decided: 2026-11-25 Nick: the setup code is printed on a label on the device, so assembly gains a step: the code is made and the label printed per unit.
- [ ] **The project download carries its history** — Nick, 2026-11-23: a Skipper's revisions (the newest 200 per project, `project/history.rs`) exist only on that device, and the .zip holds the files alone. The download gains the revisions (who, when, what, the recorded instruction), and an upload of such a .zip brings them back, so a backup made by hand is whole. Bounded and checked like any upload; an older .zip without history still loads.
  result: one file is a complete backup of a project
  est: 3d
  difficulty: med
  id: history-in-download
  audit: 2026-11-25 plan: 1) a history entry in the zip (who, when, what, instruction); 2) the box export writes up to 200 revisions; 3) import restores them, bounded and checked; 4) hostile-input tests, and an old zip without history still loads.

- [ ] **Nothing a user can download: what is missing** — Nick, 2026-11-23: "are these going to be releases in the repo users can download?" Not today. Shipping publishes the site and the playground in the browser (Pages serves master's docs/) and the relay; someone who wants Helmsman on their own hardware has to clone and build. Missing, smallest first: (1) no tags, no GitHub Releases and no version that means anything across the four repositories the release train promotes together; (2) no box binary to download: CI builds and tests it on Linux, macOS and Windows and keeps none, and the box is a binary plus a folder (`--ui DIR`, the canvas), so a download is an archive or an installer, not one file; (3) no Skipper image to download: the Box image workflow builds one and boots it under QEMU, then keeps only the boot log and the checksum for 14 days, and a preloaded image needs the setup code of card `first-run-and-accounts` first; (4) signing: an unsigned binary is stopped by Gatekeeper on a Mac and by SmartScreen on Windows; (5) the Windows job is not reliably green (card `windows-intermittent-failures`), so a Windows download is not ready to promise. Next door, supervictor already does the Linux half of this: a release workflow, tags (v0.2.1), one static musl binary, and an installer that checks the sha256 and installs a systemd unit (`scripts/install-endpoint.sh`); start from that pattern. UPDATE 2026-11-23: Nick chose a Linux binary with an installer first, after supervictor's. Built as a draft PR: `.github/workflows/release.yml` (a `v*` tag builds static aarch64 and x86_64 archives of the box, its editor and its unit, with checksums), `scripts/install.sh`, `scripts/package-release.sh`, docs/INSTALL.md. Left on this card: the Skipper image, macOS and Windows, signing, and what a version covers across the four repositories (the tag is the box's version for now).
  needs: Nick on what a download is first (a Linux binary with an installer, the Skipper image, or both) and on what a version number covers
  result: a proposal with the first download named and sized; nothing built before it
  est: 4h
  difficulty: med
  id: downloads-for-users
  audit: 2026-11-25 partly: the Linux release workflow, install.sh and docs/INSTALL.md landed (#595). Left: a first tag and release, the Skipper image, other platforms.

- [ ] **Describe carries the file's whole history, compacted at 64k tokens** — Nick, 2026-11-23: "context should carry over to where the entire commit history is slowly fed into context but compacted every 64k tokens". Today an instruction carries the program, the tests and the last five instructions on that file (`instruct::recent`), nothing older and nothing summarised. Decided in the interview that day: (1) each past change contributes its instruction, its one-line reason and its result (landed, what did not pass), not its code changes; (2) one record per file, as the last-five list is; (3) past 64k tokens the oldest part is summarised by Helmsman and the summary replaces it, the recent changes staying word for word; (4) the record goes as a cached prefix (prompt caching), and time and cost are measured on the agent ladder before it is the default. Open in the build: where a file's summary is kept so it is made once (a page in the project's History is the likely place, so it follows the project to a Skipper and into a download with `history-in-download`), how tokens are counted in the page (an estimate from characters is enough to decide when to compact), and what a hand edit, a rename or a set value contributes (their History entries already read as instructions). UPDATE 2026-11-23: the page and box half is built (`instruct::history`, `fit`, `tokens`, the `Summary for:` page read back, a box reading all its pages). Left: the relay's summarising profile and what writes a summary page with it, the record as a cached prefix, and the measurement on the agent ladder; until then a history past 64k tokens is cut at its old end and says how many changes it leaves out. UPDATE 2 (2026-11-24): the browser kept only 20 revisions, so the history could not outlive them; it is now a log kept in the project's agent.md (`SimProjects::log`), in the download too, and the page asks the relay's `summary` agent (agent-relay draft #69, `helmsman/summary-v1`) to write up a file's oldest changes when its history passes 64k tokens or the log is three quarters full. Left: the same on a box (it reads its 200 revisions' pages and asks for no summary), the history as a cached block at the front of the message, the measurement on the agent ladder, and reading real summaries from the live model.
  needs: a relay profile for the summarising step and its deploy (Nick); the page half works without it and compacts nothing until the relay serves it
  result: a change asked today knows what was asked and decided on that file from its first version on
  est: 1w
  difficulty: high
  id: history-in-context
  audit: 2026-11-25 partly: the page and relay halves are built (instruct.rs, summary-v1). Left: the box side, the cached block, the ladder measurement.

- [ ] **Zoom ladder: the Process and Machine levels** — Nick asked for seven levels (interview 2026-11-23): Code, Wiring, Statements, Sections, Programs, Process, Machine. Five are built (`fold::ladder`, `st_ladder.rs`): each is a card painted over its own blocks, so a level needs something on the canvas to stand over. Past the one program's card there is nothing yet: Programs needs every program and function block of the project placed in one world with the calls between them, Process needs the sequences (a CASE on a state variable, `fold::states`) drawn as steps with the live one lit, and Machine needs the devices and their points placed around the programs. Start with the world of several programs; the other two sit on it.
  result: zooming out past one program shows the project, then what the machine does, then the machine
  est: 4d
  difficulty: hard
  id: zoom-ladder-upper
- [ ] **Take out the old three-level zoom** — `fold::zoom`, `st_zoom.rs` and their tests (`st_zoom_tests.rs`, `tests/fold_zoom.rs`) close folds and lay the diagram out again at each level. It has been off since 2026-11-23 (`SEMANTIC_ZOOM`), the zoom ladder replaces it, and only its own tests turn it on. The corner-control tests in `st_zoom_tests.rs` are of what ships and move to the ladder's tests.
  depends: zoom-ladder-upper
  est: 0.5d
  difficulty: easy
  id: zoom-old-levels-out

## The ST plan (INVESTIGATION.md, decided 2026-11-07 and 2026-11-08)

The investigation's "Suggested order" as Nick agreed it (`st-plan-order`), with his calls folded in: lints shown, two block, a pragma overrides (§1, 2026-11-07); the §4 dialect table with VAR_GLOBAL inside one module only; OSCAT or standard names for the library with plain words in the tooltips; no LSP or formatter until `version-control` ships; no SCL for the early release; IEC 61131-3 cited from secondary references marked as such (docs/SOURCES.md). Done already: the ruSTy 1.0.8 bump (#282) and `helmsman-plc test` with JUnit XML (#289). Each dialect "yes" adds oracle cases, property tests, the no_std build check and a new relay profile id (a served profile never changes).

- [ ] **Process-control library: 10-15 blocks with tests** — in `crates/helmsman-iec/src/library.st`, written here (OSCAT's license is unverified and it leans on pointers): `PID` with anti-windup and bumpless manual/auto transfer, `RAMP`, `HYST`, `SCALE` (raw to engineering units, using the signal's `unit`), `LIMIT_ALARM`, `FILTER_PT1`, `DEBOUNCE`, `PULSE_COUNT` and a rate, sequence helpers. Decided (Nick, 2026-11-08: `st-library-names`): OSCAT's pin and block names where they exist, the standard's otherwise, no aliases; the plain-words explanation of every block and pin goes in its tooltip and its help page, the way the UI rule "tooltips on everything" already works. Each block gets requirement tests of its own, the non-IEC ones are marked in the docs as `BLINK_PATTERN` is, and the agent's profile gets the list (a new id).
  result: an analog loop the agent writes calls a tested PID instead of a hand-rolled PI with its own windup bug
  est: 4d
  difficulty: med
  depends: st-var-in-out
  id: st-library
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan: 1) HYST, SCALE and LIMIT_ALARM with requirement tests (smallest); 2) FILTER_PT1, DEBOUNCE, RAMP; 3) PID with anti-windup and bumpless transfer, oracle and property tests; 4) PULSE_COUNT and sequence helpers; 5) tooltips and help pages; 6) a relay profile that lists them.

- [ ] **matiec as a second oracle** — the ruSTy pattern (docker/rusty, `HELMSMAN_RUSTY`, tests/oracle.rs) for `beremiz/matiec` (GPL-3.0, the compiler inside OpenPLC and Beremiz): `docker/matiec/`, `HELMSMAN_MATIEC`, a harness that wraps the generated C in a scan loop and prints the variables, and the oracle corpus run through both. Where helmsman and ruSTy disagree a third vote says which is wrong, and "your OpenPLC project runs here unchanged" becomes a tested claim (with the PLCopen corpus that exists). Executed only, never linked.
  result: a disagreement with ruSTy is settled by a vote, and OpenPLC agreement is measured
  est: 1d
  difficulty: med
  id: matiec-oracle
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan (needs Docker): 1) a docker/matiec image that builds matiec and a C scan-loop wrapper, the GPL tool executed only; 2) a harness that prints variables as run-st.sh does; 3) HELMSMAN_MATIEC in tests/oracle.rs; 4) triage what disagrees.

- [ ] **The diagram draws a VAR_IN_OUT argument as a read** — found while building `st-var-in-out` (#446, 2026-11-15): a function block can write through its in-out argument, but helmsman-diagram's lowering draws that argument as an input wire only, so the write the block makes is missing from the picture. Draw the in-out as both ways, and extend the generators in crates/helmsman-diagram/tests/props.rs to call blocks with in-outs and functions with outputs.
  result: a block that takes a buffer by reference shows that it changes it
  est: 1d
  difficulty: med
  id: diagram-in-out
  asked: 2026-11-15
  by: claude-session (board run)
  audit: 2026-11-25 plan, about a day: 1) lower an in-out argument as a read-and-write edge (helmsman-diagram lower.rs, the node model); 2) layout and drawing for both directions; 3) props.rs generators for FB calls with in-outs and functions with outputs.

- [ ] **A relay profile that knows VAR_IN_OUT, function outputs and VAR_GLOBAL** — the box and the playground accept them (#446 and this card's PR, 2026-11-15) but the served agent profiles still tell the model they are refused. A new profile id in ../agent-relay must say: VAR_IN_OUT on a FUNCTION_BLOCK only, given by name at every call, a plain variable, element or member of exactly the same type, never a located variable, a PROGRAM input, an expression or a constant, never the same variable twice in one call; VAR_OUTPUT on a FUNCTION, every one bound with `=>` at every call (a call that leaves one out is refused, as ruSTy does: Nick, 2026-11-15); VAR_GLOBAL blocks stand before the first POU (or in a CONFIGURATION and its one RESOURCE), every POU of the module reads and writes a global by name, VAR_EXTERNAL may name one with exactly its type and no initial value (VAR_EXTERNAL CONSTANT for a CONSTANT global, or to read only). Refused: a local with a global's name, a global passed as a VAR_IN_OUT, VAR_GLOBAL inside a POU, VAR_ACCESS and VAR_CONFIG (a CONFIGURATION's TASK lines and program instances are read past with a warning); a global is never a port and no other module sees it. A served profile never changes, so this is a new id and the box's and the playground's default moves to it.
  needs: a session in ../agent-relay for the profile, then your deploy
  result: the agent writes and repairs programs with the syntax the compiler now takes
  est: 3h
  difficulty: low
  id: relay-profile-dialect-2
  asked: 2026-11-15
  by: claude-session (board run)
  audit: 2026-11-25 low-hanging but in ../agent-relay: copy the newest playground profile to a new id with the in-out, function-output and VAR_GLOBAL rules; draft PR there, then Nick's deploy.

- [ ] **Strings as box signals** — `STRING[n]` is in the language (card `st-string`) but only inside a program: `Program::ports()` skips a STRING port, so it is no signal. Left for here, each with the skip it has today: `/api/signals` and the values stream, trend records, OPC UA, `RETAIN` (state.rs format; the checker refuses RETAIN on a string and names this card), I/O binding, `helmsman_io::Value`, nodecore `SignalValue`, helmsman-sim and its panel controls, and a requirement test sharing a STRING port with its program (`spec.rs` only shares ports `ports()` lists, so `ASSERT_EQ_STRING` can compare text the test builds but not a program output). Also the playground's `SYSTEM` prompt (helmsman-canvas/src/claude.rs, "Not supported: STRING and WSTRING") and a new relay profile id that tell the agent STRING, ASSERT_EQ_* and the string functions exist; a served profile never changes, so that is a new id in ../agent-relay.
  needs: your call on the signal type for text (a bounded string of at most 255 bytes, as in the language), then a session in ../agent-relay for the profile and your deploy
  result: a program's text outputs show on the desk, in trends and over OPC UA, survive a restart, and a requirement test reads them; the agent knows the syntax
  est: 4d
  difficulty: high
  id: st-string-signals
  asked: 2026-11-17
  by: claude-session
  depends: string-signal-type
  audit: 2026-11-25 plan: 1) Value::Str through helmsman-io and Program::ports, output only; 2) /api/signals, the values stream and the node's SignalValue; 3) trend and an OPC UA String node; 4) RETAIN; 5) shared STRING ports; 6) sim controls; 7) a relay profile id.
  decided: 2026-11-25 Nick: one text signal type, bounded at 255 bytes as STRING is in the language.

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
  status: blocked
  audit: 2026-11-25 waits for a new relay profile in ../agent-relay and its deploy.

- [ ] **Explain mode answers timing questions** — arpus4KM on PLCTalk wants to ask "How many milliseconds will this logic take to execute". With `scan-stats` the box knows each module's measured time, and the instruction budget gives a worst case. Let explain mode answer "how long does this take", "what is the longest path" and "when does this timer finish", with the numbers from the box.
  needs: scan-stats
  result: the timing questions engineers ask an AI get real numbers
  est: 2d
  difficulty: med
  depends: scan-stats
  id: explain-timing
  asked: 2026-11-07
  by: claude-session
  audit: 2026-11-25 plan: 1) a read-only box tool with measured scan time, jitter and the instruction-budget worst case; 2) a static longest-path and timer-finish calculation over the checked program; 3) a new explain profile with evals (agent-relay, Nick's deploy). Open: whether the instruction budget is a sound worst case without a cost per operation.

- [ ] **One page that is the ST dialect** — every third-party tool (blark, iec-checker, UniTest, vscode-st, STweep) breaks on vendor dialects. A single, written description of what Helmsman's ST accepts and refuses, with the IEC section for each (once `iec-standard-source` lands), is useful to the agent, to an engineer importing code, and to anyone building a tool for it. Generate the "refused" list from the checker's messages so it cannot drift.
  needs: iec-standard-source answered
  result: docs/help/st-dialect, linked from every "not supported" message
  est: 4h
  difficulty: low
  id: dialect-page
  asked: 2026-11-07
  by: claude-session
  audit: 2026-11-25 low-hanging, next wave: docs/help/st-dialect.md with the refused list generated from the checker's messages; cite secondary references (CLAUDE.md), not the standard's sections.

- [ ] **A project pins the library version it was built with** — TwinCAT users report library mismatches that block going online "even with otherwise identical code", and OSCAT breaks on new CODESYS service packs. Once the process-control library (INVESTIGATION §6) exists, `library.st` will change between releases. A project records the library version it was approved with, keeps that behaviour until an engineer upgrades it as a proposal, and the diff shows which blocks changed.
  needs: the process-control library carded and started (`st-plan-order`)
  result: a project approved last year behaves the same after a box update
  est: 1d
  difficulty: med
  id: library-pin
  asked: 2026-11-07
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for the process-control library (st-library). Plan after that: 1) a version constant in library.st and a field in the project; 2) old library text kept per version; 3) load a project against its pin; 4) an upgrade as a proposal with the block diff.

- [ ] **Accounts as the default sign-off, PINs as the fallback** — the liability fear ("a tort lawyer will argue you don't know what you're doing") is answered by a named person signing off. With PINs only, the audit records a role ("engineer (PIN)"), not a person. Make first-run setup create a named engineer account, show the approver's name on every version in the history, and keep role PINs for sites that want them.
  needs: the first-boot flow (`box-sku`)
  result: every change in the audit trail names the person who approved it
  est: 1d
  difficulty: low
  id: named-signoff
  asked: 2026-11-07
  by: claude-session
  audit: 2026-11-25 partly: named accounts exist and PINs are refused once one exists. Left: the first-run account (first-run-and-accounts) and the approver's name on every version.

- [ ] **TOD, DATE and DT: the rest** — built: the language, the box's wall clock in its system time zone (`--time-zone`), the canvas simulation's browser time, and the prompts (relay PR agent-relay#54: `helmsman/v14`, `playground-v7`, `tests-v2`). Left: point the box (`--relay-agent`), the demo (`RELAY_DEMO_AGENTS`) and the test writer at those ids once the relay is deployed; DATE/TOD/DT ports are no signals (with STRING: `st-string-signals`); `SPLIT_DATE`/`SPLIT_TOD`/`SPLIT_DT` and the `L` forms are not built.
  result: a controls engineer can read a DATE, TOD or DT port beside the other signals, and the agents write with the new ids
  est: 4h
  difficulty: low
  depends: st-string-signals
  id: st-tod-date
  asked: 2026-11-08
  by: claude-session
  status: ready
  audit: 2026-11-25 partly: relay profiles v14 and v15 exist. Left: the box default is still v7, SPLIT_* and the L forms, and ports as signals (st-string-signals).

- [ ] **TwinCAT .TcPOU / .TcDUT / .TcGVL import** — INVESTIGATION.md §8: XML wrapping ST in CDATA, so the ST parser does the work and the importer is small; the L5X pattern (convert, refuse what does not map, list every problem). Beckhoff code leans on METHOD / PROPERTY and VAR_GLOBAL, so wait for `st-corpus` to say how much of it would import before spending the day.
  result: a TwinCAT project's POUs open in the editor with every refusal listed
  est: 1d
  difficulty: low
  depends: st-corpus, st-var-global
  id: tcpou-import
  asked: 2026-11-08
  by: claude-session
  audit: 2026-11-25 plan, on the L5X importer's pattern: 1) an XML reader with CDATA extraction for one TcPOU; 2) TcDUT and TcGVL; 3) a refusal list for METHOD and PROPERTY; 4) the Import menu entry. First check the st-corpus ranking for how much Beckhoff code would import.

- [ ] **A language server and a formatter on helmsman-iec** — decided (Nick, 2026-11-08: `st-edit-outside-canvas`): not yet. Until `version-control` puts a project in git the canvas is the only way into the code, so an LSP (diagnostics, hover types, go to definition, the lints) has no users, and the formatter needs a lossless token layer first (the lexer drops comments and folds case) and could only ever run as an explicit Format command (agent prompt Rule 2). Revisit once an engineer has cloned a project.
  result: VS Code, Neovim and Zed get Helmsman's dialect and lints; proposal diffs show only real changes
  est: 1w
  difficulty: high
  depends: version-control, st-lints
  id: st-lsp-formatter
  asked: 2026-11-08
  by: claude-session
  notes: 2026-11-17 Nick's pick: the lints are built (#509); the language server and the formatter stay parked.
  status: blocked
  audit: 2026-11-25 parked by Nick until version-control and an engineer who has cloned a project.

- [ ] **Siemens SCL import** — decided (Nick, 2026-11-08: `st-scl-import`): not for the early release. SCL (TIA Portal) is an ST dialect (`#local`, `"DB".member`, REGION, IEC_TIMER instances, its own instructions); a source-level converter would follow the L5X pattern. Carded here so it is not lost: pick it up when a customer brings TIA code, and estimate then (likely 1-2 weeks).
  needs: a customer with TIA Portal code to import
  result: an SCL block converts to ST with every problem listed
  est: 2w
  difficulty: high
  id: scl-import
  asked: 2026-11-08
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for a customer's SCL sample.

### Fewer, more descriptive blocks on the diagram (Nick, 2026-11-15: "do everything")

Built: n-input gates, the fold layer (`helmsman_diagram::fold`) with the canvas drawing it, the comparison with its literal, range, latch, edge, selector and set/reset (on as the diagram opens), and sections named by a comment and arms (fold on request). Each card below is one more rule or renderer. Gates,
chips and idioms are on as the diagram opens; arms, sections and output
cards fold on request and the choice is kept in `diagram.layout`.


## Product vision (interview 2026-10-29)

Working title until naming lands. A small, opinionated controller for factory edge boxes (Pi 5 reference, headless, browser UI), for non-mission-critical work: low-frequency detection, shift-aware scheduling of its own outputs, on-floor signalling (stack light, andon, screen). Data is pushed elsewhere (webhooks, OPC UA); the box keeps no historian.

Agentic first. The agent (Claude, behind our relay) is the primary programmer. Every module is Structured Text the agent writes and an engineer can tweak; the existing parser, checker, interpreter and ruSTy oracle are its feedback loop. The canvas is the operator's window into the code: ST rendered as a live diagram with values on wires and the reason each output is on. The canvas is the app shell; chat lives in its side panel.

Extension tiers: data (params, signals, schedules) → ST modules → WASM (drivers, heavy code; M3). Users are layered: operator (view, ask), engineer (approve, edit ST, PIN), integrator (modules, policy). The approval policy depends on tier and role. No user theming: the look is fixed. Param changes go live with undo; logic changes get a preview; anything that can move an output needs an engineer PIN plus a sim replay.

Hard limits, enforced by the host and never by the prompt: no writes to external PLCs (CIP is read-only toward them); no output change without human approval; modules have no network access (core sinks publish); the agent never touches the Rust core. Offline: everything installed keeps running, chat degrades to rule-based param edits (no local model). Business: open core (runtime + canvas MIT; relay, fleet and verified marketplace paid) plus a preloaded box. Standalone first, fleet-shaped data model from day one.

Update 2026-10-29 (I/O mapping): "This device is the PLC, it just may talk to other slave/client devices. This will be a master/server." The box masters its devices and writes to them through the I/O map; read-only stays the rule for other controllers it only observes (a CompactLogix it reads tags from). Agent-written drivers run in the WASM tier with one extra capability: a socket to their own device's endpoint.

## Roadmap M1: bench demo, talk → module → light

Demo: on the bench, "flash the light when CompactLogix tag X is high for 5 s". The agent drafts ST, the canvas shows it live against real values, a PIN approves it, the Pi's output toggles, and undo restores the previous snapshot.

- [ ] **A stack light on the E1213's outputs** — the demo's light. The Pi's GPIO path (`do:N` of `--gpio-chip`) is built but the Pi is out of the loop (Nick, 2026-11-07), so the light is a 24 V stack light on the E1213's DO through the map, as decided on 2026-10-31 (stack-light-hw). `BLINK_PATTERN(EN, MODE, PERIOD) -> Q` is in helmsman-iec's library.st (off, steady, slow, fast, double flash, on the scan clock).
  needs: a 24 V stack light wired to E1213 DO-00..02 with its supply
  result: the demo's light, blinking from ST
  est: 2h
  difficulty: low
  id: gpio-outputs
  status: blocked
  audit: 2026-11-25 waits for the stack light wired on the bench.

- [ ] **M1 demo run** — the sentence at the top of this section, on the bench, recorded. scripts/m1-demo.sh brings the box up from a clean deploy.
  needs: CompactLogix on the bench
  result: the thing to show people
  est: 1d
  difficulty: med
  depends: gpio-outputs
  id: m1-demo
  notes: plan (m1-demo-plan, 2026-10-31): E1213-only demo first, the CompactLogix version after.
  status: blocked
  audit: 2026-11-25 waits for the CompactLogix and the light; scripts/m1-demo.sh, which the card names, does not exist.

- [ ] **Tabletop I/O bench** — one Modbus TCP remote I/O block with 24 V DI/DO on the Studio link (192.168.3.x), outputs DO0-3 wired back to inputs DI0-3, a 24 V supply, and a Pi GPIO to 24 V output stage for do:N. Then `conformance --endpoint <ip>:502 --loop coil:0=di:0 ...` against it.
  needs: hardware bought and wired (any Modbus TCP 8DI/8DO block; note its manual's register map and whether it numbers from 0 or 1)
  result: the harness passes on real hardware, and agents have a bench that proves writes physically
  est: 4h
  difficulty: low
  id: io-bench
  status: blocked
  audit: 2026-11-25 waits for the Modbus block to be bought and wired.

## Roadmap M2: floor-ready core

- [ ] **A library of known devices, and their points as tags in ST** — Nick, 2026-11-02 (while the E1213 came up): build a library of devices and the drivers for them (where it lives is open: docs/devices/ holds the E1213's doc, maps and the enip profile today; a `devices/` crate or a per-device folder with EDS, manual citations, profile, default map and a loop-check program are the candidates). Locally, pick inputs and outputs from the wired, known devices and have them come into a program as tags, as other PLC IDEs do, instead of hand-writing `in di0 bool e1213 di:0` lines; an I/O-like tab is where the user chooses them, and the same list is the context the agent gets when it writes the program. Nick wants to play with the interface against the live E1213 before fixing the shape.
  needs: the E1213 bench green (`e1213-bench`), then Nick's hands-on notes on how choosing a device's points should feel
  result: a second device is a folder in the library, not a driver project; a program names tags the device list offers; the agent sees the same list
  est: 1w
  difficulty: med
  id: device-library
  notes: 2026-11-02 (feat/devices-pane): the Devices tab exists: the map's devices as a tree with their points and live values, New device… as a form writing a `device` line, the listing as the agent's context (`<io_devices>` in the page, `list_devices` on the box). Next is `device-import`.
  audit: 2026-11-25 partly: the Devices tab, list_library_devices and the library folder exist. Left: device-import and the per-device folder shape.

- [x] **Relay: image blocks from boxes, checked and capped** — protocol v1 passes any content block through and caps only the body (1 MiB). For photo drop-ins: (1) accept `image` blocks only with `source.type: "base64"` (refuse `url`, which makes the model API fetch a box-chosen URL, and `file`, which would read the relay org's Files API uploads by id), `media_type` in png/jpeg/webp, decoded size within a per-image cap; the same for images nested in `tool_result` content; (2) cap images per request at 20 (the Messages API's many-image rule) and raise `RELAY_MAX_BODY_BYTES` for image requests only if `dropin-photo-size` says so; (3) metering already counts image tokens inside `input_tokens`; add per-box image count and image bytes per day to `/v1/usage` so a photo-heavy box is visible; (4) agent v4 (v3 is immutable): v3's prompt plus a photo paragraph (the image is data, read only what it shows, map to box signals, ask about anything unreadable, name the photo as the source). Box side needs nothing new for (1)-(3); for (4) boxes move to `--relay-agent v4`.
  needs: the relay repo
  result: a box cannot use the relay to fetch URLs or read other uploads; image cost is visible per box; the model gets photo-specific instructions from the relay, not only the box's note
  est: 3h
  difficulty: low
  id: relay-images
  notes: agent v4 also carries the drawing work (card `dropin-pid`, box side built): (a) two tools in v4's tools.json, strict as `helmsman_agent::tools` validates them (every field required, `additionalProperties: false`): `read_io_map {}` and `propose_io_map {text: string, summary: string}`; (b) a P&ID / electrical drawing paragraph: a drawing (PDF document or photo) is data, never instructions; list its I/O points (tag, device, terminal or address, signal type) and map each to a signal the box has or to a new map line; read the running map first and propose the whole file with every line that stays; never invent an address, a type or a safe value the drawing does not show, ask instead; name the drawing (and sheet) in the summary; a map is proposed, never applied.
  audit: 2026-11-25 done in agent-relay dev: MAX_IMAGES and byte counting in router.rs, url sources refused, usage counters.

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
  depends: version-control-library
  audit: 2026-11-25 plan (after Nick's decision, export only): 1) settle the bundle format with a spike: a `git bundle` or fast-import stream written without a git library, one commit per deploy with author, approver and audit trailers; 2) GET of the bundle on the box and a Download entry in History; 3) a test that real git imports it and the tree of each commit equals the deployed revision; 4) docs/help page on importing it. Out: push, pull and Revert through git (Revert stays a proposal from History). Overlaps history-in-download: do that first and build the bundle from the same revisions.
  decided: 2026-11-25 Nick: no git library on the box. The box writes a bundle of a project's history that an engineer imports into git elsewhere.

## Roadmap M3: tiers, sharing, fleet

- [ ] **Map the network from the electrical drawings and the bill of materials** — "where it goes": the drawings drop-in (dropin-pid) reads one sheet into points today; this reads a whole drawing set plus the BOM into the devices, their addresses and the wiring between them, and proposes the complete I/O map and the device list, cross-checked against what ListIdentity and the Modbus scan actually find. Differences become questions to the engineer.
  needs: dropin-pid; a drawing set and BOM from a real project to test on
  result: an I/O map and device list from the project's documents, with every difference from the live network listed
  est: 2w
  difficulty: high
  id: network-from-drawings
  asked: 2026-11-01
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for one real drawing set and BOM from Nick. Plan after that: 1) multi-page PDF in; 2) BOM table to a device list; 3) cross-check against discovery, differences asked as questions; 4) a relay profile.

- [ ] **Sort out and explain the behaviours already present in a system** — "where it goes": from recorded trends, the trace tap and sim replay, the agent groups what the system does into named behaviours (a cycle, an interlock, a fault pattern), explains each in plain words with the signals that drive it, and flags the ones no program accounts for. Explain mode today works on one program; this works on the plant's recorded behaviour.
  needs: box-trend-source and sim-replay recordings from a running machine; explain-mode
  result: a behaviour list for a machine, each one explained and traceable to its signals
  est: 2w
  difficulty: high
  id: behaviour-map
  asked: 2026-11-01
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for recordings from a running machine. Plan after that: 1) trend and trace recordings exported to one file; 2) a profile that groups cycles; 3) flag signals no program accounts for; 4) a list that traces to signals.

- [ ] **A running template of the machine you already built** — "where it goes": the holistic one. From the device list, the I/O map, the drawings and the observed behaviours, the agent proposes a complete starting project (programs, plant model for the simulation, signal names, a screen) that runs in the simulation on day one, so the engineer edits a working machine instead of describing one from a blank page. The machine starters are the hand-written version of this today.
  needs: network-from-drawings, behaviour-map, the starters as the shape to generate
  result: a new project that runs in the simulation from the plant's own documents and recordings
  est: 3w
  difficulty: high
  depends: network-from-drawings, behaviour-map
  id: machine-template
  asked: 2026-11-01
  by: claude-session
  notes: 2026-11-17 Nick's pick: skipped until real documents exist.
  status: blocked
  audit: 2026-11-25 waits on network-from-drawings and behaviour-map; no first step until then.

- [ ] **Fleet console + OTA** — the relay grows into the fleet backbone: box registry, snapshot sync, signed updates of the core.
  est: 3w
  difficulty: high
  notes: 2026-11-02 night: the node half moved: helmsman-node runs the scan bench and Modbus (PRs #193, #199); its own API + site beacon and the Site view across nodes are what this card is now.
  id: fleet-console
  needs: a decision taken (interview 2026-11-02): no console host at all; see notes
  notes: Nick 2026-11-02 (board): "I like this because it also allows me to set it up like my ../github/supervictor project. And now I realize that I may not have enough cores to do what I need with the network, uplink, communication, and program / process that may get complicated." Interview 2026-11-02: the fleet lives on the ESP32-POE2 as "every node is its own box": each node runs the runtime, serves its own API and announces itself on the LAN (`--site`), the fleet view is the canvas's Site view across nodes from any laptop, history and uplink go node → relay. Nothing in the middle to host. This card becomes: the Site view across boxes and nodes, the relay as the uplink, OTA of the node firmware (A/B slots and the rollback are built, card node-ota, 2026-11-18: the node pulls an image from a URL; signed images are what is left); depends on the edge spike (`node-modbus`) for the node half.
  status: ready
  depends: ota-signing-key
  audit: 2026-11-25 plan: 1) sign node OTA images and check the signature on the node; 2) a Site view that lists nodes from the site beacon; 3) node to relay history uplink.
  decided: 2026-11-25 Nick: the firmware signing key is his, kept offline; he signs each release.

- [ ] **TLS on a node's data port** — your answer to `node-tls-first-peer` (2026-11-13): needed eventually to push production data out over a second network port, with strict isolation between the control runtime and the data publication; not until there is a board with more than one Ethernet port. The recipe (mbedtls-rs at the fork's main with Homebrew clang) compiles for the P4 (docs/EDGE.md Phase 3) and is all that exists.
  needs: a node board with two Ethernet ports
  result: a node publishes data over TLS on one port while its control traffic stays on the other
  est: 3d
  difficulty: high
  id: node-data-tls
  asked: 2026-11-13
  by: claude-session
  status: blocked
  audit: 2026-11-25 waits for a node board with two Ethernet ports.

- [ ] **Vision sources** — deferred per interview; card kept so the signal model leaves room for camera-derived signals.
  est: 2w
  difficulty: high
  id: vision
  status: blocked
  audit: 2026-11-25 deferred by interview; nothing to build until a camera use is wanted.

## Roadmap M4: marketplace and the box

- [ ] **Community marketplace** — publish/browse/install bundles; floor per market-floor, then an AI review on install that explains risk in plain words and can only refuse.
  est: 3w
  difficulty: high
  notes: built: the walking skeleton (2026-11-02, PR #196: signed index, bundle fetch, CLI bundle index/fetch, POST /api/market/list and /install through the bundle import and the approval policy, --market-allow-local) and the AI review on install (2026-11-18, feat/marketplace-install-review here and feat/install-review-profile in agent-relay, both drafts): the relay profile helmsman/review-v1 (no tools, strict JSON verdict, three evals), the box's review stage between the bundle's checks and the proposal (a refusal stops it, a clean review goes on the proposal and the audit lines, no review stops at 409 until an engineer's session sends without_review), and the canvas's Install form and review panel on the Proposals screen. Left: nothing is live until the relay is promoted to main and deployed, then one make eval-live run of the three review scenarios on the deployed model; accounts, publisher uploads and payments (Nick, 2026-11-17: none of them yet); mirrors.
  id: marketplace
  needs: the relay deploy (Nick: agent-relay dev to main, then the deploy) before the review is live; accounts and payments stay out until asked
  notes: Nick 2026-11-02 (board): do it now with my own judgement; interview: walking skeletons. Cut: a static, signed index of bundles (a JSON file and the bundles in a public repo, signed by trusted publishers), a box lists it and installs from it through the existing bundle path and approval policy. Nick 2026-11-17 (interview): the next slice is the AI review on install, no money or account infrastructure.
  status: blocked
  audit: 2026-11-25 waits for Nick's relay deploy, then one make eval-live run.

- [ ] **Box SKU** — reproducible Pi image, first-boot pairing with the relay, provisioning script, enclosure/BOM for the preloaded kit.
  est: 2w
  difficulty: high
  notes: walking skeleton done 2026-11-02 night, PR #198: scripts/pi-image.sh (pi-gen in Docker, pinned ref), helmsman-firstboot pairing files → LoadCredential, docs/BOX.md; not booted. 2026-11-18 slice (branch feat/box-image-ci, draft; relay half agent-relay feat/pair-by-code, draft): BUILT: .github/workflows/box-image.yml builds the image on an arm64 runner and scripts/box-boot-test.sh boots it under QEMU raspi3b (the image's own kernel; watchdog driver kept out), a scripted relay answers the redeem, the job waits for /api/health and the canvas page and keeps the serial log; `helmsman-plc pair` + helmsman-firstboot take a pairing-code, relay-box-id and relay-url (relay-allow-http for an http relay) off the boot partition; relay: POST /v1/pairings (admin token named in PairAdmins) and POST /v1/pair, tests on memory and SQLite. DONE 2026-11-19: the CI boot is green (run 44445977754: build, boot, API up after 205 s, canvas served, one pairing redeem; 23 min); pi-gen.ref is pinned. CHANGED 2026-11-18: QEMU's raspi3b froze the guest in sysinit (cause not found), so CI boots the image's rootfs on the generic `virt` machine with a pinned Debian cloud kernel (deploy/pi-image/qemu-kernel.env; its modules copied into the CI copy), which proves the userland and first boot, not the Pi's kernel/firmware/device tree. LEFT: enclosure and BOM; deploying the relay half (PairAdmins parameter, an admin token, `make test-integration` for the DynamoDB test nobody has run, maybe a WAF on /v1/pair); the first boot of the image on a real Pi (OWED: CI no longer runs a Pi machine); a console prompt for the code was left out.
  id: box-sku
  needs: the walking skeleton only, for now
  notes: Nick 2026-11-02 (board): do it now with my own judgement; interview: walking skeletons. Cut: a reproducible Pi image build script (pi-gen or rpi-image-gen, the box binary and the canvas preinstalled, the systemd unit, first boot pairs with the relay from a one-time code) checked by booting it in QEMU in CI; no enclosure or BOM yet. Est for the skeleton: 2d.
  notes: the ready mark is dropped on 2026-11-18: what is left (enclosure, BOM, the relay deploy) is Nick's, not a worker's.
  status: blocked
  audit: 2026-11-25 waits for the enclosure and BOM, the relay deploy and a first boot on a real Pi.

- [ ] **EtherCAT master via ethercrab** — ethercrab (MIT/Apache-2.0, pure Rust) as the driver; EtherCAT slaves' PDOs mapped into the runtime's %I/%Q. Master only: a Pi cannot be an EtherCAT slave (needs an ESC chip).
  needs: a USB 3 gigabit NIC for the Pi (its one port is the Studio link) and one EtherCAT I/O slave to test against (e.g. a bus coupler + digital I/O terminal)
  result: an ST program toggles a real EtherCAT output from the Pi
  est: 1w
  difficulty: high
  id: ethercat-master
  notes: parked (fieldbus-hardware, 2026-10-31): no hardware until a customer asks.
  status: blocked
  audit: 2026-11-25 waits for a USB NIC and an EtherCAT slave.

- [ ] **PROFINET IO device (RT class 1)** — own Rust implementation over raw Ethernet (DCP, LLDP, RPC connect, cyclic RT frames); IRT is out (needs hardware). Lab use only: shipping a real device needs a PI vendor ID and certification.
  needs: a PROFINET controller to test against (a Siemens PLC, or a software controller) and access to the PROFINET spec (PI membership) or a decision to work from public captures + p-net's behaviour
  result: a PROFINET controller sees the Pi as an IO device and exchanges cyclic data with the ST program
  est: 3w
  difficulty: high
  id: profinet-device
  notes: parked (fieldbus-hardware, 2026-10-31): no hardware until a customer asks.
  notes: 2026-11-10: the controller side is built (`profinet-controller`), and with it a simulated device (`helmsman_profinet::device`: DCP, one AR, cyclic data, alarms) that is most of this card's protocol work. Still parked as a product: LLDP, the port and interface submodules, record reads and certification are not started.
  status: blocked
  audit: 2026-11-25 waits for a PROFINET controller and a customer.
