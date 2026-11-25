# Feature parity, `make staging` and `make ship`

`make compat` (docs/COMPAT.md) is the horizontal check: the four
repositories held to each other's contracts. This is the vertical one: every
feature a user reaches, walked end to end on the candidate and on live, so
a promotion is one command and its report says, feature by feature, whether
live does what the candidate does.

The line is `dev` → `staging` → the default branch, in all four
repositories. `make staging` takes the dev tips to `staging`; `make ship`
takes the staging tips to production. "The candidate" is the tip being
promoted: dev's for `make staging`, staging's for `make ship`. `make ship`
takes nothing from dev: it names what dev holds that staging lacks and
leaves it there.

```sh
make parity                                   # every journey on this checkout's box (no relay: agent journeys not run)
make parity ARGS="--relay-bin target/compat/build/agent-relay-dev/debug/agent-relay-local"   # agent journeys on the stub (the relay make compat builds at agent-relay dev)
make staging ARGS=--dry-run                   # check the dev tips: contracts, CI, journeys, tabs; nothing promoted
make staging                                  # rebuild a stale playground, check the dev tips, promote them to staging
make ship ARGS=--dry-run                      # check the staging tips the same way; nothing promoted
make ship                                     # check staging, ask, promote to production, deploy the relay, wait for live, check live, report
make ship ARGS=--live-only                    # the live half again
```

## The pieces

- **docs/FEATURES.md**, the manifest. Every API route the box lists at
  `GET /api`, every pane and bottom tab, and every agent profile the box asks
  the relay for is in exactly one feature's row, with the journey that walks
  it. The table is held to the code both ways:
  helmsman-plc `tests/features.rs` reads a running box's route list and
  `--help`, and helmsman-canvas `src/features_tests.rs` reads the tabs. A new
  surface with no row fails the gate, and so does a row naming a surface the
  code no longer has.
- **`helmsman-compat parity`** (crates/helmsman-compat/src/parity/) walks
  every journey on one box: a `--projects` box on loopback with an engineer
  account. Each journey is the main path of its feature through the box's
  API (sign in, make a project, deploy it, see it run, ask the agent), and it
  checks the answer's shape, not the edge cases each crate tests itself.
  One finding per feature. A journey not written yet is *pending*, and an
  agent journey with no relay is *not run*. Neither is a finding: both are
  counted, and `make ship` reports either one on live as a gap against a
  pass on the candidate.
  - Agent side: `--relay-bin BIN` puts the stub model behind that relay (the
    candidate, no model spend), and `--url URL --token FILE` points the box
    at the deployed relay (live, one short real turn per agent route).
    Turns are judged on their event shape, never on the wording.
  - Devices: simulators only, never a bench unit. Device settings stays
    pending until the box can be pointed at a console simulator (it reaches
    a unit's console on port 80 only).
  - `tests/features.rs` runs every written journey on this checkout's box in
    the normal gate, so a broken main path fails on dev before it can become
    a gap between dev and live. Its agent journeys run only on the relay
    `COMPAT_RELAY_BIN` names (else NOT RUN), never a build found on disk:
    an old one serves no design or help profile.
- **scripts/parity-tabs.py** opens every manifest tab on the playground:
  `--dist` serves a build (the candidate), `--site` the public page (live).
  Each tab must draw, look unlike the other tabs of its panel, and log no
  error. Screenshots go to `target/parity-tabs/`.
- **scripts/ship.sh** (`make staging`, `make ship`) runs the steps below.

## What they do

Run either from the main checkout on `dev`, pulled. Ship runs its own scripts and
the checker from that checkout, so it first compares them with `origin/dev`
and stops, with the command that updates the checkout, if they differ
(`SHIP_LOCAL_SCRIPTS=1` runs local copies, for changing ship itself).

0. **Playground** (`make staging`). When the canvas code has changed since the commit that
   last wrote dev's `docs/playground`, the page is rebuilt at the dev tip
   (`scripts/build-playground.sh`, with the relay stack's `DemoUrl` and
   `DemoDesignUrl`) and merged into dev by a pull request before anything
   is checked. On 2026-11-21 a ship went out with a page built six days
   earlier. `--dry-run` only says the page is stale. `make ship` builds
   nothing: a stale page on staging stops it, and `make staging` is the fix.
1. **Candidate.** `scripts/compat.sh --candidate` (contracts and CI of the
   four candidate tips, written to `target/compat/train.txt`), then the journeys on the box compat built there, with
   the relay it built at agent-relay's candidate, and the tabs on the candidate's
   committed playground, which is what master will serve. Any failure stops
   here with nothing promoted.
2. **Promote.** It shows how far each candidate is ahead. `make staging`
   promotes without a question; `make ship` asks you to type `ship`
   (`--yes` skips the question). Each repository whose candidate is
   ahead gets a `ship/<stamp>` branch at the candidate commit and a pull
   request from it into `staging` (`make staging`) or the default branch
   (`make ship`), merged with a merge commit once its checks are green. What
   merged after the check is not promoted with it. `make staging` ends
   here. For `make ship`, agent-relay is
   then deployed from a throwaway worktree at `main` (`make deploy`, which
   keeps the stack's settings).
3. **Live.** It waits for `scripts/live-state.sh --at` each candidate commit
   (up to 20 min, for the Pages build) to say every row but the try box is
   live; commits merged since are not waited for. Then it runs the
   journeys on a box built at the new master, held to *the candidate's* features, and
   with the deployed relay, and the tabs on the public site.
4. **Report.** `target/ship/report.md` lists every journey and tab with its
   candidate and live outcome. A feature that passed on the candidate and
   is anything else live is a regression: it is named, and a **draft** pull
   request reverting helmsman's promotion merge is opened on master for you
   to merge or close. Nothing is force-pushed or reset.

Exit status: 0 shipped and live matches (`make staging`: promoted), 1 a check failed or live
regressed, 2 something could not run, 3 you answered no.

## Adding a feature

Give the new route, tab or profile a row in docs/FEATURES.md, or add it to the
feature it extends. Write its journey in the area file it belongs to
(`parity/mod.rs` for the box's own, `agent.rs`, `devices.rs`, `more.rs`) and
put it in that file's `JOURNEYS`. The gate tells you if you forgot either.

## What it does not cover

- Edge cases, error paths and security. Those are each crate's own tests.
- Real devices and the node board: the benches (`scripts/bench-e1213.sh`,
  helmsman-hardware `docs/bringup-test.md`).
- The box-served page's tabs (the tab pass drives the playground) and the
  Pi image, which `box-image.yml` boots under QEMU on its own schedule.
- A box on site with `--site` (multicast, a fixed port).
