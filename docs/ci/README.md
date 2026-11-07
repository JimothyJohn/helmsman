# Requirement tests in CI

A Helmsman project is a folder of Structured Text: `main.st` and any other
program files, `io.map`, `project.settings`, and a `test_NAME.st` file for
each requirement. `helmsman-plc test` checks that folder the way a deploy
builds it and runs its requirement tests in the simulation, the same tests
the box runs before it deploys. No box has to be running and nothing goes
on the network, so a project kept in git can be gated on every change by
any CI runner, with no vendor IDE in the loop.

```text
helmsman-plc test PROJECT_DIR [--junit FILE] [--scan-ms MS]
```

```text
project pump-station (pump-station)
  builds: 1 module(s): pump_station
  untested  test_dry_well: Neither pump runs while the low float says the well is dry. (no test yet)
  pass      test_green_lamp: The green lamp is on while the station runs: start_pb turns it on and stop_pb turns it off.
2 test(s): 1 passed, 0 red, 1 untested
```

## Exit status

| Status | Meaning |
|---|---|
| 0 | The project builds and no test is red. |
| 1 | The project builds and at least one test is red: each red one is printed with the check that failed and when. |
| 2 | The project does not build, or a file in it breaks the project rules. Each problem is printed as `PROJECT_DIR/FILE:LINE:COL: message`. |
| 64 | Usage: a bad flag, or `PROJECT_DIR` is missing, not a folder, or holds no `.st` file. |
| 74 | A file could not be read, or the report could not be written. |

A requirement written down with no test behind it yet ("untested" on the
Requirements tab) never fails the run, just as it never refuses a deploy.
It shows in the output and in the report so it is not forgotten.

The tests run at the box's default scan period, 10 ms. If your box runs
with a different `--scan-ms`, pass the same value here.

## The JUnit report

`--junit FILE` also writes a JUnit XML report, which GitHub, GitLab,
Jenkins and Azure DevOps all read. The file is written whole or not at
all. It holds one test suite named after the project folder:

- `build`: passes when the project builds; an `<error>` listing every
  problem when it does not.
- one test case per `test_NAME.st`, named `test_NAME: the requirement`
  (the file's first comment). Red is a `<failure>` whose message is the
  check that failed and when. Untested is `<skipped/>`.

## GitHub Actions

[`helmsman-test.yml`](helmsman-test.yml) is a workflow to copy into your
project's repository as `.github/workflows/helmsman-test.yml`. Set two
values at its top:

- `PROJECT_DIR`: the project folder, relative to the repository root.
- `HELMSMAN_REV`: the Helmsman commit your boxes run, so CI tests with the
  same build rules and simulation as the plant.

It builds `helmsman-plc` at that commit, runs the tests on every pull
request and push to `main`, prints the results on the run's summary page
and leaves `helmsman-test.xml` for a test-report step of your choice. The
actions it uses are pinned by commit.
