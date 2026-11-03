# Licensing: which part is under which terms

Helmsman is open core. This page names each part, its licence and the
reason, so nothing paid lands in an open repository by accident and nothing
open gets fenced off by one. Decided with Nick on 2026-11-02 (TODO.md
`open-core-split`).

| Part | Where | Licence | Why |
| --- | --- | --- | --- |
| The controller: runtime, Structured Text compiler and interpreter, drivers, the CIP and Modbus stacks, the box API, the canvas, the simulation, the starters and the docs | this repository, every crate under `crates/` | **Apache License 2.0** (`LICENSE`, `Cargo.toml`'s workspace `license`, chosen 2026-11-01) | A controller is used in products and plants; Apache 2.0 gives every user an explicit patent grant and lets a vendor ship it in a product, with the notices kept. |
| The site beacon and the Site view (boxes finding each other on a LAN, the fleet seen from any browser) | `crates/helmsman-plc/src/site.rs`, the canvas's Site view | **Apache License 2.0**, as the rest of this repository | Nick's split puts the fleet under Apache; it lives here, under the same licence, so no separate file is needed. A fleet of its own (OTA, signed firmware for the nodes) gets its own crate or repository when it exists, under Apache 2.0 too. |
| The relay: per-box tokens, the model key, metering, agent profiles | the private repository `agent-relay` (local at `../agent-relay` until published) | **MIT** (to be added there; not this repository's change) | A small HTTP contract that other clients may copy into their own stacks; the simplest licence does the most good there. Being MIT does not make it public: the repository stays private because it holds the metering and the key handling. |
| Usage metering and billing | the relay | MIT, same repository | Part of the relay. |
| The marketplace's server side: publisher accounts, review, payments | a private repository, not yet made | **Proprietary** | The paid side of open core. What a box needs to *use* a marketplace (the signed bundle format, the index format, the install path) is in this repository under Apache 2.0, so a third party can run its own index. |
| The device library: device records, map lines, docs with citations | the public repository [`helmsman-devices`](https://github.com/JimothyJohn/helmsman-devices) | **MIT** for the records; vendors' files keep their own terms and are referenced by hash, not copied | Data meant to be copied into anyone's project. |

## In practice

- A change to this repository is submitted under Apache 2.0 with a
  `Signed-off-by:` line (`CONTRIBUTING.md`, "Licensing and sign-off").
- Code that meters, bills or holds a model key does not land here. If a
  card needs it, it goes to the relay.
- A bundle, a starter or a device record is licensed by its author; the
  formats this repository defines say where the licence line goes
  (`docs/ARCHITECTURE.md`, bundles), and the marketplace index carries it.
- `NOTICE` holds the attribution lines Apache 2.0 section 4(d) asks a
  product built on Helmsman to keep.
