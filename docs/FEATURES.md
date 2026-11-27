# Features

Every surface a user reaches, under the feature it belongs to, with the
journey that shows the feature works. `make ship` walks every journey on
the candidate (the dev tips) and on live, and reports any feature that
works on one and not the other (docs/PARITY.md).

The table is held to the code both ways. helmsman-plc
`tests/features.rs` reads the box's own route list (`GET /api`) and the
`--relay-*-agent` flags of its `--help`; helmsman-canvas
`features_tests.rs` reads the pane and bottom tabs. A route, tab or
agent profile with no row here fails the test, and so does a row that
names one the code no longer has. When you add a surface, add it to the
feature it belongs to, or give it a new row and a journey
(crates/helmsman-compat `parity`).

How a surface is written: `METHOD PATH` exactly as the box's route list
writes it (query included); `tab:KEY`, the key the tab saves in a layout;
`profile:ROLE`, the box's `--relay-ROLE-agent` flag (`--relay-agent` is
`chat`).

| Feature | Journey | Surfaces |
|---|---|---|
| Box is up | `health` | `GET /api/health`, `GET /api/running` |
| Sign in | `sign-in` | `GET /api/login`, `POST /api/login`, `POST /api/logout`, `GET /api/session`, `GET /api/policy` |
| Live values | `live-values` | `GET /api/signals`, `GET /api/signals/{name}`, `GET /api/outputs`, `GET /api/events`, `tab:io` |
| Run and stop a program | `module-control` | `GET /api/modules`, `GET /api/modules/{name}`, `POST /api/modules/{name}/stop`, `POST /api/modules/{name}/run`, `POST /api/modules/{name}/restart` |
| Why is it in this state | `explain` | `GET /api/modules/{name}/trace`, `GET /api/modules/{name}/trace/events`, `GET /api/traces/events`, `POST /api/explain`, `profile:explain` |
| Projects and files | `projects` | `GET /api/projects`, `POST /api/projects`, `GET /api/projects/{p}`, `DELETE /api/projects/{p}`, `POST /api/projects/{p}/rename`, `GET /api/projects/{p}/files/{f}`, `PUT /api/projects/{p}/files/{f}`, `DELETE /api/projects/{p}/files/{f}`, `GET /api/projects/{p}/zip`, `POST /api/projects/{p}/zip?replace=true`, `POST /api/projects/{p}/import?file=F&replace=true`, `tab:files`, `tab:code` |
| Check and deploy | `deploy` | `POST /api/projects/{p}/check`, `POST /api/projects/{p}/replay`, `POST /api/projects/{p}/deploy` |
| Requirements and tests | `requirements` | `POST /api/projects/{p}/tests`, `POST /api/projects/{p}/requirements`, `profile:tests` |
| Proposals and approval | `proposals` | `GET /api/proposals`, `GET /api/proposals/{id}`, `POST /api/proposals`, `POST /api/proposals/check`, `POST /api/proposals/replay`, `POST /api/proposals/{id}/replay`, `POST /api/proposals/{id}/apply` |
| History and rollback | `history` | `GET /api/snapshots`, `GET /api/snapshots/head`, `GET /api/snapshots/{id}`, `GET /api/snapshots/{a}/diff/{b}`, `POST /api/snapshots/{id}/replay`, `POST /api/snapshots/{id}/rollback`, `GET /api/replays/{n}` |
| Bundles and the marketplace | `bundles` | `GET /api/snapshots/{id}/bundle`, `POST /api/bundles`, `POST /api/market/list`, `POST /api/market/install`, `profile:review` |
| Audit trail | `audit` | `GET /api/audit?after=N&limit=M`, `tab:messages` |
| Agent chat | `chat` | `POST /api/chat`, `GET /api/chat/status`, `POST /api/dropins?name=N&kind=K`, `GET /api/dropins`, `GET /api/dropins/{id}?chunk=K`, `GET /api/dropins/{id}/plc?view=V&name=N&offset=O`, `profile:chat`, `tab:claude` |
| Design step | `design` | `POST /api/design`, `profile:design` |
| Help and issues | `help` | `POST /api/help`, `profile:help`, `tab:help` |
| Devices and the I/O map | `devices` | `GET /api/devices`, `GET /api/adapter`, `GET /api/io_map`, `GET /api/dhcp`, `POST /api/devices/{name}/browse`, `tab:devices` |
| Device settings | `device-config` | `GET /api/devices/{id}/config`, `PUT /api/devices/{id}/config/password`, `DELETE /api/devices/{id}/config/password`, `POST /api/devices/{id}/config/set`, `POST /api/devices/{id}/config/restart`, `POST /api/devices/{id}/config/import`, `POST /api/devices/{id}/config/password`, `POST /api/devices/{id}/config/snapshot` |
| Device profiles | `device-profile` | `GET /api/device-profile/proposal`, `POST /api/device-profile/proposal`, `POST /api/device-profile/proposal/accept`, `DELETE /api/device-profile/proposal`, `POST /api/device-profile/publish`, `profile:publish` |
| Forces | `forces` | `GET /api/forces`, `POST /api/forces`, `DELETE /api/forces/{id}`, `DELETE /api/forces` |
| Placeholder (sim) points | `sim-points` | `PUT /api/signals/{name}` |
| Trends | `trends` | `GET /api/trends?signals=a,b`, `GET /api/trends/pins`, `PUT /api/trends/pins`, `tab:trend` |
| Desk and layouts | `layout` | `GET /api/desk`, `PUT /api/desk`, `GET /api/pane-layouts`, `PUT /api/pane-layouts`, `DELETE /api/pane-layouts/{name}` |
| Drivers | `drivers` | `GET /api/drivers`, `PUT /api/drivers/{name}`, `DELETE /api/drivers/{name}` |
| Site view | `site` | `GET /api/site` |
| Agent tools for your own client | `mcp` | `POST /mcp` |

## On the page alone

What follows is page state with no route, tab or profile of its own, so
it has no row above and no journey: the box's side of it is the rows it
uses (`deploy`, `health`, `history`, `projects`). The canvas's own tests
walk it through the page (docs/UI.md "Show Changes", Nick 2026-11-27).

- **A described change is held.** Describe tab, type an instruction,
  Send (Enter). A checked answer is not written: the tab shows the
  pending change (`agent pending`), its size in blocks and lines (`agent
  pending size`), the outputs it switches by name (`agent pending
  outputs`), and Show Changes, Apply, Discard (`agent show changes`,
  `agent apply`, `agent discard`). Nothing is written, saved, run or
  deployed until Apply; Send is off ("Apply or Discard the change first").
  Apply writes it as before (the simulation runs it at once; on a box it
  is the file's PUT, deployed only from the sheet below); Discard changes
  nothing; Apply refuses when the file changed since Send. One step is at
  most 8 blocks or 30 lines and switches only outputs the instruction
  names; past that Apply is off and `agent pending refusal` says why. A
  current file that does not compile is held to the line cap, with `agent
  pending note`. A new program from an example is not held. Tests:
  `ui_tests::show_changes`, `box_projects_tests::on_a_box_*`.
- **Show Changes.** `agent show changes` with a change held: the Code tab
  shows the proposed file read-only (`st proposed`, `changes code note`)
  and the canvas the proposed diagram under the banner "Proposed change,
  not applied: the machine runs the current program" (`changes banner`),
  with Show current, Apply, Discard (`changes show current`, `changes
  apply`, `changes discard`). The simulation or the box runs the current
  program the whole time. Tests: `ui_tests::show_changes`.
- **What changes on the machine.** On a box, the Code header's deploy
  arrow, File › Deploy, a file's right-click Deploy and Ctrl/⌘ Shift Enter
  open the sheet (`deploy sheet`) and deploy nothing. It shows each file's
  diff against the running program (`deploy sheet diff <file>`), the
  counts (`deploy sheet counts`), the outputs whose logic changes (`deploy
  sheet outputs`), the check, the tier and who may apply, the replay
  (`deploy sheet check`, `deploy sheet tier`, `deploy sheet replay`) and
  the first changed module's diagram (`deploy sheet diagram`). Deploy on
  the sheet (`deploy sheet deploy`) sends one deploy; Cancel (`deploy
  sheet cancel`) sends none, also while the check or replay is out; when
  the box already runs the project it says so (`deploy sheet same`) and
  Deploy is off. Deploy in the simulation is unchanged, with no sheet.
  Tests: `box_projects_tests` (`every_way_to_deploy_*`, `cancel_while_*`,
  `deploy_on_the_sheet_*`, `the_sheet_names_*`).
