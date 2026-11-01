# Contributing to Helmsman

Helmsman is a soft PLC for factory edge boxes: a Rust cargo workspace under
`crates/helmsman-*`. `docs/ARCHITECTURE.md` is the map of the code,
`docs/UI.md` the shape of the browser app, `TODO.md` the plan and the open
questions. Read the architecture note before a change that crosses a crate.

## Set up

- Rust stable (`rust-toolchain.toml`), at least 1.88 (`Cargo.toml`
  `rust-version`). `make install-dev` adds rustfmt, clippy and the
  `wasm32-unknown-unknown` target the canvas builds for.
- `make help` lists every target. Each is a thin wrapper over a script in
  `scripts/`, so CI and you run the same thing.
- Docker is needed only for the interop suite (`make opener-up`,
  `make test-interop`) and the ST oracle (`make test-oracle`).
- The canvas runs natively (`make canvas`) or in the browser
  (`make canvas-web`, then `make canvas-serve`).

## The gate

Run both before every push, each on its own, and read the result:

```
./scripts/lint.sh                      # fmt, clippy (warnings denied),
                                       # helmsman-plc without `box`, canvas for wasm32
cargo test --workspace --locked
```

CI (`.github/workflows/ci.yml`) runs the same `gate` plus `interop` against
an OpENer container; both are required on `dev`.

Rules the gate enforces or that cost a day to learn:

- **`box` is a feature boundary.** Anything in `helmsman-plc` that touches
  the network, SQLite, TLS or the OS goes behind its `box` feature (test
  targets get `required-features = ["box"]`), and the crate must still
  build without it: the canvas embeds that runtime as wasm.
  `crates/helmsman-canvas/tests/wasm_deps.rs` fails if a native-only crate
  reaches the canvas.
- **No fixed sleeps in tests.** Wait on a condition with a deadline
  (`wait(...)` in `crates/helmsman-plc/tests/reload.rs`). The suite must pass
  with several copies running at once.
- **A bug fix starts with its regression test.** It fails before the fix,
  passes after, and stays. Never skip or delete a failing test to clear CI.
- **Protocol claims cite a source.** EtherNet/IP, CIP, Modbus and IEC 61131
  behaviour gets an entry in `docs/SOURCES.md` with a section or page.
- **Dependencies are a supply-chain decision.** Prefer the standard library
  or a crate already in the tree; name the trade-off in the PR.

## Branches and pull requests

- Branch from `dev`: `git fetch origin && git switch -c <topic> origin/dev`.
- Open the PR against `dev`. Green required checks are the approval; a PR
  that needs discussion stays a draft.
- `dev` requires branches to be up to date. Merge `origin/dev` in (no
  rebase, no force push) before pushing a PR that has been open a while.
- Pull requests land one at a time. Two PRs that each pass can break `dev`
  together (a new enum variant against a new exhaustive match merges cleanly
  and does not compile), so overlapping PRs are test-merged on a throwaway
  branch first.
- `dev` is promoted to `master` by the maintainer. Never open a PR against
  `master` or push to it.
- Keep a PR to one change. Commit messages say what changed and why, in
  plain prose; the subject names the TODO card when there is one
  (`feat(seed): ... (seed-deploys)`).

## Text people read

The site, the README, UI text and release notes are written for a controls
or plant engineer, not a developer. Say what the product does for them, in
plain sentences, and claim only what is built.

## Licensing and sign-off

The crates are published under the MIT license (`Cargo.toml`). By opening a
pull request you certify the [Developer Certificate of Origin](https://developercertificate.org/):
you wrote the change or have the right to submit it under that license.
Sign each commit off (`git commit -s`), which adds the `Signed-off-by:`
line.

## Reporting

- Bugs and feature requests: open an issue with the templates under
  `.github/ISSUE_TEMPLATE/`. A bug report needs the version or commit, what
  you did, what happened and what you expected; a stack trace or the
  journal lines from the box if there are any.
- Security problems: do not open a public issue. Email the maintainer
  (see the commit log) and allow time for a fix before disclosure.

## Conduct

Be direct and be kind. Review the code, not the person; assume the other
side is competent and busy. The maintainer removes anyone who makes the
project unpleasant for others.
