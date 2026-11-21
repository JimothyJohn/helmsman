# Feature parity and `make ship`

`make compat` (docs/COMPAT.md) is the horizontal check: the four
repositories held to each other's contracts. This is the vertical one: every
feature a user reaches, walked end to end on the candidate (the dev tips) and
on live, so a promotion is one command and its report says, feature by
feature, whether live does what dev does.

```sh
make parity                                   # every journey on this checkout's box (no relay: agent journeys not run)
make parity ARGS="--relay-bin target/compat/build/agent-relay-dev/debug/agent-relay-local"   # agent journeys on the stub (the relay make compat builds at agent-relay dev)
make ship ARGS=--dry-run                      # the candidate half: contracts, CI, journeys, tabs; nothing promoted
make ship                                     # check, ask, promote, deploy the relay, wait for live, check live, report
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
- **scripts/ship.sh** (`make ship`) runs the four steps below.

## What `make ship` does

1. **Candidate.** `scripts/compat.sh --candidate` (contracts and CI of the
   four dev tips), then the journeys on the box compat built at dev, with
   the relay it built at agent-relay's dev tip, and the tabs on dev's
   committed playground, which is what master will serve. Any failure stops
   here with nothing promoted.
2. **Promote.** It shows how far each dev is ahead, then asks you to type
   `ship` (`--yes` skips the question). Each repository that is ahead gets a
   dev → default pull request, merged with a merge commit. agent-relay is
   then deployed from a throwaway worktree at `main` (`make deploy`, which
   keeps the stack's settings).
3. **Live.** It waits for `scripts/live-state.sh` (up to 20 min, for the
   Pages build) to say every row but the try box is live. Then it runs the
   journeys on a box built at the new master, held to *dev's* features, and
   with the deployed relay, and the tabs on the public site.
4. **Report.** `target/ship/report.md` lists every journey and tab with its
   candidate and live outcome. A feature that passed on the candidate and
   is anything else live is a regression: it is named, and a **draft** pull
   request reverting helmsman's promotion merge is opened on master for you
   to merge or close. Nothing is force-pushed or reset.

Exit status: 0 shipped and live matches, 1 a check failed or live
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
