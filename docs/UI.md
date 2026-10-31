# The Helmsman UI

What the browser app is, agreed with Nick on 2026-10-31 (interview). Build to
this; change it here first when a decision changes.

## Principles

- **Structured Text and its live diagram, nothing else.** The diagram is a
  read-only view of the ST: it never edits the program. (The node-graph Lab
  editor is gone.)
- **One app wherever it runs.** Served by a Pi, it talks to that Pi. Opened
  from the public site (docs/playground), it runs a simulated box in the
  page. Same screens, same buttons; the source is a setting, not a mode.
- **Two screens: Editor and Monitor.** No Lab, Live, Site, Demo, Andon or
  Playground modes.
- **Navigation is never lost.** The header is always there: project name,
  Editor | Monitor, Save, Deploy, login, and a ⋯ menu (Proposals with a
  badge when one waits, Upload, Download).

## Editor (desktop and tablet; a phone opens the Monitor)

- **Left sidebar, collapsible:** the open project's files, with the program
  tree under them (PROGRAM › instance › instance, foldable; a click opens
  that scope's diagram).
- **Centre: the live diagram.** Pan, zoom, drag blocks to rearrange (kept
  per project, Reset layout), breadcrumbs with ↑ (Alt+↑), double-click an
  instance to go into it.
- **ST on demand:** an "ST" toggle slides the editor open beside the
  diagram; clicking a block jumps the editor to its line.
- **Right panel, collapsible, tabs:** I/O (values; inputs the diagram does
  not show), Trend (pinned signals), Claude.
- **NOT is a bubble** on the pin it feeds (or the output it inverts), the
  FBD convention, not a block of its own. Display only.
- **Why is the diagram itself:** tapping an output highlights the blocks and
  wires that drive it and dims the rest; tapping empty space clears it. No
  Why tab or panel.
- **Inputs on the diagram:** in the simulation, an input's block switches,
  presses or drags it. On a Pi, the same click **forces** the input, for an
  engineer only: the block shows FORCED in the warning colour, a banner
  lists every force with "release all", and forces end on logout or deploy.

## Projects

- A project is a folder of free-form `.st` files that compile together; one
  or more PROGRAMs in it run as tasks.
- On a Pi: `/var/lib/helmsman-plc/projects/<name>/`. Many stored, one
  running. Opening or editing another project never disturbs the running
  one.
- **Save** writes the edited files (a draft; a file someone else saved
  since it was read is refused, and the page offers to reload it).
  **Deploy** saves first, compiles the open project, records a snapshot in
  the store (history and rollback keep working) and hot-swaps it in
  between scans; compile errors list file and line, and a click goes
  there.
- **Who:** only an engineer login edits, saves, deploys or forces. Anyone
  else watches: the ST is read-only and the write controls are disabled,
  saying why. Sign in and out from the header.
- **Download:** a `.zip` of the project folder (its `.st` files, the I/O map,
  settings). **Upload:** that `.zip` back, or single `.st`, PLCopen XML or
  L5X files into the open project (XML and L5X converted to ST).
- In the browser simulation, projects persist in the browser's storage, with
  the same Upload and Download.

## Monitor (phone to wall TV; replaces Andon)

- **Top:** status tiles: running or faulted, scan time, then one tile per
  top-level instance with its key outputs, green or red.
- **Tap a tile:** the same tiles one level down (that instance's instances
  and outputs). Back and breadcrumbs go up; the full diagram is one tap
  away.
- **Watch only.** The Monitor never writes.

## Elsewhere

- **Proposals** (agent changes approved with a PIN) sit behind the ⋯ menu
  until they get a home.
- **Claude** on a Pi goes through the relay and proposes; in the simulation
  it uses the visitor's key, as the playground does today.
- **Site** (every box on the network) is gone for now; it returns with a
  fleet view.

## Build order (one PR each)

1. Clean-up: remove Lab, Why, Site, Demo and the Andon mode; the always-on
   header; collapsible sidebars; NOT bubbles; the highlighted path.
2. Projects on the Pi: folders, API, Save and Deploy, upload and download
   with import, the engineer gate, forcing.
3. The Monitor.
