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

- [ ] **Cable the Pi to the office LAN** — USB-C on a Pi 5 is power only; the 2026-10-27 sweep found no Pi on either subnet, over mDNS, or on USB.
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
