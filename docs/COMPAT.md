# The release train

Four repositories ship one product, and each is promoted from `dev` to its
default branch by hand: helmsman (the box, the canvas, the node firmware),
agent-relay (the relay the box's agent talks to), helmsman-devices (the
device library a box fetches) and helmsman-hardware (the node board). Each
one's CI only sees itself. `make compat` (`scripts/compat.sh`) checks the
contracts between them at the commits a promotion would ship, so all four
can be promoted together.

```sh
make compat                          # every contract on its three corners
make compat ARGS=--candidate         # only the four dev tips together
make compat ARGS=--live              # also the deployed relay (spends one short model turn per box ref)
scripts/compat.sh --relay origin/feat/x   # another candidate ref for one repository
```

The verdict is the last line; the report is `target/compat/report.md`, and
each cell's full output is in `target/compat/cells/`. Exit 0 means ready,
1 means a check failed (or a candidate commit's CI is red), and 2 means a
cell could not run.

## When it goes live

The repositories go live at different moments. A promotion plan has to
account for that, and the skew corners exist for it.

| repository | live when | live ref |
|---|---|---|
| helmsman | a box is rebuilt from `master` | `master` |
| agent-relay | `make deploy` from `main` | `main` (deployed separately) |
| helmsman-devices | the merge to `main`: boxes fetch `index.json` from raw `main` (`devlib::DEFAULT_URL`) | `main` |
| helmsman-hardware | a board is ordered | `master` |

## Corners

Each contract between A and B runs three times: A@dev with B@dev (the
candidate), A@dev with B@live, and A@live with B@dev. The second and third
are the two orders a staggered rollout can take. A failure in only one of
them tells you which repository has to go first.

## The contracts

Each one is black-box where the other side is a program, so a box built
from an old `master` is checked by the same code as one built at the tip.

- **helmsman ↔ helmsman-devices** (`crates/helmsman-compat/src/devices.rs`).
  A box at one ref is started with `--devices-index` pointing at a devices
  checkout served on loopback. It is asked for every index entry the way the
  Devices tab asks: by text, by EtherNet/IP identity when the entry has one,
  and by `wired=` for a record. Each entry must be listed, and its map
  lines, profile and record must read on that box. If the box cannot read
  the index at all, that is one failure.
- **helmsman ↔ agent-relay** (`relay.rs`). `agent-relay-local`, built at
  one ref, runs in an empty directory (no `.env`) in front of a stub model
  that streams the Messages API. Every tier (`default`, `best`, `cheap`) is
  on the stub and the dev token is granted each, so a profile is refused
  only for what the relay's own code decides. A box at another ref is
  pointed at it. What the model was sent is kept in the cell's work
  directory (`stub-requests.json`).
  - Every profile the box names by default in its `--help` must be served,
    including the explain and tests profiles, which get no turn here.
  - A chat, help and (where the box has it) design turn must end in `done`,
    with no `error` or `fallback`, after at least one model request.
  - On a turn's first request the stub calls every tool the profile offers
    with `{"compat_probe": 1}`, an input no tool takes. The box checks the
    name and the mode before the input, so it refuses the input and runs
    nothing. A tool it answers with `no tool named` or `not available in
    this mode` is one the relay offers and the box will not run there.
  - A design turn makes the box's own `list_devices`, which includes its
    read-only discovery sweep, as it does in production. From an SSH
    session macOS keeps that sweep off the LAN.
  - On the candidate corner the relay's newest `helmsman/vN` is also run as
    the chat profile (a harness profile on relay-held sessions).
- **live** (`--live`). The deployed relay's health, every default profile
  of the dev and the master box looked up there, and one chat turn through
  it per box. The relay's URL comes from the try box's `run-box.sh` and
  its token from `helmsman-try/scratch/relay-token` (`COMPAT_LIVE_URL`
  and `COMPAT_LIVE_TOKEN` override both). The deployed relay does not say
  which commit it runs, so this corner checks whatever is deployed, which
  is not necessarily `main`.
- **helmsman ↔ helmsman-hardware** (`hardware.rs`). node-p4-poe keeps the
  ESP32-P4 RMII and management pins that `helmsman-node-p4` drives on the
  Waveshare ESP32-P4-ETH (that repository's README). Each
  `field: peripherals.GPIOn` in the firmware must sit on its net in the
  exported netlist (`hardware/node-p4-poe/node-p4-poe.net`, which that
  repository's tests hold to its generator), and the reset line must reach
  the switch's reset. The firmware must never name GPIO54, which on this
  board would release the module's radio from reset.
- **CI**: each workflow's newest run on each candidate commit, from
  `gh run list --commit`. A merge that auto-merge made fires no push run
  on dev (it merges with `GITHUB_TOKEN`), so a merge commit with no run is
  judged by its PR head (the second parent), and the report says so. Any
  other commit with no run counts as unverified, not as green.

## What it does not cover

- The explain and tests routes get a profile lookup only, with no turn
  (explain needs a traced module, tests a project with requirements).
- A devices entry's map is fetched by the box, but its lines are not
  parsed as an I/O map here. Only the map of the box's chosen protocol
  (`enip-io`, else the first) is fetched.
- Real devices and the node board itself. Those are the benches
  (`scripts/bench-e1213.sh`, helmsman-hardware `docs/bringup-test.md`).
- What the site and the try box serve. That is `make live-state`.
- Whether each feature works end to end on live as it does on dev. That
  is the vertical check, `make parity` and `make ship` (docs/PARITY.md),
  which runs this one first.

## Running it

Everything is local: clones (`git clone --shared`, read-only to your
checkouts) and builds go to `target/compat/`, and the sibling repositories
are found beside the main checkout (`COMPAT_REPOS` overrides). A cold run
builds two boxes and two relays. Boxes and relays listen on loopback only
and are stopped when their cell ends.

## In CI

`.github/workflows/compat.yml` runs `make compat` (the full matrix, not
`--live`) every night at 03:17 UTC and on "Run workflow". It covers what no
single repository's CI does: each repository's checks see only that
repository, and a merge that auto-merge made fires no push run on dev, so
nothing else re-tests dev after one. The job summary shows the verdict line
and the report; `target/compat/report.md` and `cells/` are uploaded as the
`compat` artifact for 30 days. Exit 1 or 2 fails the job.

- **A schedule fires only from the default branch**, so the nightly starts
  once dev is promoted; before that, run it by hand (`gh workflow run
  compat.yml --ref <branch>`).
- **The relay token.** agent-relay and helmsman-hardware are private, so
  the job needs the repository secret `COMPAT_RELAY_TOKEN`: a fine-grained
  token for those two repositories with Contents, Metadata and Actions read-only, 90-day expiry
  (`scratch/33-compat-token.sh` prints the settings, checks the token and
  stores it). Without the secret the job writes one line to its summary
  and passes without running. An expired token fails the clone.
- **The token stays out of files and logs.** `scripts/compat.sh` takes it
  from `COMPAT_RELAY_TOKEN` and unsets it at once. It is sent only as an
  `http.extraheader` in the environment of the two private repositories'
  own `git` and `gh` commands, never in a URL, an argument list or a clone's `.git/config`, and
  no build sees it. `scripts/compat-scrub.sh` then fails the job if the token
  or its header form is in the report, the cells, the run log, the build
  logs or a clone's git config; the upload runs only after it passes.
- **Cloning.** With `COMPAT_CLONE=1` (the workflow sets it), compat.sh
  clones the missing siblings from github.com into `COMPAT_REPOS`;
  `compat.sh --sync` does just that. Locally nothing changes.
- **Cache.** Cargo's registry and the four builds (`target/compat/build`)
  are cached under a key hashed from the Cargo.lock of every ref a build
  comes from, so a build is redone only when a dependency changed
  somewhere; a miss starts from the newest older cache. Only scheduled and
  dev runs save it.
