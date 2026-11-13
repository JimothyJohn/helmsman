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

- **One header row (48 pt):** on the left the HELMSMAN wordmark (a link
  to the site) and a menu bar: File (New project, Copy project, Save,
  Deploy, Upload, Download),
  Edit (Undo, Redo, Go to definition, Cross references), View (Editor /
  Monitor, Structured Text, Files, the side panel and its tabs, Trend,
  Zoom to fit, the layout items, Scan detail for an engineer, and Light
  mode: dark by default, the choice kept per browser with the layout) and Help
  (Help and Keyboard shortcuts on the site, About Helmsman: version,
  build, licence, and on a box its certificate fingerprint). After them
  the program's state, green running, red stopped or faulted (a box: its
  link). On the right the project's name, its edited mark, a "n waiting"
  badge when proposals wait, the Editor | Monitor toggle (a glyph a
  side since 2026-11-13, `< >` and a screen; a click on either side
  flips it), Sign in on a box, and ? in the very corner (help
  on the site). Proposals and "Simulate in this browser / Back to this
  box" are View items. A fault, a deploy under way or a stale file shows
  on a line under the header only while it lasts. Below the narrow width
  (a phone) the wordmark, the state and the "simulation" tag are hidden,
  the four menus fold into one Menu button with File, Edit, View and Help
  as submenus, and the project's name is truncated, so the two groups
  never overlap (2026-11-08).
- **Glyphs over the diagram's top-right corner:** run/stop, step and
  restart (the simulation), then save, upload, download. Strokes in the
  brand colours; each tells its key on hover.
- **Bottom panel**, under the diagram, collapsible and resizable (View ›
  Trend, Alt 2), two tabs: Trend (its time axis is the long side; one
  Window slider, logarithmic from 100 ms to 10 min, the plot following
  as it moves; Nick, 2026-11-02) and
  Messages (the last 200 things the page had to say, filtered by level
  with the counts on the filters: info, warnings,
  errors, with the error count on the tab), so a fault or a refused
  deploy is still there to read after the line under the header has gone.
- **Panes collapse** from their chevron, their Alt key, or a double click
  on any of their tabs (Nick, 2026-11-07), on the left pane and the side
  panel alike.
- **Left pane, collapsible, tabs (320 pt):** Files (the projects, the
  open one expanded with its files under it, Nick 2026-11-13: no
  separate files section and no program tree, the diagram's crumbs
  reach the POUs; a right-click on a project offers Copy, Rename,
  Delete and, on the open one, Paste, on a file Copy, Paste, Duplicate, Rename and
  Delete with an app-wide clipboard so a file pastes into any open
  project, and on the blank space New project and Paste; New project and
  Copy project are also the File menu's first items; the Delete key
  deletes the row last clicked, a double click on a file opens the Code
  tab, every delete is confirmed in a modal at the screen's centre, and
  the earlier iterations of the program, `vN.st`, fold under one
  "earlier (n)" row with `main.st` marked as the one running; the running
  program, open, carries a → that deploys it again with the latest edits,
  as does the Code tab's Structured Text header, Nick 2026-11-08),
  Devices (Nick, 2026-11-02: the I/O map's devices as a tree like the file
  system's, each with its points under it, live values and the link's
  state, which on a box says the protocol a device by its profile is
  reached over and whether the link is slow (2026-11-12); on a box the
  open project's `io.map`, else the map the box runs; under the map's
  devices, "On the network": Scan asks the box for one scan on every
  protocol (`POST /api/devices/discover`; the simulation answers with its
  simulated E1213; since 2026-11-11 the request asks EtherNet/IP, Modbus
  and, where the build has it, PROFINET, and answers `found`, one entry
  per device however many protocols it answered on, which the tab reads,
  with the older `devices` list as the fallback for an older box) and
  lists every device that answered by vendor and model, with whether a
  map device binds it and what the public device library knows of it
  (github.com/JimothyJohn/helmsman-devices, looked up by identity first,
  then by vendor and model text; `--devices-index` points a box at
  another). **Add by click** (2026-11-12, PROTOCOLS.md step 3): a device
  the library knows, whose entry names a profile (`NAME.device`), offers
  Add: a small form asks one thing, the device's name, and lists the
  channels it will get; Add copies the profile into the project as
  `NAME.device` (the box fetches and checks it, `GET
  /api/devices/library`; the simulation carries its E1213's), writes
  `device NAME device:PROFILE IDENTITY` with the identity the scan saw,
  and one `ch:` signal per channel named `NAME_channel`. No protocol,
  address, byte order or cycle time is asked. A device the library does
  not know says so and offers Advanced. **Device configuration**
  (2026-11-13, `devconf-canvas`, `device_config.rs`): under an open
  device whose profile has `configure moxa-e1200`, a Configuration section
  reads first, in words: the firmware, what the outputs do if the box stops
  (the device's watchdog), which outputs switch on at power-up, whether
  EtherNet/IP and the RESTful interface are on, and how many settings differ
  from the project's saved `ID.devconf` (the lines on demand). One Actions
  control holds the engineer's work: Save a snapshot; Change a setting (the
  watchdog and its time, each output's safe value, EtherNet/IP, RESTful),
  staged on the device and marked "Staged, not applied yet" until Apply;
  Apply (restart), which says first that the device is offline for about
  21 s and, when the box answers 409, names the modules it would interrupt
  and asks again; Restore from snapshot (the IP address stays as it is);
  the stored console password (write-only, the field never shows a value)
  and Change the console's password. Everyone else sees the controls
  disabled with the reason as the tooltip. Errors are the box's own words.
  **Advanced** (a tick box beside
  Scan, off by default, remembered in the browser) is the older way: a
  right-click on the blank space adds a device as a file is added: a
  form in the centre, name, protocol (Modbus TCP, EtherNet/IP), address
  and settings, written as one `device` line of `io.map`; Import device…
  takes a vendor's EDS, which joins the project as `NAME.eds` and fills
  the form in (EtherNet/IP, `eds=NAME.eds`); the box reads the file from
  the project when the map loads (2026-11-02, `device-import`); and Use
  this device on a found one, the form filled in by identity
  (`cip=V:P:S`) with the library's name and profile (2026-11-02,
  `devices-repo`); the same listing, network
  included, is the context the agent gets, in the page and as the box's
  `list_devices` tool; under that, "Plant states" (2026-11-02,
  `sim-feedback-devices`): the quantities the project's plant model
  keeps, marked `{state [unit=U] [measured_by=NAME]}` on their VAR line
  (the IEC pragma the compiler skips), each with the simulation's value,
  the map `in` signal that measures it on the real machine (found by
  name: the state's, `NAME_in`, `NAME_raw`, the name without its unit
  suffix, or the attribute) or, unmeasured, the sensor kind that would
  (level transmitter, pressure transducer, RTD, encoder, flow meter,
  load cell, counter input, a switch); a right-click on an unmeasured
  state offers Add a sensor…, a form in the centre (signal name, the
  map's device, point, unit) that appends the `in` line; a measuring
  signal that is one of the program's inputs, in the state's unit and
  not a `_raw` count, stands in for the model's variable in the
  simulation (it is written into each instance before every scan, so
  the model runs from the sensor, set in the I/O tab); the agent's
  context carries the same list, so it can suggest and reason about
  what the machine does not measure yet) and Code (the Structured Text
  editor). One width
  for every tab (320 pt since 2026-11-10, so the Code tab holds 27
  characters a row); Alt Shift 0, Alt Shift 4 and Alt Shift 1 open the
  pane on its tab, or shut it when that tab shows.
- **Right panel, collapsible, tabs (236 pt):** I/O
  (values; inputs the diagram does not show), Palette (the glyph
  library's pieces, `palette`: a click, or a drag onto the diagram, sends
  the piece's request to the Agent as if typed, and the panel turns to
  the Agent tab; Nick, 2026-11-12) and Agent. (The Requirements
  tab of 2026-11-05, a list of requirements in plain words with the agent
  writing the test behind each, is archived since 2026-11-08 under
  `crates/helmsman-canvas/archive/requirements/`: the scenarios the Agent
  writes with each change are the tests. `test_NAME.st` files still run,
  after every build change and before Deploy and "Use this code" in the
  page, when a project opens, after each save and before a deploy on a
  box; a red one refuses both, naming the test. Test files stay out of
  the Files list.) Agent (Nick, 2026-11-05, `crate::instruct`, the
  same on the page and on a box: no conversation. The tab is an
  instruction box, Send or Stop, Undo, and after an answer a status line
  and the Agent's one-sentence reason. Send updates the open file: one
  request carries the whole file in its scope, the program and its
  errors, and the last 5 instructions carried out on that file, read back
  from its History revisions. `verify::check` runs the answer on a copy
  with its scenarios and the requirement tests; a red one goes back with
  what failed, up to `verify::RETRIES` times, and if it is still red
  nothing is written and the tab says why. Green is written into the file
  as one revision whose History entry records the instruction, the
  reason, the scenarios and the failed attempts; the scenarios are kept
  as test files, and Undo restores the revision before. On a box (card
  `agent-on-box`) the same message goes to `POST /api/chat` with the
  session cookie, a new conversation per instruction; the change is a
  PUT, Undo the box's restore, and a proposal the box stores instead is
  offered for review under Proposals with nothing written. A line on top
  says whether the box's agent is online, offline or not configured;
  signed out, Send stays off. The agent edits only what is in focus (Nick 2026-11-01,
  interview 2026-11-02, card `agent-scoped-edits`): the open file by
  default, the lines selected in the Code tab or the block picked in the
  diagram (its statement, while the diagram is of the editor's text) when
  there is one; a line above the instruction says which ("Editing: main.st",
  "Editing: lines 12–30 of main.st", "Editing: block Motor1") and by which
  rule, with Whole file to drop the selection or pick. The scope goes to
  the model in an `<edit_scope>` envelope (`scope.rs`, `apply`): another
  file, lines outside the selection, a block's neighbour, several code
  blocks, the envelope markers or no change at all are refused, saying
  what it tried to touch, and nothing outside the scope changes, byte for
  byte. With a built-in example open (read-only) the scope is a new
  project: the first answer asks for the program's name in a popup
  ("Use this code") and lands in that project as `main.st`). On a Pi the Trend opens with
  the box's history of each pinned signal (the last 10 minutes), then
  streams; pins are the signed-in person's, kept by the box across
  restarts (history is not), or the browser's when signed out.
- **NOT is a bubble** on the pin it feeds (or the output it inverts), the
  FBD convention, not a block of its own. Display only.
- **Icons on blocks:** one character at the header's left, from the
  project's `icons.map` (`WHAT = ICON` a line: a block type, a name, or a
  prefix with `*`; the project's rules over the built-in ones for the
  standard blocks: T for timers, # for counters, ↑ ↓ for the edges, S R
  for the bistables, ~ for `BLINK_PATTERN`). Zoomed out past where the
  label is drawn, the icon stands alone at a readable size.
- **A fit never goes below a legible zoom** (0.75, about 9 px type): a
  diagram too big for the view at that zoom shows its left part and you
  pan, instead of shrinking to one-letter labels. Controls on blocks cut
  their text instead of wrapping it.
- **The box link's status counts down:** "reconnecting (retry in 2s)",
  ticking each second; every retry waits 2 s (it was 5 s, too slow:
  Nick, 2026-11-13).
- **What happens to the box is said in the middle of the screen**
  (`box_popup`, Nick, 2026-11-13). A first look that takes over 0.4 s
  shows "Connecting to the box"; a box that answered and went away shows
  "Reconnecting to the box" with the countdown (Hide, and Simulate in this
  browser once it is unreachable). Both are a window that leaves the page
  usable, so the last diagram can still be read. A page no box answers
  falls back to the simulation on the first failed look, not the third,
  and says so in a modal; once the simulation is up a second look goes
  in the background, one module list read, and a box that answers then
  is offered in a modal: Connect to the box, or Keep simulating. Nothing
  answers the second look: the modal says so, and no third look goes
  until View › Back to this box.
- **Why is the diagram itself:** tapping an output highlights the blocks and
  wires that drive it and dims the rest; tapping empty space clears it. No
  Why tab or panel.
- **Inputs on the diagram:** in the simulation, an input's block switches
  or presses it, or takes a typed number (written on Enter). On a Pi, the same click **forces** the input, for an
  engineer only: the block shows FORCED in the warning colour, a banner
  lists every force with "release all", and forces end on logout or deploy.

## Projects

- A project is a folder of free-form `.st` files that compile together; one
  or more PROGRAMs in it run as tasks.
- On a Pi: `/var/lib/helmsman-plc/projects/<name>/`. Many stored, one
  running. Opening or editing another project never disturbs the running
  one.
- **Save** writes the edited files (a draft; a file someone else saved
  since it was read is refused, and the page offers to reload it). Every
  save, upload, import and restore is a **revision** of the project on
  the box (Nick 2026-11-01, project-history; interview 2026-11-02:
  revisions, not git): the files as they were, who, when, what, and the
  Agent page the change came from when there was one. The left pane's
  **History** tab (a clock, Nick 2026-11-13) lists them newest first,
  each as one line saying what changed (the Agent's reason when it
  made the change, else what the save said), under Newer and Older that
  step through them (the Up and Down keys too). In the simulation a pick
  runs that version at once, its files in place and marked green and
  red against the version before it, as the Agent's changes are;
  nothing is saved (Save keeps it as the newest) and the newest is a
  step away. A double click opens the revision in the centre with a line
  diff against the project as it is and Restore (an engineer's, a new
  revision); on a box a pick opens that, since a version runs there
  only once deployed. The simulation keeps the newest twenty in the browser's
  storage the same way. Deploy snapshots in the store are untouched.
  **Deploy** saves first, compiles the open project, records a snapshot in
  the store (history and rollback keep working) and hot-swaps it in
  between scans; compile errors list file and line, and a click goes
  there. A revision's diff and a proposal's source diff draw as GitHub
  does: `+` on a green tint, `−` on a red tint, the row the pane's width.
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
- **Glyph library** (Nick, 2026-11-12: "the app shouldn't even need
  words"). The panels' action buttons are glyphs too (2026-11-13,
  `glyphs::action`): Send and Ask for help a yellow paper plane, Stop a
  red square, Undo a hooked arrow; the Editor | Monitor switch `< >` and
  a screen. Words stay where they are the safeguard or the content:
  menus, form labels, dialog confirmations (OK, Cancel, Restore this
  revision, Use this code), Release on a forced input, Sign in, and the
  Devices tab's buttons until the device configuration work lands. One SVG a glyph in `crates/helmsman-canvas/assets/glyphs/`,
  rasterized to alpha masks by the crate's build script and tinted in the
  theme's colours when drawn (`glyph_lib`); redrawing one is replacing
  its file. Placeholders in the spirit of emoji until Nick draws the set.
  Today a glyph stands beside every point in the I/O tab, the box's
  signal list, the outputs list and the Devices tree: by the words of the
  point's name (`start_pb` a push button, `e_stop` an emergency stop,
  `mode_sel` a selector, a colour or `lamp` a pilot lamp, `alarm`,
  `interlock`, `phase`/`step`, `hmi`, `timer`, `count`, `latch`, `edge`),
  else a switch for a BOOL input, a pilot lamp for a BOOL output, a gauge
  for a number; a device by its driver (the box, the screen, an I/O
  module). The standard blocks on the diagram carry theirs in the
  header (timer, counter, latch, edge; `Icons::glyph`), where a rule of
  the project's `icons.map` naming the block or its type still puts its
  character instead. The Palette tab holds the pieces to drag.
- **Lamps** (Nick, 2026-11-12: the first screen should speak without
  words). The program's BOOL outputs are lamps in the diagram's
  bottom-right corner (`lamps`), lit from the scan: a red, a yellow and a
  green together are a traffic light, anything else a row of pilot lamps
  named under them. An output named after a colour (`red`, `lamp_green`,
  `amber`, `blue`, `white`) takes that colour there, in the I/O tab's
  output text when on, and on both trends until a palette colour is
  picked for it. Only the program's own diagram shows them, not a block
  instance's.
- **Examples** (Nick, 2026-11-05). Six programs from the first chapters
  of a Structured Text book, a few lines each: Traffic light (first, Nick,
  2026-11-12), Blink, Read and write, Logic (AND, OR, NOT), Start/stop
  latch, Counter (`playground::EXAMPLES`).
  The worked machines and templates are gone. A click on an example asks
  for the program's name (prefilled, made unique) and opens the person's
  own copy; the example itself never opens for editing (a first visit
  shows the traffic light running). On a box the same list sends the copy as a `.zip`
  with an `io.map` made from the example's own ports, BOOL inputs on the
  box's `di` channels and outputs on its `do` channels, so it deploys with
  nothing wired; nothing else changes what the box holds, and a first
  install still seeds `line` alone, undeployed.

## Keyboard

CODESYS's default keys where the browser leaves them alone (its
"Shortcuts" help page, checked 2026-11-01); nothing on a key the browser
owns, checked on 2026-11-08 against Chrome, Edge, Firefox and Safari
(Ctrl D bookmarks, Ctrl B is Firefox's bookmarks sidebar, F10 the
toolbar on Windows, Alt 1 to 9 Firefox's tabs, Ctrl ] Forward); the
editor's own conventions stay (Ctrl S, Ctrl Z, Ctrl Shift Z, Ctrl F,
Ctrl G), as in every editor on the web. `hotkeys::BROWSER` holds the
browsers' chords and a test keeps the table off them. The page keeps the
browser's default off our keys. Ctrl is ⌘ on a Mac.
Save, Deploy, Upload, Download, Force, Release all, Undo, Redo, Go to
definition, Cross references, the step chords and Breakpoint act only on
the Editor screen; on the Monitor or Proposals they do nothing (Deploy's
key there used to deploy the hidden project). Ctrl Z and Ctrl Y go to the
focused text field alone, never to the editor's history as well
(2026-11-08).

| key | does | CODESYS command on that key |
|---|---|---|
| Ctrl S | Save | Save Project |
| Ctrl Shift Enter | Deploy | (Download has no key; Ctrl D is the browser's bookmark) |
| Ctrl Enter | Run / Stop the simulation | (Start is F5, the browser's) |
| Shift F8 | Stop the simulation | Stop |
| F8 | Step one scan (stopped); from a breakpoint, the rest of the scan | Step Into (Step Over is F10, the browser's toolbar on Windows) |
| Alt F8 | Sign in (box) | Login |
| Ctrl F8 | Sign out (box) | Logout |
| Ctrl Z, Ctrl Shift Z; Ctrl Y off a Mac | Undo, Redo in the ST editor | Undo, Restore |
| Alt Shift 0 | The left pane's Files tab | Devices view (Alt 1 to 9 are Firefox's tabs on Windows and Linux) |
| Alt Shift 1 | The left pane's Code tab (the Structured Text) | POUs view |
| Alt Shift 2 | Trend along the bottom | Modules view |
| Alt Shift 3 | Side panel | Messages view |
| Alt Shift 4 | The left pane's Devices tab | |
| Alt Shift D | Go to definition: the picked block's variable, at its VAR line | Go to Definition (Ctrl Shift D bookmarks every tab) |
| Alt Shift X | Cross references: every read and write of it lit, until the next click | Display Cross References (Ctrl Shift X is Firefox's text direction) |
| Shift F9 | Breakpoint on the picked block (simulation): the scan stops before it, framed in orange; Ctrl Enter goes on to the next stop | (Toggle Breakpoint is F9, Edge's reader; Ctrl B is Firefox's bookmarks) |
| Alt Shift F | Force the picked input: a switch flips, a number's field opens (on a box an engineer's audited force) | (Ctrl Shift F is Firefox's full screen on a Mac) |
| Alt Shift U | Release every force (box, engineer) | (Ctrl Shift U is Edge's Read aloud) |
| Alt ] | Into the picked block instance's diagram; a stopped simulation steps one scan | (Ctrl ] is Forward) |
| Alt [ | Out to the caller's diagram, the instance left picked; a stopped simulation steps one scan | (Ctrl [ is Back) |
| Ctrl F | Find in the ST: every match marked, the current one brighter and scrolled to; Enter / Shift Enter next / previous, Esc closes with the match selected | |
| Ctrl G, Ctrl Shift G | Next / previous match | |
| Alt ↑ | Up one level in the diagram | |

Each menu item prints its key. Reserved, not built (TODO card
`hotkey-agent-reserved`): F2 / Shift F2 the Agent.

## Monitor (phone to wall TV; replaces Andon)

- **Top:** status tiles: running or faulted, then one tile per top-level
  instance with its key outputs, green or red. The scan period and
  counts are too low level for the wall: hidden unless an engineer turns
  "Scan detail" on (card `monitor-scan-time`).
- **Tap a tile:** the same tiles one level down (that instance's instances
  and outputs). Back and breadcrumbs go up; the full diagram is one tap
  away.
- **Watch only, but for an engineer's Stop and Restart.** In the
  simulation the header carries the transport (run or stop, step,
  restart) and the same keys as the Editor (Nick, 2026-11-01). On a box,
  an engineer signed in gets Stop or Run and Restart there (no step):
  they act on the program shown, or on every program at the top level,
  through the box's audited routes (card `monitor-box-run-stop`, Nick
  2026-11-06). A stopped program's tile says STOPPED, "outputs at safe
  values" and who stopped it. For anyone else the Monitor writes
  nothing. The Editor's glyph strip carries the same Stop or Run and
  Restart for every program the box runs; no key does it on a box.

As built (step 3): a group per PROGRAM (a module on a box), each with its
status tile (RUNNING, FAULTED with the fault, HELD, STOPPED; the scan
period and count only while an engineer is signed in and this browser's
"Scan detail" is on, kept with the Editor's layout), a tile of the PROGRAM's own outputs (`VAR_OUTPUT`s
and located `%Q` variables), then a tile per user function block
instance in declaration order (standard blocks such as TON are not
tiles). A BOOL output is a lamp (TRUE green, FALSE red; card
`monitor-lamp-polarity`), a number its value. "Open diagram" opens the
Editor's diagram at the level shown. Stale data is never a colour: a
program with no new scan for 3 s, or every program while the box is out
of reach, goes grey with "stale" and the age, under a red NO DATA banner
when the box itself is the problem; a stopped simulation is grey too.

Scan load (card `scan-stats`): on a box, "Scan detail" also puts three
lines on each status tile, each with a tooltip on what it means and
what is normal: the scan time (average and peak over the module's last
1000 scans; the tooltip adds the p99 and the shortest), the jitter (how
late each scan started after its deadline, the same way), and the
overruns with how long ago the last one was. An overrun in the last
minute shows on the tile for everyone, in yellow, Scan detail or not.
The simulation runs in the page on no real clock and shows none of
these.
Phones (under 600 points wide), `?monitor` and the old `?andon` open on
it; the header switches either way. Tiles are at least a finger wide,
one column on a phone, and grow with the screen up to 4K.

## Elsewhere

- **Proposals** (agent changes approved with a PIN) sit behind the ⋯ menu
  until they get a home.
- **Agent** (the tab) on a Pi goes through the relay and proposes; in the simulation
  it uses the visitor's key, as the playground does today.
- **Site** (every box on the network) is gone for now; it returns with a
  fleet view.

## Build order (one PR each)

1. Clean-up: remove Lab, Why, Site, Demo and the Andon mode; the always-on
   header; collapsible sidebars; NOT bubbles; the highlighted path.
2. Projects on the Pi: folders, API, Save and Deploy, upload and download
   with import, the engineer gate, forcing.
3. The Monitor.
