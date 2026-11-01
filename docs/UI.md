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
- **Centre: the live diagram.** Pan, zoom, drag blocks to rearrange,
  breadcrumbs with ↑ (Alt+↑), double-click an instance to go into it.
  An engineer saves the arrangement as the project's shared default
  (Save as default; the project's `diagram.layout`, so it travels in the
  `.zip`); anyone's further moves are kept in their browser on top of it.
  Reset layout goes back to the shared default, a second step to the
  automatic layout. The simulation does the same with its projects.
- **ST on demand:** an "ST" toggle slides the editor open beside the
  diagram; clicking a block jumps the editor to its line.
- **Right panel, collapsible, tabs:** I/O (values; inputs the diagram does
  not show), Trend (pinned signals), Claude. On a Pi the Trend opens with
  the box's history of each pinned signal (the last 10 minutes), then
  streams; pins are the signed-in person's, kept by the box, or the
  browser's when signed out.
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
  L5X files into the open project (XML and L5X ladder converted to ST).
- In the browser simulation, projects persist in the browser's storage, with
  the same Upload and Download (the box's `.zip`). The built-in examples are
  read-only starter projects: Save keeps your own copy. Typing only checks:
  errors are marked after a pause, and what runs is unchanged, as on a box.
  Deploy compiles and runs the project in the page; one that does not
  compile is refused and the old program keeps running. A browser that
  keeps nothing (a private window) still works; the projects last until
  the page closes.
- **Machines and templates.** Worked machines (conveyor sorter, pump
  station, batch mixer, pick and place, bottle filler, air compressors,
  air handler) and templates (discrete machine, process loop) are one list,
  kept as files in `crates/helmsman-project/starters/`. In the simulation
  the Files sidebar shows Examples (the short teaching programs, in view,
  and the one a first visit opens), then Machines and Templates, folded
  until opened; each is a read-only starter that Save copies. On a box,
  "Start from a machine or template" in the sidebar asks an engineer for a
  name and sends the project as its `.zip`; nothing else changes what the
  box holds, and a first install still seeds `line` alone.

## Monitor (phone to wall TV; replaces Andon)

- **Top:** status tiles: running or faulted, scan time, then one tile per
  top-level instance with its key outputs, green or red.
- **Tap a tile:** the same tiles one level down (that instance's instances
  and outputs). Back and breadcrumbs go up; the full diagram is one tap
  away.
- **Watch only.** The Monitor never writes.

As built (step 3): a group per PROGRAM (a module on a box), each with its
status tile (RUNNING, FAULTED with the fault, HELD, STOPPED; the scan
period and count), a tile of the PROGRAM's own outputs (`VAR_OUTPUT`s
and located `%Q` variables), then a tile per user function block
instance in declaration order (standard blocks such as TON are not
tiles). A BOOL output is a lamp (TRUE green, FALSE red; card
`monitor-lamp-polarity`), a number its value. "Open diagram" opens the
Editor's diagram at the level shown. Stale data is never a colour: a
program with no new scan for 3 s, or every program while the box is out
of reach, goes grey with "stale" and the age, under a red NO DATA banner
when the box itself is the problem; a stopped simulation is grey too.
Phones (under 600 points wide), `?monitor` and the old `?andon` open on
it; the header switches either way. Tiles are at least a finger wide,
one column on a phone, and grow with the screen up to 4K.

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
