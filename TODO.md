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

- [ ] **Keep the ruSTy oracle as a CI job?** `make test-oracle` runs the ST interpreter against ruSTy (LGPL-3.0) in a pinned, hash-checked Docker image; ruSTy is executed, never linked or shipped, so the licence never touches what we distribute. The draft PR adds a non-required `oracle` job (~1 min, downloads ~32 MB of .deb per run).
  asked: 2026-10-28
  by: claude-session
  needs: your call
  result: oracle job stays (and can become required) or is dropped to local-only
  options: keep, not required | keep, required | local only
  est: 5m
  difficulty: low
  id: oracle-in-ci

- [ ] **Report ruSTy's unsigned-compare bug upstream?** ruSTy 1.0.7 compares UDINT/ULINT as signed (`ULINT_max > 1` is FALSE; minimal repro in tests/oracle.rs `rusty_unsigned_compare_bug`). Filing an issue on PLC-lang/rusty is a public post under your account.
  asked: 2026-10-28
  by: claude-session
  needs: your call
  result: an issue on PLC-lang/rusty with the repro, or nothing
  options: file it | leave it
  est: 10m
  difficulty: low
  id: rusty-unsigned-bug

## Tasks

- [ ] **Bench sweep against the CompactLogix and FP0H** — `cargo run --example discover <subnet broadcast>`; fill the rows in `docs/INTEROP.md`, keep the first capture.
  needs: devices powered and on a Studio-reachable subnet (answer on 2026-10-26: not hooked up yet)
  result: first real ListIdentity from each device
  est: 30m
  difficulty: low
  status: blocked
  id: bench-sweep

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

- [ ] **Flash the Pi 5 for CODESYS** — reflashes the card that was the perceptronics pick PC with Raspberry Pi OS Lite 64-bit, SSH on. Run in the native terminal (sudo + dd), not through `!`.
  needs: the Pi's SD card in a reader on the Mac; `diskutil list` to find /dev/diskN
  result: a headless Pi with user nick and a printed first-boot password
  notes: scripts/pi-flash.sh /dev/diskN
  est: 20m
  difficulty: low
  id: pi-flash

- [x] **Cable the Pi to the office LAN** — USB-C on a Pi 5 is power only; the 2026-10-27 sweep found no Pi on either subnet, over mDNS, or on USB.
  needs: an Ethernet cable from the Pi to the 10.0.0.0/24 switch
  result: Pi answers at raspberrypi.local
  est: 5m
  difficulty: low
  depends: pi-flash
  id: pi-cable

- [ ] **Prep the Pi** — key-only SSH, hostname codesys-pi, packages current, checks 44818/2222 free. Runs here.
  needs: the first-boot password pi-flash printed
  result: Pi ready for Deploy Control SL
  notes: ! scripts/pi-prep.sh nick@raspberrypi.local
  est: 10m
  difficulty: low
  depends: pi-cable
  id: pi-prep

- [ ] **Install CODESYS Control for Raspberry Pi 64 SL and an EtherNet/IP Adapter project** — Windows-only IDE, so this is yours. Steps 4–5 in docs/interop/codesys-pi.md; record the assembly numbers and sizes it assigns.
  needs: a Windows machine with the CODESYS Development System
  result: Pi runs an EtherNet/IP adapter (2-hour demo without a licence)
  est: 1h
  difficulty: med
  depends: pi-prep
  id: codesys-runtime

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

- [ ] **Differential test oracle against ruSTy** — run the same ST programs through PLC-lang/rusty (LGPL-3.0, LLVM, dev/CI only, never linked) and our interpreter, compare variable state after N scans. Generated programs via proptest plus a hand-written conformance corpus.
  needs: LLVM in CI (ruSTy builds against it); a decision that a dev-only LGPL tool in CI is fine
  result: `make test-iec-oracle`, run in CI; disagreements become regression tests
  est: 3d
  difficulty: med
  depends: iec-st-runtime
  id: iec-oracle
  notes: harness done 2026-10-28 (tests/oracle.rs, make test-oracle, CI job `oracle` not required); 6000 random programs + corpus agree; open question oracle-in-ci decides whether the CI job stays

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

- [ ] **LD/FBD via PLCopen XML import** — accept TC6 XML from existing free editors (ladder and FBD drawn there), lower to the same IR as ST. No graphical editor of our own.
  needs: iec-stdlib; one editor chosen to test exports from
  result: a ladder program drawn in a free editor runs on our runtime
  est: 1w
  difficulty: high
  depends: iec-stdlib
  id: plcopen-xml

