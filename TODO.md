# TODO

## Open questions for Nick

- [x] **Language: Python or Rust?** The role is a scanner/client (originator), not an adapter. Python gets you pycomm3 as prior art and fast iteration on the config-tool side; Rust gets you a static binary for a Pi/Jetson/STM32-class box and a parser that survives hostile devices. One language per project per your rules, so this decides the whole repo layout.
  asked: 2026-10-26
  by: claude-session
  needs: your call
  result: repo scaffold (uv workspace or cargo workspace), CI gate, first codec module
  options: Python (uv) | Rust (cargo)
  est: 30m
  difficulty: low
  id: language
  answer: Rust (cargo)

- [x] **First deliverable shape?** Library + CLI (`tcpethernet discover`, `tcpethernet get 1.1.7`, `tcpethernet io --rpi 10ms`), or a daemon on the open device that exposes the customer's adapters over something simpler (REST/MQTT)? The sources only cover the wire protocol; this is the product question.
  asked: 2026-10-26
  by: claude-session
  needs: who the first user is and what they plug into
  result: scope for milestone 1
  options: library+CLI | daemon with REST/MQTT | both, library first
  est: 1h
  difficulty: med
  depends: language
  id: deliverable-shape
  answer: both, library first

- [x] **Get ODVA's okay in writing.** You said ODVA confirmed they're fine with this. An email from a named ODVA contact, saved under `docs/odva-correspondence/`, is what protects the project if that person moves on. Also confirm with them that "CIP" (also their trademark, PUB00213R0 p.2) can appear in prose, not just "EtherNet/IP".
  asked: 2026-10-26
  by: claude-session
  needs: you to send one email
  result: `docs/odva-correspondence/<date>.eml` committed
  est: 15m
  difficulty: low
  id: odva-written-ok
  answer: I put the email in the folder for you.

- [x] **Which real devices are the first interop targets?** OpENer-in-Docker is the CI target, but the project is pointless until it talks to a customer's actual adapter. Name one or two (vendor, model, which stack it runs) so the first milestone has a real Forward_Open to pass.
  asked: 2026-10-26
  by: claude-session
  needs: device list, ideally one on the LAN
  result: interop matrix in `docs/`
  est: 15m
  difficulty: low
  id: interop-targets
  answer: I have a CompactLogix and a Panasonic FP0H or something I think.

- [x] **Create the GitHub remote now?** The repo has no `origin`. The repo-baseline wiring (CI gate, Dependabot, auto-merge, `dev` protection) needs one, and the Cargo manifest already points at `github.com/JimothyJohn/tcpethernet`. Public from day one, or private until the ODVA email is filed?
  asked: 2026-10-26
  by: claude-session
  needs: your call on visibility and owner (JimothyJohn vs an org)
  result: `gh repo create`, push master + dev, repo-baseline draft PR into dev
  options: public now | private until ODVA email | wait
  est: 30m
  difficulty: low
  depends: odva-written-ok
  id: github-remote
  answer: I just pushed the first master manually

- [x] **CompactLogix and FP0H: model, firmware, IP, and which subnet?** A `ListIdentity` sweep from the Studio on 2026-10-26 found nothing on 192.168.3.0/24 or 10.0.0.0/24, so neither is reachable here yet. For the FP0H, only AFP0HC32ET/EP have EtherNet/IP; check the label.
  asked: 2026-10-26
  by: claude-session
  needs: devices powered and on a Studio-reachable subnet, or a Pi on their subnet to run the example from
  result: `docs/INTEROP.md` rows filled in; first real ListIdentity capture
  est: 30m
  difficulty: low
  id: bench-devices
  answer: I haven't hooked them up yet.

- [x] **The ODVA email is already in public history.** `docs/odva-correspondence/emails.txt` went out in commit dbf8e88 on public `master` before your `docs/*` ignore. This branch narrows the ignore to `docs/odva-correspondence/` and untracks the file going forward, but the bytes stay in history until `master` is rewritten (`git filter-repo` or a fresh initial commit) and force-pushed, which is your push, not mine. Alternatively redact the contact's address and keep it tracked.
  asked: 2026-10-26
  by: claude-session
  needs: your call: rewrite master history | redact and re-add | leave it
  result: public repo contains only what you intend
  options: rewrite master | redact and re-add | leave it
  est: 15m
  difficulty: low
  id: odva-email-public
  answer: redact and re-add

- [x] **Keep the ruSTy oracle as a CI job?** `make test-oracle` runs the ST interpreter against ruSTy (LGPL-3.0) in a pinned, hash-checked Docker image; ruSTy is executed, never linked or shipped, so the licence never touches what we distribute. The draft PR adds a non-required `oracle` job (~1 min, downloads ~32 MB of .deb per run).
  asked: 2026-10-28
  by: claude-session
  needs: your call
  result: oracle job stays (and can become required) or is dropped to local-only
  options: keep, not required | keep, required | local only
  est: 5m
  difficulty: low
  id: oracle-in-ci
  answer: keep (stays not required)

- [x] **Report ruSTy's unsigned-compare bug upstream?** ruSTy 1.0.7 compares UDINT/ULINT as signed (`ULINT_max > 1` is FALSE; minimal repro in tests/oracle.rs `rusty_unsigned_compare_bug`). Filing an issue on PLC-lang/rusty is a public post under your account.
  asked: 2026-10-28
  by: claude-session
  needs: your call
  result: an issue on PLC-lang/rusty with the repro, or nothing
  options: file it | leave it
  est: 10m
  difficulty: low
  id: rusty-unsigned-bug
  answer: report it, as a draft issue
  notes: not filed: already fixed on ruSTy master by PLC-lang/rusty#2015 (merged 2026-10-21, after v1.0.7); repro re-verified against 1.0.7 on 2026-10-28. Follow-up card rusty-bump.

- [x] **Theme depth: tokens, layout, or custom widgets?** "Want a new theme? Talk to it." You skipped this in the 2026-10-29 interview. Tokens only (colors, fonts, density) is safe and always consistent. Layout adds per-role panel arrangement. Custom widgets (a gauge, a machine mimic) makes visuals a module tier with its own sandbox story.
  asked: 2026-10-29
  by: claude-session
  needs: your call
  result: scope of theme-engine (M3)
  options: tokens only | tokens + layout | custom widgets too
  est: 5m
  difficulty: low
  id: theme-depth
  answer: Don't allow for customizations like this I changed my mind

- [x] **Marketplace floor under the AI review.** You picked "AI-reviewed on install". I'd keep that, but not alone: a hostile bundle can carry text aimed at the reviewer (prompt injection), and AI review is non-deterministic. Proposal: signing, a capability manifest, and WASM/ST sandboxing enforced by the host are the mechanical floor; Claude's plain-language review sits on top and can only refuse, never grant.
  asked: 2026-10-29
  by: claude-session
  needs: your call
  result: trust model for bundle-format and marketplace
  options: floor + AI review (recommended) | AI review only
  est: 5m
  difficulty: low
  id: market-floor
  answer: floor + AI review (recommended)

- [x] **Can the operator edit in the canvas, or only watch?** Per your 2026-10-29 note, ST is the source of truth (agent-written, engineer-tweakable) and the canvas is the operator's live view of what the code does. Edit-in-canvas means round-tripping a graph edit back to ST, which is hard to do without mangling the agent's source. View-only plus "ask the agent" keeps one direction.
  asked: 2026-10-29
  by: claude-session
  needs: your call
  result: whether st-graph-view needs a graph→ST writer, and what the uncommitted tcpethernet-graph editor becomes
  options: view-only, change via agent (recommended) | small edits (params, thresholds) in canvas | full graph editing
  est: 10m
  difficulty: med
  id: canvas-edit-scope
  answer: view-only, change via agent (recommended)

- [x] **Local fallback model: on the Pi, or nothing until a hub exists?** You chose "runs + local fallback" for offline. A Pi 5 can run a ~1–3B model via llama.cpp, slowly, good enough to change a threshold or explain a node, not to write modules. Costs RAM next to the runtime.
  asked: 2026-10-29
  by: claude-session
  needs: your call
  result: scope of local-fallback (M2)
  options: small model on the Pi | site hub only | rule-based param edits only (no model)
  est: 10m
  difficulty: med
  id: fallback-model
  answer: rule-based param edits only (no model)

- [x] **Relay hosting shape.** The relay holds the Anthropic key, meters per box, and versions prompts/tools. My default: API Gateway + Lambda + DynamoDB (box tokens, usage), streaming via Lambda response streaming. Your AWS account, a new stack.
  asked: 2026-10-29
  by: claude-session
  needs: your call; which AWS account/region
  result: relay-v0 can start
  options: Lambda + DynamoDB (recommended) | small EC2/Fargate service
  est: 10m
  difficulty: low
  id: relay-hosting
  answer: Lambda + DynamoDB (recommended)

- [x] **Does the EtherNet/IP library keep the `tcpethernet` name?** You picked Helmsman Factory Automation on 2026-10-29 and asked for the crates and project to be renamed, but the roadmap's naming card said "tcpethernet stays the protocol layer". My pick: keep `tcpethernet` (and the adapter) as the protocol library, since it's an accurate, searchable name and a protocol library shouldn't carry a product brand; everything product-shaped becomes helmsman-*. The fresh agent on helmsman-rename needs this before it starts.
  asked: 2026-10-29
  by: claude-session
  needs: your call
  result: the name map helmsman-rename applies
  options: keep tcpethernet for the protocol crates (recommended) | rename everything to helmsman-*
  est: 5m
  difficulty: low
  id: protocol-crate-name
  answer: rename everything to helmsman-*

- [x] **Multi-module: module-relative addresses, private %M?** Built tonight (feat/multi-module) on two calls I made and you may want to reverse. (1) A module's `%I`/`%Q` addresses are relative to its window: `%QX0.0` is bit 0 of *its* outputs, wherever the window sits, so the checker bounds it at compile time and a module can be moved without editing its ST. The cost: the same address means different bytes in different modules, which matters for debugging against the raw assembly. (2) `%M` is private per module; modules can't share memory, and talking between modules waits for signals (io-runtime / signal-model). Both are cheap to change now and expensive after module-format and hot-swap build on them.
  asked: 2026-10-30
  by: claude-session
  needs: your call
  result: confirms or changes the addressing the module-format manifest and hot-swap inherit
  options: keep relative + private %M (recommended) | absolute addresses, checked against the window | allow a shared %M window
  est: 5m
  difficulty: low
  id: module-addressing
  answer: keep relative + private %M (recommended)

- [x] **A device drops: hold the modules that read it?** Built tonight (feat/io-runtime): a module never computes on missing data. If any signal it reads has no value (its device is down, or the value doesn't fit the port, e.g. 40000 into an INT), it is *held*: not scanned, its %Q window zeroed, its output signals sent to their safe values; it resumes when every input is back. The alternative most PLCs offer is holding the last good value with a quality bit the logic can test, which keeps unrelated logic in the same module running but lets a stale "tag X is high" keep a light on. Held is the conservative call for a box whose outputs need a human's approval to move.
  asked: 2026-10-30
  by: claude-session
  needs: your call
  result: the failure semantics module-format, sim-replay and the proposal UI explain to the engineer
  options: hold the module, outputs safe (recommended) | last good value plus a quality input the ST can read | zero the missing inputs and keep scanning
  est: 5m
  difficulty: low
  id: missing-input-policy
  answer: hold the module, outputs safe (recommended)

- [x] **I/O map file: keep the line format?** `--io-map FILE` reads a small line format (`device rio modbus-tcp 192.168.3.50:502 unit=1`, `out lamp bool box do:0 safe=true`; parser in tcpethernet-io `text.rs`, round-trip proptested). I didn't pull serde into the runtime for it: the repo parses its own formats and the module-format manifest and box API (JSON over HTTP) will decide the long-term shape. Switching later is one parser; nothing deployed reads it yet.
  asked: 2026-10-30
  by: claude-session
  needs: your call
  result: whether module-format / box-api replace it or adopt it
  options: keep the line format until box-api, then JSON from the API (recommended) | JSON now via serde (already in the lockfile for tcpethernet-graph) | TOML (new dep)
  est: 5m
  difficulty: low
  id: iomap-format
  answer: keep the line format until box-api, then JSON from the API (recommended)

- [ ] **Shift calendar time zones: fixed offsets, or read the OS tz database?** The shift calendar (feat/shift-calendar) takes `zone +01:00` plus each DST change written out as `zone +02:00 from 2026-03-29T01:00Z`; `zone Europe/Berlin` is refused with a message saying so. No new dependency, and evaluation stays a pure function of the instant, but someone has to add next year's change dates by hand and a forgotten one shifts every shift by an hour. Two ways to accept zone names: hand-parse the TZif files already on the Pi (`/usr/share/zoneinfo`, a few hundred lines, no crate, but the box then depends on the OS tzdata being current), or add `jiff` (well-maintained, bundles or reads tzdata, a sizeable new dependency). I kept fixed offsets for now (reversible: a zone name would only add a parser branch).
  asked: 2026-10-30
  by: claude-agent
  needs: your call
  result: whether `zone Europe/Berlin` works in a calendar file, and what the box depends on for it
  options: keep fixed offsets with explicit changes (recommended for now) | hand-parse the OS TZif files, no new crate | add jiff
  est: 1d
  difficulty: med
  id: calendar-timezones

- [ ] **Hot swap of a faulted module: carry its state, or start it cold?** Built in feat/hot-swap: `Runtime::swap` replaces a module's program between scans, carrying every retained variable whose name and type match. A Faulted (or Held) module may be swapped and restarts Running with the fault cleared, and I chose to carry its state like any other swap: a parts counter survives the fix, as tags do after a Logix online edit and fault clear. The cost: the variables are as the fault left them, so a bad index that caused the fault can fault the new program again on its first scan (outputs are already zero, so that is safe, just noisy). `%M` keeps its bytes over the shared length, as an address means the same byte in both programs. Switching to cold start for faulted modules is one branch in `swap`.
  asked: 2026-10-30
  by: claude-agent
  needs: your call
  result: what an applied fix does to a stopped module's counters and timers; box-api and the proposal UI explain it
  options: carry state, as for a running module (recommended) | start cold (initial values) after a fault | let the caller choose per swap
  est: 5m
  difficulty: low
  id: hot-swap-faulted-state

- [ ] **Logix point syntax: require `@TYPE`?** The `logix-tag` driver (feat/logix-tag-read) binds a signal to `TAG@TYPE`, e.g. `in temp real plc Program:Main.Temp@real`, `in fault bool plc Status.5@bool`. The map validator needs a point's type without touching the device, and the declared Logix type is checked against every reply, so a tag that turns out to be a DINT is an error rather than a REAL read as garbage. The alternative is a bare tag path whose type is learned on connect (browse), which reads nicer but makes `IoMap::validate` unable to catch a bool/int/real mismatch until the box talks to the PLC. Built with `@TYPE` required; switching later is a change in `helmsman-io/src/logix.rs` and the map files that use it.
  asked: 2026-10-30
  by: claude-agent
  needs: your call
  result: the address syntax map files and the signal-model card use for Logix tags
  options: keep TAG@TYPE required (recommended) | bare TAG, type learned on connect | both, @TYPE optional
  est: 5m
  difficulty: low
  id: logix-point-syntax

- [ ] **Download 1756-PM020 by hand?** The Logix 5000 Controllers Data Access manual (the reference for Read Tag, fragmented reads, Multiple Service Packet and the Symbol object) returns HTTP 403 to scripted fetches, like the Modbus PDFs did. The tag-read code cites pycomm3 and libplctag for its byte layouts instead (`docs/SOURCES.md`). If you save the PDF anywhere on the Studio, the next session checks every layout and error code against it and adds `PM020 p.<n>` citations.
  asked: 2026-10-30
  by: claude-agent
  needs: the PDF from the vendor's literature library (publication 1756-PM020), dropped under `~/Downloads` or `docs/`
  result: logix layouts checked against the primary source, citations by page
  options: here it is (path) | skip, the prior art is enough until the bench test
  est: 5m
  difficulty: low
  id: logix-pm020

- [ ] **Commit GPL editor examples as fixtures?** The real PLCopen exports that prove the importer (OpenPLC Editor's Blink ladder and Traffic_Light_FBD, Beremiz's test projects) are GPL-2.0/GPL-3.0 project files with no header of their own; this repo is MIT. I did not copy them in: `scripts/plcopen-corpus.sh` fetches 87 of them pinned by commit and SHA-256 into target/ and runs `tests/plcopen_corpus.rs` (`make test-plcopen-corpus`), so CI without network never sees them. Committing two small ones (Blink, Traffic_Light_FBD) would make them run on every `cargo test`, at the cost of GPL files in the tree.
  asked: 2026-10-30
  by: claude-agent
  needs: your call on licensing
  result: real exports either in crates/helmsman-iec/tests/plcopen/ or fetched only
  options: fetch only, run via make test-plcopen-corpus (recommended, current) | commit Blink + Traffic_Light_FBD with a GPL notice in that directory | draw our own ladder in OpenPLC Editor and commit that (MIT)
  est: 10m
  difficulty: low
  id: plcopen-fixture-license

## Tasks

- [x] **Explicit messaging: RegisterSession, SendRRData, Get_Attribute_Single on Identity** — the next protocol step in the library. Reference: OpENer `encap.c` (register/unregister session, SendRRData CPF with null address + unconnected data item) and `cipmessagerouter.c`.
  needs: nothing
  result: `tcpethernet::explicit` module with a loopback test server, same no-mock pattern as discovery
  est: 1d
  difficulty: med
  id: explicit-messaging

- [x] **OpENer POSIX sample in Docker as the CI integration target** — build from EIPStackGroup/OpENer master, run in `docker compose`, point the discovery and explicit tests at it.
  needs: Docker on the CI runner (ubuntu-latest has it)
  result: `docker-compose.yml` + an integration test gated on `TCPETHERNET_OPENER_ADDR`
  est: 4h
  difficulty: med
  depends: explicit-messaging
  id: opener-docker

- [x] **`tcpethernet-cli` crate** — `discover`, `get <class>.<instance>.<attribute>`, later `io`. Answer to deliverable-shape was "both, library first", so this lands after explicit messaging works.
  needs: explicit-messaging
  result: second workspace member, `cargo install`-able binary
  est: 4h
  difficulty: low
  depends: explicit-messaging
  id: cli-crate

- [ ] **Daemon on the open device (REST/MQTT front for the customer's adapters)** — the second half of "both". Shape undecided; revisit once the CLI exists.
  needs: cli-crate, a decision on REST vs MQTT
  result: design note in `docs/`
  est: 2h
  difficulty: med
  depends: cli-crate
  id: daemon

- [x] **Flash the Pi 5 for CODESYS** — reflashes the card that was the perceptronics pick PC with Raspberry Pi OS Lite 64-bit, SSH on. Run in the native terminal (sudo + dd), not through `!`.
  needs: the Pi's SD card in a reader on the Mac; `diskutil list` to find /dev/diskN
  result: a headless Pi with user nick and a printed first-boot password
  est: 20m
  difficulty: low
  notes: dropped 2026-10-29: no reflash (was scripts/pi-flash.sh /dev/diskN); pi-prep ran in place on the pick PC
  id: pi-flash

- [x] **Cable the Pi to the office LAN** — USB-C on a Pi 5 is power only; the 2026-10-27 sweep found no Pi on either subnet, over mDNS, or on USB.
  needs: an Ethernet cable from the Pi to the 10.0.0.0/24 switch
  result: Pi answers at raspberrypi.local
  est: 5m
  difficulty: low
  depends: pi-flash
  id: pi-cable

- [x] **Prep the Pi** — key-only SSH, hostname codesys-pi, packages current, checks 44818/2222 free. Runs here.
  needs: the first-boot password pi-flash printed
  result: Pi ready for Deploy Control SL
  notes: done 2026-10-29 in place on the unflashed pick PC (`scripts/pi-prep.sh nick@pickpc.local pickpc`): hostname kept as pickpc, perceptronics-cockpit left running, eth0 192.168.3.20, wlan0 10.0.0.56, password SSH off, 44818/2222 free. pickpc.local mDNS hung afterwards; use the IP
  est: 10m
  difficulty: low
  depends: pi-cable
  id: pi-prep

- [x] **Install CODESYS Control for Raspberry Pi 64 SL and an EtherNet/IP Adapter project** — Windows-only IDE, so this is yours. Steps 4–5 in docs/interop/codesys-pi.md; record the assembly numbers and sizes it assigns.
  needs: a Windows machine with the CODESYS Development System; Deploy Control SL target 192.168.3.20, user nick (key login, passwordless sudo)
  result: Pi runs an EtherNet/IP adapter (2-hour demo without a licence)
  est: 1h
  difficulty: med
  depends: pi-prep
  notes: dropped 2026-10-29: Nick skips CODESYS. OpENer (make opener-up) is the CI adapter, tcpethernet-plc is our own soft PLC + adapter (MIT) and is what customers run
  id: codesys-runtime

- [x] **Deploy tcpethernet-plc (IEC runtime) to the pick PC** — the Pi target now that CODESYS is out. The old toy-PLC deploy/smoke scripts left with #11; write scripts/pi-deploy-plc.sh for the IEC runtime (cross-build aarch64, systemd unit, open 44818/tcp+udp and 2222/udp in perceptronics' firewall) and re-smoke from the Studio.
  needs: nothing; Pi prepped (192.168.3.20, key SSH, passwordless sudo)
  result: a .st program on the Pi drives Class 1 I/O the Studio's scanner sees
  notes: `! scripts/pi-deploy-plc.sh nick@192.168.3.20` then `scripts/adapter-smoke.sh 192.168.3.20` (both run here); smoke PASSED 2026-10-29 on the pick PC: echo.st as a systemd service, 32 bytes echoed over Class 1 at 10 ms RPI; firewall rules are runtime-only, a redeploy restores them after a reboot
  est: 2h
  difficulty: med
  depends: pi-prep
  id: pi-deploy-iec

- [x] **Deploy tcpethernet-plc to the Pi and smoke it from the Studio** — our own adapter + soft PLC as a systemd service (web page on :8080, token printed by the script). Alternative or complement to CODESYS; needs no Windows. Runs here.
  needs: pi-prep done (key SSH, passwordless sudo)
  result: Pi answers discover/identity and echoes 32 bytes over Class 1; then scripts/plc-smoke.sh <pi-ip>
  notes: smoke PASSED 2026-10-27 on the unflashed pick PC (hostname pickpc, eth0 192.168.3.20 direct to the Studio, wlan0 10.0.0.56), then removed the same night with the toy PLC (service, unit, state, firewall rules); the IEC runtime replaces it
  est: 20m
  difficulty: low
  depends: pi-prep
  id: pi-deploy-plc

- [x] **IEC 61131-3 runtime, Structured Text first** — our own MIT runtime in this workspace: ST lexer/parser, type checker (elementary types, arrays, structs, enums), interpreter, POUs (PROGRAM, FUNCTION, FUNCTION_BLOCK with instance state), cyclic task with a fixed scan, located variables (%I/%Q/%M) mapped onto the adapter's assemblies. Programs are .st files deployed from the CLI; no web editor.
  needs: nothing to start; the IEC text is paywalled, so behaviour is pinned by the differential oracle (iec-oracle), not by quoting the standard
  result: crates/tcpethernet-iec (library) + a runtime binary; a .st program on the Pi drives Class 1 I/O a scanner sees
  est: 2w
  difficulty: high
  id: iec-st-runtime

- [x] **Standard function blocks and functions** — TON/TOF/TP, CTU/CTD/CTUD, R_TRIG/F_TRIG, SR/RS, numeric/bit-string/selection/comparison functions, type conversions. Each one property-tested against the oracle.
  needs: iec-st-runtime parsing and evaluating FB calls
  result: a library that real ST programs written for other runtimes compile against
  est: 1w
  difficulty: med
  depends: iec-st-runtime
  id: iec-stdlib

- [x] **Differential test oracle against ruSTy** — run the same ST programs through PLC-lang/rusty (LGPL-3.0, LLVM, dev/CI only, never linked) and our interpreter, compare variable state after N scans. Generated programs via proptest plus a hand-written conformance corpus.
  needs: LLVM in CI (ruSTy builds against it); a decision that a dev-only LGPL tool in CI is fine
  result: `make test-iec-oracle`, run in CI; disagreements become regression tests
  est: 3d
  difficulty: med
  depends: iec-st-runtime
  id: iec-oracle
  notes: harness done 2026-10-28 (tests/oracle.rs, make test-oracle, CI job `oracle` not required); 6000 random programs + corpus agree; oracle-in-ci answered: keep, not required

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

- [ ] **PROFINET IO device (RT class 1)** — own Rust implementation over raw Ethernet (DCP, LLDP, RPC connect, cyclic RT frames); IRT is out (needs hardware). Lab use only: shipping a real device needs a PI vendor ID and certification.
  needs: a PROFINET controller to test against (a Siemens PLC, or a software controller) and access to the PROFINET spec (PI membership) or a decision to work from public captures + p-net's behaviour
  result: a PROFINET controller sees the Pi as an IO device and exchanges cyclic data with the ST program
  est: 3w
  difficulty: high
  depends: iec-st-runtime
  id: profinet-device

- [x] **LD/FBD via PLCopen XML import** — accept TC6 XML from existing free editors (ladder and FBD drawn there), lower to the same IR as ST. No graphical editor of our own.
  needs: iec-stdlib; one editor chosen to test exports from
  result: a ladder program drawn in a free editor runs on our runtime
  est: 1w
  difficulty: high
  depends: iec-stdlib
  id: plcopen-xml
  notes: slice 1 on feat/plcopen-xml (2026-10-28): tcpethernet_iec::compile_plcopen, `tcpethernet-plc PROJECT.xml [--emit-st]`; LD/FBD/ST bodies, data types, located vars; lowered to ST text (not the AST), diagnostics mapped to XML line:col. Format pinned to the TC6 v2.01 XSD and Beremiz's own test project (both GPL, read only, nothing copied; Beremiz's LD/FBD counters lower to what its generator emits). Not yet: SFC, IL, jumps/labels, VAR_IN_OUT block params. Ticks once plcopen-editor-check passes.

- [x] **Draw a ladder in a real editor and run its export** — OpenPLC Editor (Beremiz-based, free) or Beremiz: a rung or two with contacts, a coil, a TON; located I/O as `%IX0.n` / `%QX0.n` in the variable table. Save, then `cargo run -q -p tcpethernet-plc -- path/to/plc.xml --emit-st` to see the lowering, and drop `--emit-st` to run it behind the adapter. Paste any error here; a project that works becomes a fixture.
  needs: none: done without a GUI against 87 plc.xml files the editors saved themselves (Beremiz and OpenPLC Editor examples/tests, pinned by commit + SHA-256), `make test-plcopen-corpus`. OpenPLC's Blink ladder (contact, coil, TON, TOF) and Traffic_Light_FBD (located %QX0.0..3 with an initial value, F_TRIG, TOF) compile and are checked scan by scan; every other file is refused with a named reason (SFC/IL/STRING/DT/VAR_EXTERNAL/extension types/multi-level addresses). Fixtures stay fetched, not committed: see plcopen-fixture-license.
  result: a real export in crates/tcpethernet-iec/tests/plcopen/ and plcopen-xml ticked
  est: 30m
  difficulty: low
  depends: plcopen-xml
  id: plcopen-editor-check
  notes: Why can't you do this?

- [x] **Consolidate the canvas work into dev** — on 2026-10-29 the node canvas existed in two places, both outside dev: (1) the main checkout /Users/nick/github/tcpethernet on `feat/node-canvas` (4591603), about 20 files staged, never committed: crates/tcpethernet-canvas (app, canvas, example, look, main, ui_tests, index.html), crates/tcpethernet-graph (edit, model, sim + proptests), scripts/canvas-serve.sh, scripts/canvas-web.sh, Cargo.toml/Cargo.lock/Makefile edits; (2) worktree /Users/nick/github/tcpethernet-canvas on `feat/canvas` (98f4afb), draft PR #19, three commits (7ca5b1e canvas + sim, 5d8eae8 todo card, merge of dev). Steps: confirm both sessions are stopped; commit (1) to `wip/recovered-node-canvas-20261029`; diff it against `feat/canvas` and keep the newer/fuller one, cherry-picking anything only the other has; land it via PR #19 into dev; remove the spent worktree and branches. Per canvas-edit-scope the canvas becomes the read-only ST view (st-graph-view), so the editing paths (edit.rs, canvas edit UI) can stay as-is for now but aren't the direction.
  needs: the two agents in /Users/nick/github/tcpethernet and /Users/nick/github/tcpethernet-canvas wound down
  result: one canvas branch in dev; main checkout back on dev; no stray worktrees
  notes: also stale: /Users/nick/github/tcpethernet/.claude/handoff.md (says main is on dev, pi-deploy-iec pending; both untrue). Replace it with /Users/nick/github/tcpethernet-dev/.claude/handoff.md, then remove the tcpethernet-dev worktree (its branches are all merged). Worktree /Users/nick/github/tcpethernet-io (`feat/io-model`) was clean at dev on 2026-10-29; check it before removing.
  est: 1h
  difficulty: med
  id: consolidate-canvas

- [x] **Decide the leftover README branch** — `wip/recovered-readme-odva-20261028` holds a README edit reverted off the main checkout on 2026-10-28 and never merged. Read it (`git log -p dev..wip/recovered-readme-odva-20261028`), then PR it into dev or delete the branch.
  result: no unmerged wip branches
  est: 10m
  difficulty: low
  id: readme-wip-branch

- [x] **Rename the project to Helmsman Factory Automation** — move crates, binaries, repo, docs and UI off the tcpethernet name. Suggested map (final shape per protocol-crate-name): tcpethernet-plc → helmsman-plc, tcpethernet-iec → helmsman-iec, tcpethernet-cli → helmsman-cli (bin `tcpethernet` → `helmsman`), tcpethernet-canvas/-graph → helmsman-canvas/-graph, tcpethernet-adapter follows the protocol crate. Touches: root Cargo.toml members and the `repository` URL, package names and every `use` path, deploy/tcpethernet-plc.service and scripts/pi-deploy-plc.sh (the new deploy must stop and remove the old unit on the pick PC), the other scripts/ and Makefile targets, CI job names, README and docs, web page titles, env vars TCPETHERNET_OPENER_ADDR / _OPENER_IO / _RUSTY / _RUSTY_CASES / _RUSTY_SEED. The GitHub repo is already JimothyJohn/helmsman (old URL redirects, checked 2026-10-29): point the Cargo `repository` URL and README links at it; local checkouts still have the old origin URL (`git remote set-url origin https://github.com/JimothyJohn/helmsman.git`).
  needs: protocol-crate-name answered; PR #19 (feat/canvas, adds tcpethernet-canvas and tcpethernet-graph) merged or closed, and no crate edits in flight in sibling worktrees (feat/io-model); this is a mass edit that conflicts with everything open
  result: one PR into dev, `./scripts/lint.sh` and `cargo test --workspace --locked` green under the new names, interop job green
  est: 1d
  difficulty: med
  depends: protocol-crate-name, naming
  id: helmsman-rename
  notes: done 2026-10-30. Map: tcpethernet → helmsman-proto (kept CIP, an ODVA mark, out of crate names), -adapter → helmsman-adapter, -cli → helmsman-cli (bin `helmsman`), -plc/-iec/-io/-graph/-canvas → helmsman-*. Env vars HELMSMAN_*. The next pi-deploy-plc.sh run retires the old tcpethernet-plc unit and its firewall rules.
  notes: brand canvas (private) https://claude.ai/artifact/WB1uKtP1NRHHfGoWw95tBt: wordmark Saira Stencil One, descriptor Barlow Condensed 700; palette yellow #FFC20E, black #141414, graphite #3A3A3A, steel #8C8C8C (rules only), white; never yellow on white; mark is the stencil encoder gear. crates.io `helmsman` is taken by an unrelated crate: run `cargo search <name>` on each new name before publishing. Restarting or removing the service on the pick PC asks Nick each time.

- [x] **GitHub Pages brand gallery for Helmsman** — Pages already serves docs/ from master (legacy build, https://jimothyjohn.github.io/helmsman/, landing page docs/index.html still titled tcpethernet). Rebrand that landing page to Helmsman Factory Automation (stencil wordmark, yellow/black/graphite/white, stencil encoder mark), and add docs/brand/index.html showing every concept from the brand canvas: the marks (round 1 and the round-2 gear set), the wordmarks, all color themes with hex and contrast rules, the in-the-field mockups and the spray-painted stencil-encoder surfaces. Static HTML/CSS and inline SVG only, no build step, no JS framework (the repo ships no JS); fonts from Google Fonts. Also commit the final mark as SVG files under docs/brand/ (full stencil encoder, a heavier small-size variant for 16–20 px, black, yellow and white) so the rename and the UI can reuse them.
  needs: the brand canvas (private, so the agent needs Nick to share it or the HTML exported into the repo first; the SVG geometry is in its StencilEncoder board); helmsman-rename landed, so the page and the code use the same names
  result: docs/brand/ with the gallery page and mark SVGs; docs/index.html rebranded; live at /helmsman/brand/ after Nick merges dev → master
  est: 4h
  difficulty: low
  depends: helmsman-rename
  id: brand-pages
  notes: GitHub Pages builds from master/docs, so nothing goes live until Nick's own dev → master merge; don't change the Pages source or add a deploy workflow without asking. Helmsmand brand guide is here bt that should be in another corktree already: https://claude.ai/artifact/WB1uKtP1NRHHfGoWw95tBt#page-dcb81aee8d83
  notes: done 2026-10-30. docs/brand/index.html reproduces all eight canvas boards with computed WCAG ratios; mark SVGs are helmsman-mark-{black,yellow,white}.svg (full, 32 px and up) plus helmsman-mark-20-* and helmsman-mark-16-* (the board's two heavier small-size cuts). The pages are hand-maintained static HTML, no generator.


## Product vision (interview 2026-10-29)

Working title until naming lands. A small, opinionated controller for factory edge boxes (Pi 5 reference, headless, browser UI), for non-mission-critical work: low-frequency detection, shift-aware scheduling of its own outputs, on-floor signalling (stack light, andon, screen). Data is pushed elsewhere (webhooks, OPC UA); the box keeps no historian.

Agentic first. The agent (Claude, behind our relay) is the primary programmer. Every module is Structured Text the agent writes and an engineer can tweak; the existing parser, checker, interpreter and ruSTy oracle are its feedback loop. The canvas is the operator's window into the code: ST rendered as a live diagram with values on wires and the reason each output is on. The canvas is the app shell; chat lives in its side panel.

Extension tiers: data (params, signals, schedules) → ST modules → WASM (drivers, heavy code; M3). Users are layered: operator (view, ask), engineer (approve, edit ST, PIN), integrator (modules, policy). The approval policy depends on tier and role. No user theming: the look is fixed. Param changes go live with undo; logic changes get a preview; anything that can move an output needs an engineer PIN plus a sim replay.

Hard limits, enforced by the host and never by the prompt: no writes to external PLCs (CIP is read-only toward them); no output change without human approval; modules have no network access (core sinks publish); the agent never touches the Rust core. Offline: everything installed keeps running, chat degrades to rule-based param edits (no local model). Business: open core (runtime + canvas MIT; relay, fleet and verified marketplace paid) plus a preloaded box. Standalone first, fleet-shaped data model from day one.

Update 2026-10-29 (I/O mapping): "This device is the PLC, it just may talk to other slave/client devices. This will be a master/server." The box masters its devices and writes to them through the I/O map; read-only stays the rule for other controllers it only observes (a CompactLogix it reads tags from). Agent-written drivers run in the WASM tier with one extra capability: a socket to their own device's endpoint.

## Roadmap M1: bench demo, talk → module → light

Demo: on the bench, "flash the light when CompactLogix tag X is high for 5 s". The agent drafts ST, the canvas shows it live against real values, a PIN approves it, the Pi's output toggles, and undo restores the previous snapshot.

- [x] **Architecture note** — docs/ARCHITECTURE.md: the vision above as boxes and arrows. Box core (runtime, signals, sinks, snapshots, policy), box API, canvas, agent loop, relay. Which crate owns what; the trust boundaries.
  result: one page every later card points at
  est: 3h
  difficulty: med
  id: arch-note

- [x] **Multiple ST modules in one runtime** — tcpethernet-plc runs one program today. Run N modules as separate tasks, each with its own scan rate, %I/%Q/%M slice and fault isolation (one faulting module zeroes only its own outputs).
  needs: nothing
  result: a box runs several independent agent-written modules
  est: 3d
  difficulty: med
  id: multi-module

- [x] **Hot-swap a module without dropping I/O** — replace one module's compiled program between scans, carrying over retained state where names and types match; the adapter connection stays up.
  result: applying a change is instant and doesn't fault the scanner
  est: 2d
  difficulty: med
  depends: multi-module
  id: hot-swap

- [x] **Module format** — a module is a directory/bundle: `module.st` plus a manifest (signals read, outputs driven, schedule, params with types and ranges, author, prompt that produced it). The manifest is what policy and the sandbox enforce.
  result: the unit the agent writes, snapshots store, and bundles ship
  est: 1d
  difficulty: med
  depends: arch-note
  id: module-format

- [x] **Signal model** — named, typed signals with units and poll rates, bound to a source (CIP tag, CIP attribute, GPIO in, another module's output). Modules read signals by name, never raw addresses.
  result: one vocabulary for the agent, the canvas and the manifest
  est: 2d
  difficulty: med
  depends: module-format
  id: signal-model

- [x] **Logix symbolic tag read** — Read Tag (0x4C) and multi-service packets over the existing explicit session; tag list browse (Symbol object 0x6B) so the agent can see what's on the PLC. Read-only by construction: no Write Tag in the box build.
  needs: a fixture server for tests (OpENer can't serve symbolic tags); real-device verification waits for an EtherNet/IP target device on the bench
  notes: built on feat/logix-tag-read: helmsman-proto `logix` (TagReader: Read Tag, MSP, Read Tag Fragmented, Symbol browse, backplane routing), the `unrestricted` feature gate, the fixture in helmsman-adapter `logix`, the `logix-tag` driver in helmsman-io. Verified only against our fixture; real-CompactLogix verification still waits for hardware on the bench (see `logix-pm020`, `logix-point-syntax`).
  result: CompactLogix tags become signals
  est: 4d
  difficulty: high
  id: logix-tag-read

- [ ] **Box outputs: GPIO stack light** — Pi GPIO (or a relay hat) as box-owned outputs with named channels; blink patterns as a stdlib FB.
  needs: a relay hat or 24 V stack light + driver on the bench
  result: the demo's light
  est: 1d
  difficulty: low
  id: gpio-outputs

- [ ] **Box API** — one local HTTP API on the box: signals (list, live values), modules (list, source, manifest, status), proposals, snapshots, outputs, audit. The canvas, the agent tools and later MCP all go through it. Supersedes the `daemon` card's shape.
  result: single surface, testable without a UI
  est: 4d
  difficulty: med
  depends: module-format, signal-model
  id: box-api

- [x] **Snapshots and rollback** — each applied change set is an immutable, content-addressed snapshot in SQLite (modules, params, signal bindings) with author, prompt, approver, time. One-call rollback. Exportable as a bundle later.
  result: undo for everything the agent does
  notes: built: helmsman_plc::store (rusqlite bundled + sha2), `helmsman-plc --store DIR` runs head; see ARCHITECTURE "Snapshots"
  est: 2d
  difficulty: med
  depends: module-format
  id: snapshots

- [x] **Runtime trace tap** — the interpreter exposes per-scan variable values and which statements/branches executed, sampled at UI rates, without slowing the scan.
  result: the data the live canvas draws
  est: 3d
  difficulty: high
  depends: multi-module
  id: trace-tap

- [ ] **ST → diagram view** — lower a module's ST AST into an FBD-like diagram (blocks for FB calls and expressions, wires for data flow, IF/CASE as gated regions) and lay it out. Live values from trace-tap on wires; outputs show "on because …". Reuses tcpethernet-graph's model and the egui canvas as the renderer.
  notes: view-only (canvas-edit-scope): no graph→ST writer; every change goes through the agent as ST. The tcpethernet-graph editor on feat/node-canvas becomes a read-only renderer
  result: the operator sees what the code is doing
  est: 1w
  difficulty: high
  depends: trace-tap
  id: st-graph-view

- [ ] **Serve the canvas from the box** — wasm build of tcpethernet-canvas served by the box API, talking to it over HTTP/WebSocket; any browser or tablet on the LAN is the HMI.
  result: headless Pi, browser UI
  est: 2d
  difficulty: med
  depends: box-api
  id: canvas-web

- [ ] **Relay v0** — the boxes' only path to Claude: per-box token, the Anthropic key server-side, usage metering, versioned system prompt and tool schema. Dev runs against a local relay with a .env key.
  needs: the AWS account/region for the stack
  notes: API Gateway + Lambda (response streaming) + DynamoDB for box tokens and usage (relay-hosting)
  result: no API keys on boxes; prompts upgrade without reflashing
  est: 3d
  difficulty: med
  depends: relay-hosting
  id: relay-v0

- [ ] **Agent loop** — the tool-using agent behind the chat panel. Tools: list/read signals and live values, read modules, draft module, check (our ST checker), simulate (sim-replay), propose. It iterates on checker and sim errors before a human sees anything. The prompt treats PLC tag names, comments and bundle text as data, never instructions.
  result: "talk to it" works end to end
  est: 1w
  difficulty: high
  depends: box-api, relay-v0
  id: agent-loop

- [ ] **Sim replay** — record a rolling window of signal values on the box (RAM ring buffer, not a historian) and replay a proposed module against it, showing when its outputs would have fired.
  result: the evidence an engineer approves output changes on
  est: 3d
  difficulty: med
  depends: signal-model, multi-module
  id: sim-replay

- [ ] **Approval policy + PIN roles + audit** — proposals carry a tier (param / logic / output-affecting); policy decides live vs preview vs PIN + sim. Operator/engineer/integrator PINs; every apply/rollback is logged with role and time (accounts later).
  result: the safety story is mechanical, not a prompt
  est: 3d
  difficulty: med
  depends: snapshots, box-api
  id: approval-policy

- [ ] **Proposal review UI** — in the canvas: ST diff, diagram diff (ghost blocks), sim replay chart, manifest changes (new outputs/signals highlighted), approve with PIN.
  est: 4d
  difficulty: med
  depends: st-graph-view, approval-policy, sim-replay
  id: proposal-ui

- [ ] **M1 demo run** — the sentence at the top of this section, on the bench, recorded. scripts/m1-demo.sh brings the box up from a clean deploy.
  needs: CompactLogix on the bench
  result: the thing to show people
  est: 1d
  difficulty: med
  depends: logix-tag-read, gpio-outputs, agent-loop, proposal-ui, hot-swap, canvas-web
  id: m1-demo

- [x] **I/O map, driver contract, Modbus TCP, conformance harness** — crates/tcpethernet-io: devices the box masters, named signals bound to device points or box channels (do:N / di:N), validation, the string-addressed driver contract, a Modbus TCP driver from the spec, a simulator with cross-wires and faults, and the PASS/FAIL harness (`cargo run -p tcpethernet-io --example conformance -- --sim`).
  result: the vocabulary for signal-model, and the loop an agent's driver is judged by
  notes: branch feat/io-model; the first slice of signal-model (no units or poll rates yet)
  est: 1d
  difficulty: med
  id: io-map

- [x] **Signals in the scan** — tcpethernet-plc reads its I/O map: a poll task per device fills input signals, programs read and write signals by name instead of %I/%Q, outputs are written on change, and a device that drops sends its outputs to a declared safe state.
  result: an ST program drives a Modbus coil and a box output by name
  est: 3d
  difficulty: high
  depends: io-map, multi-module
  id: io-runtime

- [ ] **Split point errors from link errors in the I/O links** — today any failed write marks the whole device link down (crates/tcpethernet-plc/src/link.rs), so a module commanding a value the point can't hold (70000 into a u16 `hr:N`) or a device exception on one point holds every module that reads that device. Give the driver `Error` a way to tell an encode/point failure from a transport failure (Modbus `Format::encode` reuses `Error::Protocol` today), keep the link up on point errors, retry that output each cycle, and surface the point error where the box API can show it.
  result: one bad output point no longer takes its whole device offline
  est: 3h
  difficulty: med
  depends: io-runtime
  id: io-point-errors

- [ ] **Tabletop I/O bench** — one Modbus TCP remote I/O block with 24 V DI/DO on the Studio link (192.168.3.x), outputs DO0-3 wired back to inputs DI0-3, a 24 V supply, and a Pi GPIO to 24 V output stage for do:N. Then `conformance --endpoint <ip>:502 --loop coil:0=di:0 ...` against it.
  needs: hardware bought and wired (any Modbus TCP 8DI/8DO block; note its manual's register map and whether it numbers from 0 or 1)
  result: the harness passes on real hardware, and agents have a bench that proves writes physically
  est: 4h
  difficulty: low
  depends: io-map
  id: io-bench

- [ ] **Drivers in the WASM tier** — the driver contract across the sandbox: parse/read/write by address string, one capability (a socket to its own device's endpoint), fuel and memory limits. The Modbus driver ported as the first guest, passing the same harness.
  result: drivers the agent writes can't touch anything but their device
  est: 1w
  difficulty: high
  depends: wasm-tier, io-map
  id: wasm-driver

- [ ] **Agent writes a driver from the manual** — give the agent a device's documentation (PDF to text, as done for the Modbus spec); it writes the point map and, for a new protocol, a WASM driver, and iterates on harness output: simulator first, then the bench. Engineer approves the map before any output is bound.
  result: a new device or protocol without hand-written driver code
  est: 1w
  difficulty: high
  depends: wasm-driver, io-bench, agent-loop
  id: driver-agent

## Roadmap M2: floor-ready core

- [ ] **Stock module: counters and uptime** — cycle counts, run/idle/down state, shift totals; the first shipped module, written in ST like everything else, so it doubles as the agent's few-shot example.
  est: 2d
  difficulty: low
  depends: m1-demo
  id: stock-counters

- [x] **Shift calendar and scheduled outputs** — shifts, breaks, planned downtime as box data; modules can ask "in shift?" and the scheduler drives outputs on a calendar (with the output policy applying).
  est: 3d
  difficulty: med
  depends: multi-module
  id: shift-calendar

- [ ] **Floor screen output** — a box-driven full-screen andon view (big tiles, colors, a message) as an output kind, next to GPIO lights.
  est: 2d
  difficulty: low
  depends: canvas-web
  id: andon-screen

- [ ] **Webhook sink** — core-owned publisher: modules emit to named outputs, sinks POST JSON with retry and a bounded on-disk queue. Modules never see the network.
  est: 2d
  difficulty: low
  depends: signal-model
  id: sink-webhook

- [ ] **OPC UA server** — expose signals and module outputs as an OPC UA address space for SCADA/MES to pull. Dependency review first (pure-Rust stack vs bindings).
  est: 1w
  difficulty: high
  depends: signal-model
  id: sink-opcua

- [ ] **More sources: Modbus TCP/RTU, GPIO/I2C inputs** — signal sources beyond CIP, so a dumb machine plus a sensor is a valid install.
  est: 1w
  difficulty: med
  depends: signal-model
  id: sources-modbus-gpio

- [ ] **Explain mode** — tap any block or output in the canvas; the agent explains in floor language why it's in this state right now, using trace-tap data.
  est: 2d
  difficulty: med
  depends: st-graph-view, agent-loop
  id: explain-mode

- [ ] **Offline behaviour and local fallback** — relay unreachable: banner; chat accepts only structured param edits ("set hopper threshold to 80") parsed by rules against the manifest's typed ranges. No local model. Installed modules unaffected.
  est: 3d
  difficulty: med
  depends: agent-loop, fallback-model
  id: local-fallback

- [ ] **Drop-in: prose / SOP → module** — paste or upload a paragraph or doc; the agent extracts signals, conditions, outputs and drafts a module plus questions for what's missing.
  est: 2d
  difficulty: med
  depends: agent-loop
  id: dropin-prose

- [ ] **Drop-in: existing PLC program** — L5X export (and the PLCopen/ST we already parse) → the agent maps tags to signals and proposes watch modules. Read-only: never round-trips into the customer PLC.
  est: 1w
  difficulty: high
  depends: agent-loop, logix-tag-read
  id: dropin-plc

- [ ] **Drop-in: whiteboard photo** — vision via the relay turns a sketch of sensors/arrows/lights into a draft module and signal list, then asks.
  est: 3d
  difficulty: med
  depends: agent-loop
  id: dropin-photo

- [ ] **Drop-in: P&ID / electrical PDF** — extract I/O lists and tag names from drawings into a draft signal map.
  est: 1w
  difficulty: high
  depends: dropin-photo
  id: dropin-pid

## Roadmap M3: tiers, sharing, fleet

- [ ] **WASM module tier** — wasmtime host; capabilities from the manifest (read signals, emit outputs, nothing else); fuel/memory limits; for drivers and code ST is bad at. Built off-box by the relay.
  est: 2w
  difficulty: high
  depends: module-format
  id: wasm-tier

- [ ] **Bundle format and signing** — portable signed bundles (modules, params, signal map templates) importable on another box; signature + manifest checked by the host.
  notes: floor = signature + capability manifest + host-enforced sandbox; the AI review on install sits on top and can only refuse (market-floor)
  est: 1w
  difficulty: med
  depends: snapshots, module-format
  id: bundle-format

- [x] **Theme engine** — scope per theme-depth; themes are data the agent writes, applied live with undo.
  notes: dropped 2026-10-29: no user theming (theme-depth)
  est: 3d
  difficulty: med
  depends: theme-depth, canvas-web
  id: theme-engine

- [ ] **MCP server on the box** — the box API as MCP tools so an engineer's own Claude / Claude Code can work on it, under the same policy and audit.
  est: 2d
  difficulty: med
  depends: box-api, approval-policy
  id: box-mcp

- [ ] **Local accounts** — per-person logins replacing PINs; audit carries the person.
  est: 3d
  difficulty: med
  depends: approval-policy
  id: local-accounts

- [ ] **Multi-box site view** — boxes discover each other on the LAN; one canvas shows the floor.
  est: 1w
  difficulty: high
  depends: canvas-web
  id: site-view

- [ ] **Fleet console + OTA** — the relay grows into the fleet backbone: box registry, snapshot sync, signed updates of the core.
  est: 3w
  difficulty: high
  depends: relay-v0, bundle-format
  id: fleet-console

- [ ] **Vision sources** — deferred per interview; card kept so the signal model leaves room for camera-derived signals.
  est: 2w
  difficulty: high
  id: vision

## Roadmap M4: marketplace and the box

- [ ] **Community marketplace** — publish/browse/install bundles; floor per market-floor, then an AI review on install that explains risk in plain words and can only refuse.
  est: 3w
  difficulty: high
  depends: bundle-format, market-floor, fleet-console
  id: marketplace

- [ ] **Box SKU** — reproducible Pi image, first-boot pairing with the relay, provisioning script, enclosure/BOM for the preloaded kit.
  est: 2w
  difficulty: high
  depends: fleet-console
  id: box-sku

- [ ] **Open-core split** — decide which crates stay MIT and where relay/fleet/verified-market code lives (private repo), before any paid code lands here.
  est: 2h
  difficulty: low
  depends: relay-v0
  id: open-core-split

- [x] **Product name** — the app gets its own name; tcpethernet stays the protocol layer.
  est: 1h
  difficulty: low
  id: naming
  notes: decided 2026-10-29: Helmsman Factory Automation. Rename work is helmsman-rename; whether the protocol crate keeps tcpethernet is protocol-crate-name.

- [ ] **Try the node canvas** — ComfyUI/Node-RED style editor in Rust (egui, wasm): nested function blocks, live scan sim, not wired to the PLC. Kept as a lab tool (2026-10-29); per canvas-edit-scope the operator canvas is the read-only ST view (st-graph-view), which reuses tcpethernet-graph and this renderer. Serve it: `make canvas-web`, then `python3 -m http.server 8790 --bind 0.0.0.0 --directory target/canvas-dist` and open http://10.0.0.41:8790/ from the Air (stop: `kill $(lsof -tiTCP:8790 -sTCP:LISTEN)`). In it: hold a start button, double-click a motor to go inside, select nodes + Cmd+G to make a block, `?` lists the controls.
  needs: your eyes on the UI
  result: a list of changes, or none
  notes: `make canvas-serve` (trunk, live reload) only answers on localhost on the Studio: macOS's firewall holds the unsigned trunk binary's incoming connections behind a GUI prompt; python3 is allowed
  est: 15m
  difficulty: low
  id: canvas-try
