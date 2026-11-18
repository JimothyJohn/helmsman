# TODO

Done cards were removed on 2026-11-06 (git history has them). Three sections up top: decisions only you can make, things only your hands can do, and what an agent builds next. The roadmap below is the long view.

Freeze (Nick, 2026-11-14): until the front door (the promotion of what is built) is live and holds up, nothing new merges outside it and the Agent's reliability. Bug fixes only on every other card.

## Open questions for Nick


- [ ] **Keep `sha2` in the canvas for the spec's PDF fingerprint?** — #475 (spec mode on a box) records each PDF in `spec.md` by name, size and SHA-256, since a project file is text and cannot hold the PDF. Nothing in the canvas computed SHA-256, so it added `sha2` 0.11.1 with default features off: the version the box, its agent and the I/O crate already build, no new crate in Cargo.lock, and `wasm_deps` is green. Dropping it leaves name and size only, so two different PDFs of one name and size could not be told apart later.
  options: keep | drop, name and size only
  id: spec-pdf-sha2
  asked: 2026-11-16
  by: claude-session

- [ ] **The MDrive kept moving when the controller went silent, and its utility shows Halt Motion on: what does Get say?** — Measured on the bench unit (2026-11-16): slewing at 5000 steps/s over the cyclic connection, it stops on a Forward_Close and when the run/idle header goes to idle, but when the controller simply stops sending (a pulled cable, a box that dies) it was still slewing 10 s later, until `SL 0` arrived over its MCode port. Your photo of the Configuration Utility's E.I.P. page (2026-11-17) shows "I/O Implicit connection tests: Halt Motion On", which reads like the setting for exactly this, so either the page was showing its own default and the drive has it off, or on does not cover a connection that times out. Please press Get beside Halt Motion and tell me what it shows; if it comes back Off, set it On and I repeat the test. If it really is On and the drive still runs, the stop has to be an MCode program on the drive (my pick then; it writes to the drive's flash) or a hardware input.
  options: Get shows Off: I set it On, test again | Get shows On: write the MCode program | Get shows On: I wire a hardware stop | accept it on the bench for now
  id: mdrive-comms-loss-stop
  asked: 2026-11-16
  by: claude-session

## Needs your hands

- [ ] **Watch the IMPACT67's LEDs while a session switches it, and wire one output to one input** — Why it matters: the block was driven on 2026-11-16 with nothing on its ports and nobody looking, so no channel is confirmed and nobody knows what its outputs do when the controller goes away (after a Forward_Close its output assembly still read the last values written). Steps: (1) put an M12 jumper from pin 4 of port 0 to pin 4 of port 1 (or tell the session which channels you joined); (2) tell a session "watch the Murr with me": it sets port 0 pin 4 as an output, switches it, and asks what the channel LEDs do, then closes the connection and stops sending in turn and asks again.
  needs: the block powered at its DHCP address (10.0.0.73 on 2026-11-16; a session finds it by ListIdentity)
  result: the library entry's channels confirmed on a unit, and its question about outputs after a lost connection answered
  est: 15m
  difficulty: low
  id: murr-outputs-watch
  asked: 2026-11-16
  by: claude-session

- [ ] **Run spec mode against a real model, in the simulation and on a box** — #471 and #475 are tested with scripted answers only; no model has yet read a scope of work and answered in the format (a `Why:` line, the PROGRAM with a comment on each input and output, one scenario a requirement, a `requirements` block). Paste a real scope of work into "From a spec…" in the Agent tab of the playground, then the same with a PDF on a box whose agent is online, and note what comes back: names, comments, how many requirements are tested, whether the retries are used up.
  needs: a scope of work or a product document you would really build from; a box with its agent reachable for the PDF half (the try box, started for this and stopped after)
  result: spec mode is known to work on real documents, or the first real failures are on the board
  est: 30m
  difficulty: low
  id: spec-mode-real-run
  asked: 2026-11-16
  by: claude-session

- [ ] **Deploy the relay so the Agent's edits and its one question go live** — Why it matters: the page on dev answers a long program with only the lines that change and asks one question while it works, but only where the relay serves those agents; the deployed relay does not yet, so the published page behaves as before. agent-relay #40 (merged to its dev when green) adds the demo's `edits`, `ask` and `check` agents; nothing in template.yaml changes. Steps: (1) promote agent-relay dev to its default branch; (2) deploy as usual (`scripts/deploy.sh`) with `RELAY_MODEL=deepseek/deepseek-v4.1-flash-1017` (decided 2026-11-16 from the ladder: 45 of 45, fastest and cheapest of the four, a dated snapshot) and `DemoAddressTurnsPerHour` raised from 40 to about 120: one instruction is now up to three turns (the question, the change, the check), so the old limit closes the free chat for everyone about three times sooner; (3) `curl -s -H "Origin: <the page's origin>" <demo url>/demo/v1/status` should list `"agents":["ask","check","edits"]`; (4) tell a session, which then runs the ladder against the deployed demo and looks at the card in a browser.
  needs: agent-relay #40 merged, your promote and deploy
  result: the published playground answers a one-line change to a long program in about a third of the time, and asks its question
  id: agent-edits-deploy
  asked: 2026-11-15
  by: claude-session

- [ ] **Fit the P4's PoE module and measure its isolation** — Why it matters: a node in a cabinet is powered by its Ethernet cable alone, and today no run has proven that. The PoE module turns the cable's 48 V into the board's supply; if its output is not isolated from the cable side, a USB cable plugged in at the same time ties the laptop's ground to the PoE supply's return, and current can flow through the laptop's USB port. Nobody knows yet which it is, so `scripts/node-flash.sh` makes you pull Ethernet before every USB flash. One measurement settles it: the prompt goes away, or becomes a hard rule. the Waveshare ESP32-P4-ETH has run on USB and the bench switch without its PoE module (docs/EDGE.md Phase 3, "On the board"). Before USB and PoE are ever connected together: with the module fitted and the board powered from PoE only, measure for continuity and resistance between the RJ45 shield and PoE return on one side and USB ground on the other (unpowered first). Then a run on PoE alone, USB out: the node is at http://192.168.3.56:8080 (press-5).
  needs: the PoE module on the board, a meter, a PoE port
  result: "isolated" or "not isolated" written into `board.rs` and `scripts/node-flash.sh` (which then keeps or drops its Ethernet-unplugged prompt and `NODE_P4_NO_POE`), and the node seen running on PoE alone
  est: 30m
  difficulty: low
  id: p4-poe-isolation
  asked: 2026-11-10
  by: claude-session

- [ ] **Rebuild the try box at the dev tip** — it came back when you logged in at the Studio (pid 86028 on 2026-11-13, :8081 answers, the E1213 up), but on the build of 2026-11-08 (e7f1ae2, #300), about 90 merges behind dev: no device profiles, no device configuration, no glyphs. Best done after `studio-box-no-io` lands, so the rebuilt box can be switched off the devices for P4 work.
  needs: `~/github/helmsman-try/scratch/16-rebuild.sh` from your terminal at the Studio (it builds at the dev tip, signs, and restarts the LaunchAgent)
  result: https://10.0.0.41:8081/ at the dev tip with the E1213 up
  notes: check after: `curl -sk -H "Authorization: Bearer $(cat ~/github/helmsman-try/scratch/try-state/api-token)" https://127.0.0.1:8081/api/devices` shows iologik_e1213 up.
  est: 10m
  difficulty: low
  id: try-box-login
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

- [x] **Open raw Ethernet to your user on the Studio (one sudo script)** — Why it matters: only once a device has to be reached over PROFINET, and that is no longer first (Nick, 2026-11-13: EtherNet/IP and Modbus TCP are what plants run). PROFINET's cyclic data is raw Ethernet frames, not IP, and on macOS a program sends those through `/dev/bpf*`, which only root can open. Parked with the PROFINET cards. PROFINET's cyclic data, alarms and station discovery are Ethernet frames, not IP, and on macOS a program sends them through `/dev/bpf*`, which is root-only here (`/dev/bpf0` is root:wheel 0600; you are already in `access_bpf`, so Wireshark's helper was installed once and is gone). Until this runs, everything PROFINET is proven against the simulated device only: the BPF code has compiled and parsed synthetic buffers and has never carried a frame.
  needs: a device that has to be reached over PROFINET; then `~/github/helmsman/scratch/27-bpf-access.sh` in the native terminal (sudo; again after each reboot), then `! ~/github/helmsman/scratch/28-profinet-scan.sh` here
  result: the scan lists the PROFINET stations on en0 (none is a fine answer with nothing plugged in: it proves frames go out and the listen works); a session can then run a box against a real device without sudo
  done: 2026-11-15: /dev/bpf* are root:access_bpf 0660 (Wireshark's ChmodBPF LaunchDaemon is installed, so it holds across reboots; no sudo step needed). `scratch/28-profinet-scan.sh en0` from a session, no sudo: raw Ethernet 0x8892 opens, DCP Identify sent, 0 stations (nothing PROFINET on the wire).
  est: 5m
  difficulty: low
  id: profinet-raw-access
  asked: 2026-11-10
  by: claude-session

- [ ] **Bring up the Novanta IMS Liberty MDrive over EtherNet/IP or Modbus TCP** — Why it matters: the first device on the bench that is not discrete I/O. A motor drive is where the device profile's "kinds" get decided (step (g) of `device-profiles`: a drive's enable, speed, position and fault as the same four channels whatever the vendor), and where a comms loss has to stop something that moves. PROFINET first is out (Nick, 2026-11-13), so this starts on the protocols plants run. What its files say (docs/devices/novanta-mdrive.md, read 2026-11-10): one Ethernet interface speaks Modbus TCP (the factory default), EtherNet/IP, PROFINET IO or MCode/TCP, one at a time, picked with Novanta's TCP/IP Configuration Utility; default IP 192.168.33.1; its EDS is in the firmware zip. Steps: Modbus TCP as it ships (no vendor tool needed), read status and position, then a jog and a move to position from ST with the fault path and the comms-loss stop tested; then EtherNet/IP from its EDS, so one device compares two drivers; PROFINET later, if ever. Motion asks "cell clear?" once per session (hardware-guard).
  needs: your answer to `mdrive-comms-loss-stop`; the drive on the bench at 192.168.33.1 (the Studio reaches it through `sudo ifconfig en0 alias 192.168.33.10 255.255.255.0`, gone at a reboot)
  result: a device doc with citations, a profile and a library entry; an ST program that jogs and positions the axis and stops it on comms loss
  est: 2d
  difficulty: med
  notes: 2026-11-16 night, on the bench unit (LMDCE853C, EtherNet/IP mode, identity revision 2.36, `LMDCM 6.013`): EtherNet/IP came first because that is the mode it arrived in. Done: the connection (docs/devices/novanta-mdrive.md "On the bench": a run/idle header its EDS does not declare, which `enip-io` now corrects from the refusal; it sends 1.3 to 1.5 times slower than the RPI it grants; it resets the TCP session after 2 to 3 s and keeps exchanging I/O; UDP 2222 only), the vendor's EDS resolving and connecting, a 20-minute hold, a slew at 5000 steps/s and a stop from the cyclic image, the library entry (helmsman-devices `devices/novanta/lexium-mdrive-ethernet/`). Found: the output assembly is not laid out as the manual says, and a silent controller does not stop a slew (both are questions above). 2026-11-17: the unit's mapping is known (Nick's photo of the Configuration Utility; docs/devices/novanta-mdrive.md has it, and it explains the move at the first Forward_Open). Not explained, for later: a small MA through bytes 28..31 and over MCode did not move the drive (errors 25, 30). Left: the profile and the kinds (a drive's enable, speed, position, fault), MA and MR, the ST program with the fault path, and Modbus TCP for the comparison (the drive speaks one protocol at a time).
  status: blocked
  id: mdrive-bringup
  asked: 2026-11-10
  by: claude-session

- [ ] **Bring up a Universal Robot over EtherNet/IP and Modbus TCP** — Why it matters: the first device with a large mixed image (floats, 32-bit registers, bit fields), so it tests what a profile can say about a value beyond on and off, and a robot is something customers already own. PROFINET first is out (Nick, 2026-11-13). Read 2026-11-10 (docs/devices/universal-robots.md): EtherNet/IP input assembly 100 (480 bytes), output assembly 112 (224 bytes), RPI from 2 ms (its EDS); Modbus TCP server on 502, function codes 1..6, 15, 16, general purpose registers 128..255, robot state from 256, every value an unsigned 16-bit word (angles in mrad, positions in tenths of a millimetre, signed values converted by hand), the unit id ignored. EtherNet/IP and PROFINET cannot be on together. URSim is no stand-in: users report its Modbus registers stuck, so this needs the robot. PROFINET (its GSDML is read, slots 1..10) only if a customer asks.
  needs: the robot on 192.168.3.x, its model and PolyScope version, EtherNet/IP enabled under Installation > Fieldbus; a cleared cell before anything moves
  result: docs/devices/universal-robots.md "On the bench" with what each interface accepted; a profile with both transports; an ST program that reads the robot's state and sets a general purpose register; the register quirks held as tests
  est: 2d
  difficulty: med
  status: blocked
  id: ur-bringup
  asked: 2026-11-10
  by: claude-session

- [ ] **Bring up the Murrelektronik IMPACT67 E DI8 DO8 (55086)** — Why it matters: every implicit-messaging finding so far is against one Moxa unit on one old firmware; a second vendor's block is what shows whether the EtherNet/IP driver is right or only right for that Moxa. A second unit also ends the contention for the E1213's single connection: the P4 owns one, a Studio box the other. What is already done without the unit (helmsman-devices `devices/murrelektronik/`, from its EDS and handbook, 2026-11-12): the 55086 is EtherNet/IP only, vendor 640, product code 55086, input instance 100 (4 bytes), output 112 (2 bytes), RPI 10 ms by default; its folder has the doc, an enip map and a `.device` profile. It ships at rotary 000, DHCP/BOOTP, so it is also the first device `box-dhcp-server` can address with nobody touching it. Left for the bench: power it, let it take an address, Scan, add by click, and record what its Forward_Open accepts.
  needs: `murr-outputs-watch` (a jumper and your eyes) for the channels
  result: the block found, added and driven from the library's profile with no hand-written map; its doc gains "On the bench"
  est: 4h
  difficulty: med
  notes: 2026-11-16 night: the block that arrived is a **55089** (DIO8), not a 55086, at 10.0.0.73 by DHCP, revision 1.2. Done (docs/devices/murrelektronik-impact67-e.md, helmsman-devices `impact67-e-dio8-dio8`): identity and assemblies confirmed, the connection opened from the library's sizes and held 20 minutes at 10 ms, its setup sent as configuration data (`config=109 config_data=00ff00`, new in `enip-io`: out of its box it refuses instance 110 with 0x0119). Left: Scan and add by click from the canvas, a channel seen switching, what the outputs do on a lost connection.
  status: blocked
  id: murr-block-bringup
  asked: 2026-11-10
  by: claude-session

- [ ] **Update the bench E1213 from firmware 3.1 to 3.3** — your answer (2026-11-13): 3.3 only, by a session if it can. It cannot, for two reasons: helmsman-devconf maps the console's Firmware Update page (`06_4.htm` to `06_4_1.htm`, helmsman-devices `devices/moxa/iologik-e1200-web-console.md`) and does not drive it, and a scripted upload that goes wrong leaves the bench's only EtherNet/IP unit unusable until Moxa recovers it; and the 3.2 and 3.3 files were not found on Moxa's site (2026-11-13 search: the product page offers the latest release, 4.0.1). Why it matters: v3.3 is the release Moxa says fixes "Ethernet/IP cannot establish second input-only session", so after it a P4 node and a Studio box could both hold a cyclic connection to the one unit instead of taking turns. Steps: (1) in the canvas, Devices > iologik_e1213 > Configuration > Save as project file, so its settings can be restored; (2) get the E1213 firmware v3.2 and v3.3 files from Moxa (support site, ioLogik E1213, software packages; if only 4.0.1 is listed, ask Moxa support for the two older builds); (3) http://192.168.3.60 > Firmware Update, upload 3.2, wait for the unit to come back (ping, then the overview page shows V3.2); (4) the same with 3.3; (5) check on the console that EtherNet/IP is still enabled, then tell a session "E1213 is on 3.3". The session then runs `scratch/enip-fo-probe.py` and `scratch/enip-hold-probe.py` (Input Only beside an Exclusive Owner, the port-2222 change) and rewrites docs/devices/moxa-iologik-e1213.md.
  needs: your hands on the unit's web console, and the two firmware files
  result: the unit on V3.3 with its settings unchanged; a measured yes or no on two cyclic connections at once
  est: 1h
  difficulty: low
  id: e1213-firmware-update
  asked: 2026-11-10
  by: claude-session

## Next to build

- [ ] **Named start states for the simulation: set a machine up once, pick it again** — Nick, 2026-11-16: "the user should be able to rapidly configure the state of the machine for various scenarios manually and with the agent before starting." Built first (#493): an input set by hand while the program is stopped is what it starts with, the values outlive Stop, and a quiet line says nothing reacts yet. Decided by interview that day: this card follows, simulation only; a stopped box only reminds. Left: a start state gets a name ("part seated, guard open") and is saved in the project as a text file the diff shows, one `input = value` a line; the I/O tab lists them above Inputs and a click sets every input of one while the program is stopped (running: asked first, since it moves the machine); the Agent writes one on request, checked against the program's inputs and their types before it is offered. Open in the design: whether a start state also names kept values (a step number, a latch), which the panel cannot set today. The Agent's first half is built (2026-11-17, PR feat/agent-start-state with agent-relay `start-v1`): a field above the Inputs takes the state in words, the Agent proposes input values, and nothing is set until Set these; it works once the relay is deployed with the `start` agent, and it is not yet a saved, named state.
  needs: #492 and #493 in dev; the Agent's part needs a relay profile with the new block (a new id, Nick's deploy)
  result: a list of named start states in the I/O tab, saved with the project, that the Agent can add to
  est: 8h
  difficulty: med
  id: sim-start-states
  asked: 2026-11-16
  by: claude-session 4a05a85d

- [ ] **A spec run into the project that is open** — spec mode (#471, #475) always makes a new project; Nick's ask was to "parse and add it". Left out because a kept file (`spec.md`) cannot be saved one at a time: `Projects::save` on a box and `SimProjects::add_file` in the page take only names `file_name` knows, and a box has no atomic write of several files short of a whole-project zip. Shape: the same window with a project open offers "Add to this project"; the spec goes in as an instruction on the open program file (`Scope::File`), the review shows what changes (I/O added, requirements added or replaced), and Accept writes the file, the tests and the new section of `spec.md` together.
  needs: nothing
  result: more documentation dumped in later lands in the same project, with its spec kept beside the first
  est: 2d
  difficulty: med
  id: spec-into-open-project
  asked: 2026-11-16
  by: claude-session

- [ ] **PDFs in the playground's spec window** — on a box a spec can be a PDF (#475, a drop-in on `/api/chat`); in the page it is pasted text, `.md` or `.txt` (#471). The page's free route carries no documents at all (the relay's demo function refuses them, 256 KiB a request), so this is for a visitor's own key and the local proxy: `claude.rs` builds `content` as a string today and would send a `document` block.
  needs: nothing
  result: a PDF scope of work builds a project in the page with a key, and the window says why not on the free route
  est: 1d
  difficulty: med
  id: spec-pdf-in-page
  asked: 2026-11-16
  by: claude-session

- [ ] **History says "From a spec" for a project made on a box** — the page's revision is labelled "From a spec: <document names>"; a box's shows "uploaded a zip", because Accept is `POST /api/projects/{name}/zip` and that route takes no label. Give the zip route an optional label (bounded, one line, audited as any save) and have spec mode send it.
  needs: nothing
  result: a spec-made project's first revision names its documents on a box as in the page
  est: 3h
  difficulty: low
  id: spec-zip-label
  asked: 2026-11-16
  by: claude-session

- [ ] **A starter io.map a box loads: no screen tile for a number input** — `sim_projects::example_io_map` writes `in NAME int screen NAME` for a numeric input, and a box refuses that map (`IoMap::validate`: "a screen tile is something the box shows: an out signal"). Nothing loads io.map in the page, so it shows only when such a project reaches a box: an example with a numeric input copied by "Use this device", a download and upload. Spec mode on a box avoids it (`sim_projects::box_io_map` writes no line and the review lists the inputs that need a device); the page's examples and its spec mode still write the bad line.
  needs: nothing
  result: every io.map the page writes loads on a box, checked in a test with `helmsman_io::text::parse` and `validate`
  est: 3h
  difficulty: low
  id: starter-io-map-loads
  asked: 2026-11-16
  by: claude-session

- [ ] **helmsman-dhcp: a client moves off its running lease (found by its own property test)** — Why it matters: the DHCP server's promise is that an address handed out in an ACK stays that client's while the lease runs; a block that is moved to another address mid-lease drops its fieldbus connection. On 2026-11-15 `cargo test --workspace` failed once in `crates/helmsman-dhcp/tests/hostile.rs`, `no_request_sequence_breaks_the_invariants`: "00-07-46-00-00-03 moved off its running lease", left 192.168.3.102, right 192.168.3.101. It is rare (the next three runs of that test passed), so CI can fail on it at random on any PR. To reproduce every time, add this line to `crates/helmsman-dhcp/tests/hostile.proptest-regressions`: `cc 32024c1c7cf224596c6941b14f9b5192e7b4dba694ca7a7b2831f9d34cc24c4b`. The shrunk case begins with a DHCPREQUEST from that client with ciaddr 192.168.3.100 and ends with its DHCPDISCOVER. Not fixed here: it was found while gating an unrelated change (the Devices tab's wired line), the crate is another task's, and the seed was left out of that branch so dev stays green until the fix lands with it, regression first.
  needs: nothing
  result: the seed is in the regressions file, the test passes with it, and the lease rule holds
  id: dhcp-lease-moves
  by: claude-session (iodd-meta)

- [ ] **A sensor's IODD as its metadata: from the library to the point it is wired to** — Why it matters: Nick, 2026-11-14: "ioLink is a core part of this but may fundamentally be different so let's focus on it for digital input and output device metadata more than anything else", and of sensors that day: "Their output? It should be deeply known ahead of time." A prox or a valve sits on a digital channel of a remote I/O block and is never spoken to; its vendor's IODD (every IO-Link device has one) says who makes it, its variants, how each connector is wired, whether it switches without a master, and what its signals and their states are called. Built: helmsman-devices `scripts/iodd-draft.py` drafts a library record from an IODD (`[identity.iolink]`, the order numbers, `[wired]` with each connector's wires, `[[signals]]` with `on` / `off` names, `[[settings]]` with factory values such as PNP and high active; helmsman-devices #33, #38), with five real records as drafts (ifm IF6123, O5D100, PN7092, KQ6005, SICK WTB4FP; IODDs from IODDfinder through `docs-fetch.py`). In Helmsman a map line names what is on a point, `in part_present bool rio di:3 wired=ifm/if6123`: the box keeps it, the Agent's `list_devices` gives it, and `devlib` finds the record by it, by an order number or by the IO-Link identity. The records are on the library's `main` (checked 2026-11-15), which the box and the playground read. Left, in order: (1) `wired.switching` for each record (which signal the switching wire carries with no master, normally open or closed): an IODD does not say and the Smart Sensor Profile leaves it to the vendor, so it comes from each data sheet with its page; none could be fetched on 2026-11-14 (ifm's pages are rendered by script, its asset service answered 404 and 403), so this needs the PDFs by hand or a vendor source that can be fetched; (2) done: the library builds `record.json` beside each wired device's `device.toml` (helmsman-devices #39), `helmsman_project::devrecord` reads it, the box serves its brief from `GET /api/devices/library?wired=VENDOR/MODEL`, and the Agent's `list_devices` brings it for every wired point; (3) done: the Devices and I/O tabs say what is wired under the point (vendor, model, kind; on/off names, factory settings and the draft note in the tooltip), on a box and in the playground (which reads the index and `record.json` itself), and "What is wired here…" on a point searches the library by the number on the label and writes or clears `wired=` on its map line (canvas `wired.rs`), tested at the panes' default widths; (4) `agent-names-io` uses it: a sensor picked from the library comes with what it is and what on means, and only its function is asked; the box has the tool for it, `list_library_devices` (search by vendor, model or label number; each match's `wired` value and brief), which MCP clients can call today; the relay's profile for it is `helmsman/v11` (agent-relay #38: v10 plus the tool and a prompt section on `wired=`, `wired_devices`, `switching_wire_carries` and recording it through `propose_io_map`; evals `wired-sensor-by-label` and `wired-sensor-not-in-library` pass live in every regime). Left of it: Nick's relay deploy, then a box opts in with `--relay-agent v11` (the box's default is still v7; moving it is a box change after the deploy); (5) the simulation's plant takes the sensor's kind and states from the record; (6) more records, sorted by what Nick's benches and customers use.
  notes: 2026-11-15, (1), a session's search (not yet checked page by page). ifm IF6123/IF6124/IG6615, KQ6005 and PN7092: the data sheets give pin 4 = OUT (and PN7092 pin 2 = OUT2) but only "NO/NC, PNP/NPN configurable" with no factory value, so the default has to come from the IODD's parameter defaults (already in the records' `settings`) or the operating instructions' factory table. ifm O5D100 instructions (11457730/00, p. 6): pin 4 = OUT1 normally open, pin 2 = OUT2 normally closed; its 2013 data sheet says DC PNP. SICK 1105505 is WTB4FP-32161120A00; its sheet (2024-03-28, p. 4) says pin 4 = QL1 light switching (HIGH with an object), pin 2 = dark switching, push-pull. 1122374 (same type family as 1222998) says the same; 1223017, 1123751 and 1123757 were not found by their own type code, and other WTB4FP variants differ on pin 2, so each needs its own sheet. Readable hosts: media.ifm.com/dam, automatyka24.pl, assets.alliedelec.com; ifm.com itself answers 403/404 to scripts.
  needs: the vendors' data sheets for (1); the relay deploy of helmsman/v11 for (4)
  result: a wired sensor is picked from the library by name, and the program, the I/O list and the Agent know what it is and what its signal means without anyone typing it
  id: iodd-metadata
  by: claude-session (iolink)

- [ ] **Order an IO-Link master transceiver board and one sensor** — Parked (Nick, 2026-11-14, later the same day: IO-Link is for the metadata of digital input and output devices more than anything else; see `iodd-metadata`). Nothing to order until a device has to be spoken to over IO-Link. Why it would matter then: the IO-Link master is built and proven against its own simulated device only (helmsman-iolink: wake-up, COM1/2/3 detection, startup, cyclic Process Data, retries, loss). Nothing has been on a wire, and the P4 needs a master transceiver between its pins and the sensor's C/Q line (24 V, the wake-up current pulse). Two candidates, facts from distributor pages on 2026-11-14 (the vendors' own pages timed out, so neither datasheet has been read): (1) Analog Devices MAX14819 / MAX14819A, a dual-channel master transceiver with L+ supply controllers, SPI, "integrated framers" or external UARTs, 9 to 36 V, 48-pin TQFN, evaluation kit MAX14819EVKIT; (2) ST L6360, a single master port (COM1, COM2, COM3), configured over I2C with the data on a USART, 18 to 32.5 V, evaluation board STEVAL-IOM001V1 (Arduino UNO R3 headers). My pick is the MAX14819 kit: two ports on one board and the framer in the chip takes the bit timing off the P4, which runs the scan and the network beside it. Not checked for either part: how the wake-up pulse is made, and the 3.3 V logic levels against the P4. Any IO-Link sensor with a published IODD will do as the first device; one with 16 bits of input or fewer avoids the interleaved mode the master does not have yet.
  needs: a 24 V supply on the bench; the part ordered; its datasheet read by a session before the driver is written (first step of `iolink-p4-line`)
  result: a transceiver board and a sensor on the bench, named on this card with the pins they are wired to
  options: MAX14819EVKIT | STEVAL-IOM001V1 | another part
  id: iolink-transceiver
  asked: 2026-11-14
  by: claude-session (iolink)

- [ ] **IO-Link on the node: IODD import, the `iolink` device line, then the P4's line driver** — Parked behind `iodd-metadata` (Nick, 2026-11-14, later the same day: the sensor is wired to a plain digital channel and its IODD is its metadata; speaking IO-Link to it comes after, if at all). What is built stays on dev, tested, with nothing calling it. Why it mattered: Nick, 2026-11-14: everything should be reachable, IO-Link sensors included, through a master of our own on the edge node. Built so far (docs/ARCHITECTURE.md `helmsman-iolink`): the message codec, one master port and a simulated device as state machines without I/O, with the timing rules and a noisy line as tests. Also built: `iodd` reads a device's IODD file for its vendor and device id and its Process Data layout by name, and `pd` reads a value out of the octets by that layout (all 22 of the community's example IODDs read; no vendor's file tried yet). Also built in helmsman-nodecore's `iolink`: the `device NAME iolink port=N ...` line and its `pd:OFFSET[:uN|iN]` points, a `Port` that runs one on the master and gives the inputs by point and takes the outputs, and `Iodd::map_lines`, which writes those lines from an IODD; proven from an IODD to values on the simulated device. No firmware calls it, and the node's own `iomap::parse` still refuses an `iolink` line, since no node has a port. Left, in order: (1) try the reader on the IODD of the sensor that gets ordered, from IODDfinder; (2) `iolink-node-image`: a place for `int` points in the node's program image (today it is 4 bytes of bool bits a direction, for the Modbus block's points, so an IO-Link number has nowhere to land), and a map with no Modbus block (today a node's map needs exactly one); (3) `iolink-p4-line`: the transceiver driver on the P4 behind `Step` (wake-up, exchange with a timeout), `iomap::parse` taking `iolink` lines on a board with ports, the io task polling each `Port`, after `iolink-transceiver`; (4) the first real sensor, its capture kept as a test, and only then any wording a user reads (`iolink-membership`). Owed before a device with more than 2 octets of Process Data and M-sequence code 0: the interleaved TYPE_1_1 / TYPE_1_2 mode, which a master shall support (Table A.10 NOTE 3). Later cards: ISDUs (parameters, identity strings), Events, data storage, a second port, a box reading a node's ports.
  needs: nothing for (2); the sensor chosen for (1); `iolink-transceiver` for (3) and (4)
  result: a node's program reads an IO-Link sensor's Process Data by name from a deployed map
  id: iolink-node
  by: claude-session (iolink)

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

- [ ] **Device configuration: the agent's read-only tools** — the agent (and MCP) read a device's configuration, never change it: a `read_device_config` tool (facts, the export, the drift from the saved `.devconf`) through `api::devconf::config_view`, wrapped in the `<box-data>` envelope like every plant text. No set, restart, import or password tool, and the password is in no tool result (`ConfigView` has no field for it). The box's engineer gate stays the only way a unit is changed; the agent may propose a setting in words.
  needs: nothing
  result: "why does this Moxa drop its outputs when the controller stops?" is answered from the unit's real watchdog and safe values
  est: 1d
  difficulty: low
  id: devconf-agent-tools
  asked: 2026-11-13
  by: claude-session
  notes: 2026-11-13: built as #374 (draft). Its last CI run failed in helmsman-plc tests/agent.rs:2152 `an_unreachable_relay_still_gives_the_boxs_explanation_then_an_error`; not looked into yet. Then `gh pr ready 374 && gh pr merge 374 --auto --merge` (classifier-denied once).

- [ ] **The library says what has not been proven on a unit, entry by entry** — Nick, 2026-11-17: an entry is finalized by the first user who implements the device, so until then the library has to flag that it has not been proven out and exactly what has not. Today that is spread over three places a reader has to know: no `checked_on` means "from documents", a point's `source` may end "confirmed on a unit", and `[[questions]]` and the README's "Not run" hold the rest. The two entries of 2026-11-16 show the gap: the IMPACT67 55089 has `checked_on` although no channel was ever seen switching, and the Lexium MDrive has it although its output layout as delivered is unchecked. Shape to build (my recommendation, yours to change): a `[proof]` table in `device.toml` with one line per thing a unit can prove (identity, connection, each point group, outputs on a lost connection, each protocol), each `proven` with date and firmware or `unproven` with why; `checked_on` stays as the date of the last unit contact and stops meaning "done"; `build-index.py` carries a summary (`proven: 3 of 7`) and the list of what is open into `index.json`; `build-review.py` already walks every claim and gets the same marks; the site's library table and check page show "proven on a unit / not yet: channels, outputs on a lost connection" instead of one date; AGENTS.md step 5 tells the first implementer's agent to turn `unproven` lines into `proven` ones in the pull request that reports them. Backfill: all 45 entries, most of them entirely unproven, generated from what each file already says.
  needs: later (Nick, 2026-11-17); nothing else
  result: anyone picking a device sees at a glance what a unit has shown and what is still only on paper, and the first implementer's pull request closes those lines one by one
  est: 2d
  difficulty: med
  id: library-unproven-flags
  asked: 2026-11-17
  by: claude-session

- [ ] **Configure a device by talking to an agent, not through the vendor's tool** — Nick, 2026-11-17: come up with ways to configure devices agentically in the device library, casually and conversationally, so nobody has to use the vendor's software. What exists: helmsman-devconf reads and writes a Moxa E1200's settings through its web console, with the box's engineer gate as the only way a unit is changed (`devconf-agent-tools` is its read-only agent side). What the bench of 2026-11-16 showed is reachable without any vendor tool: the MDrive answers MCode on TCP 503 beside EtherNet/IP (every register read; `SL 0` stopped it), and its EtherNet/IP assembly mapping, protocol choice and "Halt Motion" setting live behind Novanta's Configuration Utility, which talks to that same port; the IMPACT67 takes its channel setup as configuration data in the Forward_Open (built: `config_data=`), and its address mode and parameterization mode through its web server; every EtherNet/IP device has the TCP/IP Interface object for its address. Shape to explore first, as an interview: a `configure` recipe per library entry (which channel reaches the setting: a CIP attribute, a web form, a vendor text port; what each setting is called in the vendor's words and in plain words; what is safe to change live and what needs a restart), read by one generic agent tool that proposes a change in words, shows the before and after, and applies it only through the engineer gate; settings the recipe cannot reach are listed as "needs the vendor's tool" so the gap is visible. First targets, because the bench has them: the MDrive's mapping page over MCode (capture what the utility sends, a hand-off `tcpdump`), the IMPACT67's parameterization mode, any device's IP address over CIP.
  needs: later (Nick, 2026-11-17); an interview on the shape before anything is built; the MDrive and the IMPACT67 on the bench
  result: "make pin 4 of every port an output" or "put the drive's mapping back to the default" is said to the agent and done, with the change shown first and recorded
  est: 1w
  difficulty: high
  depends: devconf-agent-tools
  id: agentic-device-config
  asked: 2026-11-17
  by: claude-session

- [ ] **`enip-io`: pick which of an EDS's exclusive owners to open** — Why it matters: from an EDS the driver opens the first exclusive owner. The IMPACT67 55089's EDS has four (without and with diagnosis, each for two configuration instances), and the first has no diagnosis bytes, so `eds=` alone cannot reach the short-circuit and overload bits; the library's profile works around it by giving the sizes itself. Shape: `connection=N` (the EDS's `ConnectionN`) beside `exclusive-owner` and `input-only`, refused when N is not an owner of the kind asked for, with the Devices tab's import offering the EDS's own connection names.
  needs: later (Nick, 2026-11-17); nothing else
  result: a device with several owning connections is driven from its EDS alone
  est: 4h
  difficulty: low
  id: enip-eds-owner-choice
  asked: 2026-11-17
  by: claude-session

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

- [ ] **Publish a checked device profile to the library** — the second tool, on the relay (it holds the only token that can push): a pull request into helmsman-devices' `dev` with `NAME.device` beside `device.toml` and `index.json` gaining `profile`, opened from what the user checked on the Devices tab.
  needs: a fine-grained GitHub token held by the relay alone (Nick, 2026-11-13: only the relay can push), limited to pull requests on helmsman-devices and kept in Secrets Manager; you create it from a hand-off script that names the scopes. No box ever holds a GitHub credential
  result: the library grows by a pull request each time a user maps a new device
  depends: device-profile-propose-box
  est: 1d
  difficulty: med
  id: device-profile-publish
  asked: 2026-11-13
  by: claude-session
  status: ready

- [ ] **PROFINET from the node: a real device, then the node image** — Why it matters: only once a device has to be reached over PROFINET. Parked with `profinet-raw-access` (Nick, 2026-11-13: EtherNet/IP and Modbus TCP first; PROFINET when a customer or a device needs it). the P4 runs the box's PROFINET controller (docs/EDGE.md "PROFINET on the P4"): raw frames tapped beside embassy-net in the Ethernet driver wrapper, the RPCs on a UDP socket, a `profinet` image that brings one device up and reports. On the board it boots, looks for its station every second and answers ping meanwhile; the node image with the same wrapper kept its API and its Modbus link. Nothing answered it: no PROFINET device is on the bench, and a capture to see its frames needs sudo. Left: (1) the first connection to a real device from the P4 (`NODE_BOARD=p4-eth PN_STATION=... PN_SLOTS=... scripts/node-flash.sh profinet`), with the cycle's jitter and late frames measured as the EtherNet/IP run was; (2) a `profinet` device line in the node's I/O map (helmsman-nodecore's `iomap`) so the `node` image masters a PROFINET device from a deployed map, as it does a Modbus block.
  needs: a device that has to be reached over PROFINET
  result: a node masters a PROFINET device at an 8 ms cycle with its late frames counted, from a map deployed over its API
  est: 3d
  difficulty: med
  id: node-profinet
  asked: 2026-11-11
  by: claude-session
  status: blocked

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
  notes: 2026-11-17, built (PR on feat/relay-v2-client): the box's v2 client (helmsman-agent session.rs and the v2 loop in agent.rs; docs/relay-protocol-v2.md vendored from agent-relay 26cd6dc9) and `--relay-agent v8`..`v11` on relay-held sessions; the chat route asks `GET /v1/agents/{agent}` once for the profile's `protocol` (a bare `v8` is retried as `helmsman/v8`), v7 and older stay on v1 and the default is still v7. Documents the relay stored are saved as drop-ins; a lost session is reopened once; the owed step goes first on the next message. Tested against a loopback relay scripted from the protocol doc (tests/session.rs, tests/agent.rs): no real relay has answered this client. Left: a bench run against the deployed relay (Nick's, relay card relay-deploy-v2), then v8 as the default; the model is not told a saved document's drop-in id; the MCP `fetch_document` route is still v1's; sessions are not deleted when a chat is dropped (7 days on the relay). agent-relay PR #7 merged 2026-11-07; spec mode (#475) went through v7's chat.
  id: relay-v2-client
  asked: 2026-11-05
  by: claude-session
  status: ready

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

- [ ] **Warnings on a box: while typing, and in the two answers that leave them out** — #346 shows the checker's warnings and the build's non-blocking problems in the check answer, the CLI and the canvas. Three places were left: (1) on a box the canvas shows them only after a check or a Deploy, where the browser simulation shows them as you type (the box view makes no check call per edit, and one per keystroke would fill the audit log: check on a pause in typing, or a check that is not audited); (2) `POST /api/proposals/check` (a draft's check) does not list them; (3) the deploy route's own answer does not, only the check before it. Also pin with a test what #345 and #346 do together: a kept file's "not used by this release" entry arrives in the check answer's `warnings` and as a row in the canvas.
  result: an engineer on a box sees "RETAIN here is not kept yet" while typing, and a proposal's check says the same as the project's
  est: 3h
  difficulty: low
  id: warnings-box-gaps
  asked: 2026-11-10
  by: claude-session

- [ ] **Look at a project with kept files in the canvas; fix the box's start line** — two small things seen during the 2026-11-10 run and checked by nobody. (1) #345 keeps a file this release has no use for (pump-curves.json, a newer release's file); the canvas code filters on Structured Text files wherever it was read, but the Files list of a box project that holds one was never looked at on screen: open one (`BOX=1 make canvas-smoke` with a kept file in the project, or a ui_test), and decide whether the file is listed, greyed with its "kept, not used" reason, or left out. `PUT` of a single file still refuses an unknown name, which is right while the canvas cannot edit one. (2) The box prints "API on http://...  (reads open; changes need the token)" at start; since read-lock (#323) every read needs a session, so the line is wrong.
  result: a kept file is shown the way that was chosen, with a test; the start line says reads need a sign-in
  est: 2h
  difficulty: low
  id: kept-files-in-canvas
  asked: 2026-11-10
  by: claude-session

- [ ] **Node firmware OTA: A/B slots, pull from a host, roll back on an unconfirmed boot** — Why it matters: today a node's firmware changes only over a USB cable from a laptop, so a node in a cabinet has to be opened to update it. This lets it take a new image over the network and fall back to the old one by itself if the new one does not come up, so a bad build cannot leave a machine without its controller. #312 put program deploy over the node's API (PUT /api/modules/NAME with the bearer token); the firmware itself still goes over USB (scripts/node-flash.sh). supervictor's ota/{install,policy} already has the shape to reuse: the esp-bootloader-esp-idf partition table with two app slots, write the new image to the idle slot, mark it, reboot, and confirm it from the running image once the API answers; an image that never confirms rolls back on the next boot. The node pulls the image from a URL the canvas or a curl gives it (POST /api/firmware {url, sha256}) so the host serves, the node fetches; the token guards it like every write. On the P4 (helmsman-node-p4); the ESP32 boards are closed (Nick, 2026-11-13). Test: a host-side test against the simulated flash (nodecore) for the slot state machine, and a bench run that flashes a build, confirms, then a deliberately broken build that rolls back.
  needs: the P4 on the bench (192.168.3.56, /dev/cu.usbmodem5C930154551): the node board since 2026-11-13, the ESP32 boards closed
  result: a node on the plant network takes a new firmware from a laptop without a USB cable, and a bad image cannot brick it
  notes: sized 2026-11-09 (not started, needs the bench): (1) a new partition table, nvs / otadata / phy_init / ota_0 / ota_1 on supervictor/edge/partitions.csv's model, one USB flash per board (espflash --partition-table, --erase-parts otadata; it moves the nvs region the settings and program live in, so the node is commissioned again); (2) port supervictor/edge/src/ota/{install,policy,http,manifest}.rs into nodecore (Slot trait installer with sector buffering, digest and read-back; the checksummed RTC record with MAX_PENDING_BOOTS; the strict response-head parser; 2.4k lines with their proptests, tied to supervictor_common::firmware types to replace); (3) the fetch task on embassy-net (Range + If-Match) writing through esp-bootloader-esp-idf 0.5's OtaUpdater (next_partition, activate_next_partition, set_current_ota_state), the boot policy at start, GET /api/firmware, POST /api/firmware {url, sha256}, POST /api/firmware/confirm; (4) on the P4: a push, a confirm, a forced rollback.
  est: 2d
  difficulty: high
  id: node-ota
  asked: 2026-11-09
  by: claude-session

- [ ] **A Studio box with the device links off, beside the P4** — your answer to `try-box-keep-or-retire` (2026-11-13): the Studio is for app and interface work, the P4 for runtime and embedded work, and the two should not both be on the devices; but the Studio box should still be programmable with everything except device comms, so it can run while the P4 has them. Today a box opens every link its project's map names, and the only way off the E1213 is to stop the box. Build a box setting (`--links off`, and the same as a switch an engineer can flip): the program, the canvas, the agent, proposals, history and the Monitor all work; no link opens; each input reads as the simulation would give it (the same plant model the page's simulation runs) and outputs go nowhere; the Devices tab and the Monitor say "device links off on this box" so nobody reads simulated values as the plant's. Then the try box runs that way during P4 work, and CLAUDE.md's try box entry says so.
  needs: nothing
  result: a P4 run against the E1213 no longer needs the try box stopped; the Studio box stays up for interface work meanwhile
  est: 1d
  difficulty: med
  id: studio-box-no-io
  asked: 2026-11-13
  by: claude-session

- [ ] **Devices tab: glyph buttons with their names, I/O grouped and collapsed, a search** — your answer to `glyphs-devices-buttons` (2026-11-13): "let the user choose through a subset of icons but still show their names. Collapse the I/O registers by default and group ins, outs, and datatypes so that it's easier for the user to organize. A simple pattern search feature for an input in real-time would be nice as well." Read as: (1) Scan, Edit, Delete and Use this device become `glyphs::action` buttons that keep their word beside the glyph (not glyph only, as the Agent tab's are); (2) a device's points start collapsed, and open grouped as Inputs, Outputs, then by data type inside each (BOOL, INT, REAL, ...), each group with its count; (3) a filter field at the top of the tab that narrows the list as you type, matching point names, channels and devices (plain substring and `*` wildcards). Assumed and to check with you on the PR: "choose through a subset of icons" means the glyph library's existing set, not a picker where the user assigns icons to devices. Panel width rules apply (`every_tab_leaves_the_panels_their_width`, a test at the default width).
  needs: nothing
  result: a device with 40 points opens as two short groups, and typing "DI-0" shows only those points
  est: 1d
  difficulty: low
  id: devices-tab-glyphs-io
  asked: 2026-11-13
  by: claude-session

- [ ] **The live free chat: the leftovers** — the free chat is on master (#359, promoted 2026-11-13; the published playground's page allows the relay's demo address). Left from its card: (1) your first look at the live page (a page served from another origin is refused by design, so it was never seen against the live relay); (2) the survey's buttons name model vendors, which the voice rule forbids; (3) the help pictures (docs/help/img) may still show the key field: `make help-shots`; (4) the key route can come out of the page (claude.rs Route::Direct, key_ui, the model picker) as its own change; (5) not exercised yet: the alarm filters on a real close, and the survey route against DynamoDB.
  needs: your look at https://helmsman.advin.io/playground/ for (1); the rest needs nothing
  result: the published chat has no key field, no vendor names, and pictures that match it
  est: 3h
  difficulty: low
  id: free-chat-followups
  asked: 2026-11-13
  by: claude-session

- [ ] **P4 node, after the bring-up: PoE alone, two connections on the E1213, G4 and G5** — Why it matters: the P4 is the runtime board (Nick, 2026-11-13), and these are the runs between "it scans on the bench" and "it can be left in a cabinet": powered by its cable only, sharing a device with a Studio box, and measured against the two goals docs/EDGE.md set for a node (G4: under 1 % late scans over 10 minutes; G5: the block's outputs go safe when the node's cable is pulled). Done already (2026-11-10, docs/EDGE.md Phase 3 "On the board"): the node image at 192.168.3.56 (press-5, token bench-token-2026), the chase at 10 ms with 51 to 72 us of work, a 431-line cell at 1 ms, the `enip` image against the E1213 both ways, the panic restart, PSRAM in `/api/node`, the late Modbus link at boot fixed. G4 and G5 were never run on any board; the ESP32 cards that held them are closed (Nick, 2026-11-13), so they are the P4's. Left: (1) a run on PoE alone, after `p4-poe-isolation`; (2) after `e1213-firmware-update`: Input Only from the P4 beside the Studio box's connection (`scratch/enip-fo-probe.py`); (3) G4 and G5 on the P4, 10 minutes against the E1213 and a pulled cable with the unit's watchdog and safe values set (the Devices tab's Configuration can set both now); (4) drop `ethernet::CutThroughTx` when an esp-hal release ties transmit store-and-forward to checksum offload. TLS is not on it any more: you want it for publishing data from a second Ethernet port, kept apart from control (card `node-data-tls`).
  needs: `p4-poe-isolation` for (1), `e1213-firmware-update` for (2); (3) needs nothing but the bench; (4) waits for an esp-hal release
  result: the P4 node runs on PoE alone, shares the E1213, and has its G4 and G5 numbers in docs/EDGE.md
  est: 1d
  difficulty: med
  notes: flash from a session: `NODE_BOARD=p4-eth NODE_P4_NO_POE=1 scripts/node-flash.sh node` (port /dev/cu.usbmodem5C930154551; espflash 4.7.0). A program over the flash slot runs but is not kept (12 KB did not fit): the larger slot comes with node-ota's partition table.
  depends: p4-poe-isolation, e1213-firmware-update
  id: node-p4-followups
  asked: 2026-11-10
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

- [ ] **The design step: a strong model designs the system, the everyday agent builds it** — Nick, 2026-11-15: "I need Opus 5.5 equivalent jumping in for hardcore system designs in one-shot for you to build from. Something so absurd that more abstraction will need to be added." Interview the same day: a design step the engineer asks for (not an escalation the cheap model decides, not the whole chat on the strong model); Opus 5.5 on the Anthropic key behind the relay's `best` tier; reachable from boxes with a token and, capped, from the playground. The session's own call: the playground runs it on the free chat's own capped key, never the Anthropic key, so the hard bound on public spend stays. Built: the relay profile `helmsman/design-v1` (agent-relay #39). It answers with no program: Machine, Signals, Parts (each with its ST interface and state table), Faults, numbered testable Requirements, a Build order whose steps name the requirements they must pass, Open questions with the value assumed. The dialect, the project, its I/O and its requirement tests go in the request as `<dialect>`, `<project>`, `<io_devices>`, `<requirement_tests>`. A big design takes several turns: after an answer that stopped at max_tokens the client sends the one word `Continue` with the assistant turn (its thinking block included) and the answer carries on from the exact character. Measured on a bottling line: 472 s for the first turn, 230 s for the second, about 78,000 output tokens, 11 parts, 81 requirements, 19 build steps. Since built: the page's half of the contract (`crates/helmsman-canvas/src/design.rs`, #465: the request, Continue and join, the reader, the brief for one build step, tested against the recorded bottling line and lamp), the box route (`POST /api/design`, #467) and the relay's tier grant (agent-relay #41: a token needs `--tier-daily-tokens N --tiers best` to be served a profile off the default tier, refused past that many tokens a day; `agent-relay-admin grant <client> ...` amends a live token in place). Left: the Agent tab entry (`design-step-canvas`), the playground (`design-step-playground`), the deploy (`design-step-deploy`).
  result: an engineer describes a whole machine, reads its design, and the Agent builds it step by step against the design's requirements
  est: 4d
  difficulty: high
  id: design-step
  asked: 2026-11-15
  by: design-step session

- [ ] **Design step on the canvas: ask for a design, read it, build from it** — done in #465: `crates/helmsman-canvas/src/design.rs` (`request`, `CONTINUE`, `join`, `parse`, `Design::problems`, `Design::step_brief`), registered with an expect on dead_code because nothing calls it yet. Left, once the agent-speed work on instruct.rs and claude.rs has landed: the Agent tab entry (on a box it posts to `POST /api/design` and posts `{"session"}` again while `done.more` is true; on the page it needs the playground's design entry, card `design-step-playground`), the thinking summary shown while a turn runs (a turn can take 8 minutes), the design kept with the project as `design.md` and shown readable at the panel's default width, `Design::problems` shown before Build is offered, and its Build order handed to the steps run (`steps.rs`) with `step_brief` as each step's instruction. Today a plan is at most 6 steps on one file (`MAX_STEPS`) and the bottling line's design has 19 steps over 11 parts: that limit, and one PROGRAM per project, are what the abstraction work has to lift; until then the step run takes the design in batches and says so. The page hides the design entry when the relay does not serve it.
  needs: the relay serving design-v1 (`design-step-deploy`) to try it live; fixtures of the bottling-line turns for tests (the session's scratchpad has them; hold them as test data)
  result: the Agent tab has a design entry; a design is a project file; Build runs its steps
  depends: design-step
  est: 2d
  difficulty: high
  id: design-step-canvas
  asked: 2026-11-15
  by: design-step session

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

- [ ] **Deploy the relay with a `best` tier** — the live relay (stack `agent-relay`, checked 2026-11-15 with `aws cloudformation describe-stacks`) has one model, `deepseek/deepseek-v4.1-flash`, and no `Tiers` parameter; its `main` is 29 commits behind `dev` (tiers, #24 on, are not on it). The Anthropic key is in Secrets Manager already (`agent-relay/anthropic-api-key`, 2026-11-04). Order: agent-relay #39 lands on dev; your merge of the relay's dev into main; then from the deploy checkout `SECOND_UPSTREAM_SECRET=agent-relay/anthropic-api-key RELAY_TIERS='{"upstreams":{"or":{"kind":"openrouter"},"ant":{"kind":"anthropic"}},"tiers":{"default":{"upstream":"or","model":"deepseek/deepseek-v4.1-flash-1017"},"best":{"upstream":"ant","model":"claude-opus-5-5"}}}' ./scripts/deploy.sh`. Then each box that should have the design step gets the grant on its live token, no new token: `RELAY_TABLE=agent-relay cargo run -q -p agent-relay-aws --bin agent-relay-admin -- grant <client> --tier-daily-tokens 2000000 --tiers best` (agent-relay #41; 2,000,000 tokens a day is about 25 large designs, at most about $40). Without the tiers the same build still starts and leaves design-v1 out (it says so in its log). First live turn: watch that the stream does not go silent while the model thinks (summarized thinking is there to prevent it; the trials ran through OpenRouter, never Anthropic's own API), and the DynamoDB path of `grant` has only been read, not run (no Docker for `make test-integration` that night). A design turn ran 472 s against the function's 600 s Timeout; the free chat's function allows 120 s, which no design turn fits, so its Timeout rises with the playground's design profile (a draft PR in the relay, deploy chain). Cost seen: about 78,000 output tokens for a large design, $20 a million on Opus 5.5.
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
  depends: agent-progress-box
  est: 3d
  difficulty: high
  id: agent-edits-everywhere
  asked: 2026-11-15
  by: claude-session

- [ ] **The box's Agent tab shows its progress too** — the page shows the time, the latest thought and the draft as it streams (feat/agent-progress); the box's Agent tab goes through `POST /api/chat` and still shows only a spinner (`instruct::View::progress` is `None` there). Needs the thinking and text deltas through the box's chat stream to the canvas, then the same `Progress`.
  needs: nothing
  result: an engineer on a box sees the Agent think and write, as in the playground
  est: 1d
  difficulty: med
  id: agent-progress-box
  asked: 2026-11-13
  by: claude-session

- [ ] **The Agent watches: on a box, and beside the failure** — Nick, 2026-11-14: "the agent should feel like a fundamental part of the environment. It should be watching every time it breaks to point out the stupid thing you did to show how it should be done better." Built for the simulation (`crates/helmsman-canvas/src/watch.rs`): a compile error, a fault, a program held by a lost input and a red requirement test are each said unasked in a card over the diagram with the better way and a Fix it button that hands the failure to the Agent. The words come from rules, so watching costs no model turn (decided 2026-11-14: only the click spends one). Left: the same on a box's page (`box_view`: its diagnostics, the modules' held and faulted states, a lost device); the line drawn beside the failure itself (the editor's line, the test's row) as well as over the diagram; more rules as real mistakes show which are missing; a value out of its limits.
  needs: nothing
  result: on a box too, a failure is followed by its explanation and its fix without the engineer asking
  est: 3d
  difficulty: med
  id: agent-watches
  asked: 2026-11-14
  by: claude-session

- [ ] **Name the sensors first: the device's side, the safe state, the plant** — Nick, 2026-11-14: "Those sensors? It should guide you to name them and their function. Their output? It should be deeply known ahead of time." Built for a new program in the simulation (`crates/helmsman-canvas/src/io_names.rs`): a fold in the Agent tab where each point gets a name, input or output, its type and what it is; the list goes to the Agent ahead of the instruction and the program is written in those names. Left: a device added from the library fills the list from its profile's channels (names, types, units, limits), so nothing about the device is typed (`device-profiles`); each output's safe state when its device is lost, asked once and carried into the map; the simulated plant (`plant.sim`) drafted from the same answers, so the faults thrown at the program are the named sensors' own; the same list for an existing program (its ports and their comments), and on a box.
  needs: nothing
  result: an engineer states what each sensor is once; nothing else about the I/O is asked
  est: 3d
  difficulty: med
  id: agent-names-io
  asked: 2026-11-14
  by: claude-session

- [ ] **The Agent sees your patterns: across sessions, and beyond compile errors** — Nick, 2026-11-14: "It should be seeing your patterns and fixing them. It should be putting the relevant cognitive load on you and helping you build quickly, not forcing you to take baby steps." Built in the simulation (`crates/helmsman-canvas/src/watch.rs`): the same kind of compile error in several places is said once with one Fix it for all of them, and a kind that comes up three times in a session is said to be a habit; the count lives in the page's memory only. Left: habits that last past a reload (what is kept about an engineer, where and for how long is yours to decide: this browser only, the project, or their account on a box); patterns beyond compile errors (the same edit repeated across a program, a requirement always written without its fault case); a habit turned into a standing fix the Agent applies by itself.
  needs: your decision on what may be kept about an engineer, and where
  result: a repeated mistake is pointed out once and fixed everywhere, and stays learned
  est: 4d
  difficulty: high
  id: agent-patterns
  asked: 2026-11-14
  by: claude-session

- [ ] **Device configuration follow-ups, from a recovered snapshot** — work that sat uncommitted in a worktree since 2026-11-12, now the branch `wip/recovered-devconf-canvas-20261114` (not built or tested, based on a tree from that day). In it: a failed re-read of a device's configuration keeps the last one on screen beside the error instead of replacing it; a new watchdog draft starts at 60 s and the sentence says seconds; a browser smoke of the Configuration section against the simulated E1200 console (`scripts/canvas-smoke-devconf.py`, `helmsman-devconf/examples/sim_console.rs`). Rebase it on dev, finish its tests, and land it, or delete the branch if the Configuration section has moved on.
  needs: nothing
  result: the follow-ups landed, or the branch gone
  est: 4h
  difficulty: low
  id: devconf-recovered
  asked: 2026-11-14
  by: claude-session

- [ ] **The browser smoke run shows a found device the library does not know** — Why it matters: the Devices tab's row for such a device, with its "Build from vendor file…" button, the notes beside it and the Add it offers after a profile is saved (#452, 2026-11-15), has only ever been drawn by the headless UI tests, which check geometry and text and not what a browser shows. `scripts/canvas-smoke.py` never scans a device the library lacks, so no screenshot holds that row. Give the box page's smoke run (`make canvas-smoke BOX=1`) a found device with no library profile (a scripted scan answer or a loopback device the smoke box can find, never a bench device), and take a screenshot of the row, of the note after a file is picked, and of the row once a profile is saved.
  result: the vendor-file path is looked at in a real browser on every smoke run, at the panel's default width
  est: 3h
  difficulty: low
  id: smoke-unknown-device
  asked: 2026-11-15
  by: claude-session (board run)

## The ST plan (INVESTIGATION.md, decided 2026-11-07 and 2026-11-08)

The investigation's "Suggested order" as Nick agreed it (`st-plan-order`), with his calls folded in: lints shown, two block, a pragma overrides (§1, 2026-11-07); the §4 dialect table with VAR_GLOBAL inside one module only; OSCAT or standard names for the library with plain words in the tooltips; no LSP or formatter until `version-control` ships; no SCL for the early release; IEC 61131-3 cited from secondary references marked as such (docs/SOURCES.md). Done already: the ruSTy 1.0.8 bump (#282) and `helmsman-plc test` with JUnit XML (#289). Each dialect "yes" adds oracle cases, property tests, the no_std build check and a new relay profile id (a served profile never changes).

- [ ] **PLCopen guideline lints in the checker, the first 6-8** — in `crates/helmsman-iec/src/check.rs` as `Diagnostic`s with stable IDs (`PLCOPEN-CP<n>` where the guideline has a number, `HELMSMAN-L<n>` otherwise), on the warnings path #346 built (the check answer's `warnings`, the canvas rows and the CLI lines; its parser already reads an `id` and a `blocking` flag), fed into the agent's retry message in `verify::next` beside compile errors. The picks (INVESTIGATION.md §1): an FB instance called inside a conditional (the TON-inside-IF bug), an output written in more than one place, a variable read before it is assigned with no initial value, `=` or `<>` on REAL, a FOR variable or its bounds changed in the body, unused variables and unreachable code, recursion. Decided (Nick, 2026-11-07): every lint is shown and goes to the agent; the first two block "Use this code" and Deploy the way a red requirement test does; a blocking lint is overridden on its line by a pragma with a reason (`{lint: off CP-x "imported, the branch is the enable"}`: the lexer skips `{ }` pragmas today, so it has to keep this one), and the override is written to the audit trail with the reason. The others stay advisory until `st-corpus` shows how noisy they are. Source: the PLCopen Coding Guidelines v1.0 PDF, a free download from plcopen.org (checked 2026-11-08, no login: `https://www.plcopen.org/download_file/force/ff60e817-eee5-442d-b718-a357a7279c7e/342/`); a docs/SOURCES.md entry with the rule number and page per lint ID. iec-checker in docker as a dev-time cross-check (both tools flag the same corpus files) is optional.
  result: the agent's first retry already knows about the timer-in-IF and the double-written output; an engineer sees why a draft is held, and can override with a reason on record
  est: 3d
  difficulty: med
  id: st-lints
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
  status: ready

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

- [ ] **The diagram draws a VAR_IN_OUT argument as a read** — found while building `st-var-in-out` (#446, 2026-11-15): a function block can write through its in-out argument, but helmsman-diagram's lowering draws that argument as an input wire only, so the write the block makes is missing from the picture and the diagram edits (`edit.rs`, built before in-outs existed) have never been tried on such a call. Draw the in-out as both ways, extend the generators in crates/helmsman-diagram/tests/props.rs to call blocks with in-outs and functions with outputs, and make each edit either work on those calls or refuse in plain words.
  result: a block that takes a buffer by reference shows that it changes it, and editing around it from the diagram is safe
  est: 1d
  difficulty: med
  id: diagram-in-out
  asked: 2026-11-15
  by: claude-session (board run)

- [ ] **A relay profile that knows VAR_IN_OUT, function outputs and VAR_GLOBAL** — the box and the playground accept them (#446 and this card's PR, 2026-11-15) but the served agent profiles still tell the model they are refused. A new profile id in ../agent-relay must say: VAR_IN_OUT on a FUNCTION_BLOCK only, given by name at every call, a plain variable, element or member of exactly the same type, never a located variable, a PROGRAM input, an expression or a constant, never the same variable twice in one call; VAR_OUTPUT on a FUNCTION, every one bound with `=>` at every call (a call that leaves one out is refused, as ruSTy does: Nick, 2026-11-15); VAR_GLOBAL blocks stand before the first POU (or in a CONFIGURATION and its one RESOURCE), every POU of the module reads and writes a global by name, VAR_EXTERNAL may name one with exactly its type and no initial value (VAR_EXTERNAL CONSTANT for a CONSTANT global, or to read only). Refused: a local with a global's name, a global passed as a VAR_IN_OUT, VAR_GLOBAL inside a POU, VAR_ACCESS and VAR_CONFIG (a CONFIGURATION's TASK lines and program instances are read past with a warning); a global is never a port and no other module sees it. A served profile never changes, so this is a new id and the box's and the playground's default moves to it.
  needs: a session in ../agent-relay for the profile, then your deploy
  result: the agent writes and repairs programs with the syntax the compiler now takes
  est: 3h
  difficulty: low
  id: relay-profile-dialect-2
  asked: 2026-11-15
  by: claude-session (board run)

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
  status: ready

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
  status: ready

- [ ] **Siemens SCL import** — decided (Nick, 2026-11-08: `st-scl-import`): not for the early release. SCL (TIA Portal) is an ST dialect (`#local`, `"DB".member`, REGION, IEC_TIMER instances, its own instructions); a source-level converter would follow the L5X pattern. Carded here so it is not lost: pick it up when a customer brings TIA code, and estimate then (likely 1-2 weeks).
  needs: a customer with TIA Portal code to import
  result: an SCL block converts to ST with every problem listed
  est: 2w
  difficulty: high
  status: blocked
  id: scl-import
  asked: 2026-11-08
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
  status: ready

- [ ] **Diagram edits 6: docs** — amend `canvas-edit-scope` in docs/ARCHITECTURE.md (the live ST view section: the diagram now writes ST through checked splices), docs/UI.md, and a help page in the floor-engineer voice.
  result: the docs say what the diagram can change and how it reaches the code
  est: 2h
  difficulty: low
  depends: diagram-edit-box
  id: diagram-edit-docs
  asked: 2026-11-07
  by: claude-session
  status: ready

- [ ] **Diagram edits V2 and V3: add blocks, then regions** — V2: a palette to add a block, a new assignment or a new FB instance (that edits the VAR section too). V3: regions: wrap statements in an IF, edit a condition, add ELSIF or ELSE, add CASE arms.
  result: building new logic from the diagram, not only changing it
  notes: V2 about 3-4 days, V3 about 3 days; split into cards when V1 lands.
  est: 1w
  difficulty: high
  depends: diagram-edit-docs
  id: diagram-edit-v2-v3
  asked: 2026-11-07
  by: claude-session
  status: ready

### Fewer, more descriptive blocks on the diagram (Nick, 2026-11-15: "do everything")

Built: n-input gates, the fold layer (`helmsman_diagram::fold`) with the canvas drawing it, the comparison with its literal, range, latch, edge, selector and set/reset (on as the diagram opens), and sections named by a comment and arms (fold on request). Each card below is one more rule or renderer. Gates,
chips and idioms are on as the diagram opens; arms, sections and output
cards fold on request and the choice is kept in `diagram.layout`.

- [ ] **A relay profile that writes section comments** — the diagram names a run of statements by the comment alone on its line above it (`fold::structure::sections`). A new profile id in ../agent-relay whose prompt asks for one short comment above each part of a program (a served profile never changes).
  result: programs the Agent writes open with their parts named on the diagram
  est: 2h
  difficulty: low
  id: relay-profile-sections
  asked: 2026-11-16
  by: claude-session
  status: ready


- [ ] **Zoom picks the level of detail** — far: every section and arm drawn as one block with its live state; middle: a statement as one line; near: the blocks. Sections and arms fold on request today (`Viewer::set_fold`); this folds them by zoom instead and opens nothing the reader folded by hand. The middle level is new: one fold per statement titled with its text.
  result: a large program is readable at any zoom
  est: 6h
  difficulty: med
  id: diagram-semantic-zoom
  asked: 2026-11-15
  by: claude-session
  status: ready

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
  status: ready

- [ ] **Fleet console + OTA** — the relay grows into the fleet backbone: box registry, snapshot sync, signed updates of the core.
  est: 3w
  difficulty: high
  notes: 2026-11-02 night: the node half moved: helmsman-node runs the scan bench and Modbus (PRs #193, #199); its own API + site beacon and the Site view across nodes are what this card is now.
  id: fleet-console
  needs: a decision taken (interview 2026-11-02): no console host at all; see notes
  notes: Nick 2026-11-02 (board): "I like this because it also allows me to set it up like my ../github/supervictor project. And now I realize that I may not have enough cores to do what I need with the network, uplink, communication, and program / process that may get complicated." Interview 2026-11-02: the fleet lives on the ESP32-POE2 as "every node is its own box": each node runs the runtime, serves its own API and announces itself on the LAN (`--site`), the fleet view is the canvas's Site view across nodes from any laptop, history and uplink go node → relay. Nothing in the middle to host. This card becomes: the Site view across boxes and nodes, the relay as the uplink, OTA of the node firmware (signed, A/B slots) once the node runs; depends on the edge spike (`node-modbus`) for the node half.
  status: ready

- [ ] **TLS on a node's data port** — your answer to `node-tls-first-peer` (2026-11-13): needed eventually to push production data out over a second network port, with strict isolation between the control runtime and the data publication; not until there is a board with more than one Ethernet port. The recipe (mbedtls-rs at the fork's main with Homebrew clang) compiles for the P4 (docs/EDGE.md Phase 3) and is all that exists.
  needs: a node board with two Ethernet ports
  result: a node publishes data over TLS on one port while its control traffic stays on the other
  est: 3d
  difficulty: high
  status: blocked
  id: node-data-tls
  asked: 2026-11-13
  by: claude-session

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
  notes: 2026-11-10: the controller side is built (`profinet-controller`), and with it a simulated device (`helmsman_profinet::device`: DCP, one AR, cyclic data, alarms) that is most of this card's protocol work. Still parked as a product: LLDP, the port and interface submodules, record reads and certification are not started.
