# TODO

## Open questions for Nick

- [ ] **Language: Python or Rust?** The role is a scanner/client (originator), not an adapter. Python gets you pycomm3 as prior art and fast iteration on the config-tool side; Rust gets you a static binary for a Pi/Jetson/STM32-class box and a parser that survives hostile devices. One language per project per your rules, so this decides the whole repo layout.
  asked: 2026-10-26
  by: claude-session
  needs: your call
  result: repo scaffold (uv workspace or cargo workspace), CI gate, first codec module
  options: Python (uv) | Rust (cargo)
  est: 30m
  difficulty: low
  id: language

- [ ] **First deliverable shape?** Library + CLI (`tcpethernet discover`, `tcpethernet get 1.1.7`, `tcpethernet io --rpi 10ms`), or a daemon on the open device that exposes the customer's adapters over something simpler (REST/MQTT)? The sources only cover the wire protocol; this is the product question.
  asked: 2026-10-26
  by: claude-session
  needs: who the first user is and what they plug into
  result: scope for milestone 1
  options: library+CLI | daemon with REST/MQTT | both, library first
  est: 1h
  difficulty: med
  depends: language
  id: deliverable-shape

- [ ] **Get ODVA's okay in writing.** You said ODVA confirmed they're fine with this. An email from a named ODVA contact, saved under `docs/odva-correspondence/`, is what protects the project if that person moves on. Also confirm with them that "CIP" (also their trademark, PUB00213R0 p.2) can appear in prose, not just "EtherNet/IP".
  asked: 2026-10-26
  by: claude-session
  needs: you to send one email
  result: `docs/odva-correspondence/<date>.eml` committed
  est: 15m
  difficulty: low
  id: odva-written-ok

- [ ] **Which real devices are the first interop targets?** OpENer-in-Docker is the CI target, but the project is pointless until it talks to a customer's actual adapter. Name one or two (vendor, model, which stack it runs) so the first milestone has a real Forward_Open to pass.
  asked: 2026-10-26
  by: claude-session
  needs: device list, ideally one on the LAN
  result: interop matrix in `docs/`
  est: 15m
  difficulty: low
  id: interop-targets
