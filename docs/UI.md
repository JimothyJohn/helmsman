# The Helmsman UI

What the browser app is, agreed with Nick on 2026-10-31 (interview). Build to
this; change it here first when a decision changes.

## Principles

- **Structured Text and its live diagram, nothing else.** The diagram is a
  view of the ST. On it the user edits the layout and toggles inputs; code
  changes are typed in the editor or asked of the Agent (Nick, 2026-11-18:
  "on the diagram the user edits the layout and toggles inputs; code
  changes are typed or asked of the Agent"). The diagram's right-click menu
  holds Chat about this and the io.map items (Assign to device I/O…, Back
  to simulated I/O), none of which changes the code. What was built and why it
  went: docs/decisions/diagram-edits.md. (The node-graph Lab
  editor is gone.) **Names and set values are the exception** (Nick,
  2026-11-23: "I do want to be able to change set values, labels, and names
  on the visual panel, but I don't want to manipulate the logic"): Rename…
  on a block that names a variable and on an I/O row, Set value… on a
  constant or a timer's preset, a double click for either, and the code
  follows. Each is saved as one revision with a History entry, as a
  described change is, and Undo takes it back. The label is the variable's
  name; a label of its own is a later layer (TODO.md `label-layer`).
- **One app wherever it runs.** Served by a Pi, it talks to that Pi. Opened
  from the public site (docs/playground), it runs a simulated box in the
  page. Same screens, same buttons; the source is a setting, not a mode.
- **Runs in every engine, with or without a GPU.** Chromium (Chrome, Edge),
  Firefox and WebKit (Safari on a Mac or iPad) each need WebGL2. `make
  canvas-smoke BROWSER=all` and `NOGPU=1` drive all three headless, with
  hardware drawing and in software, on every PR (the `canvas-browsers` job).
  A browser with no WebGL2 shows a notice (loading/loading.js) in place of
  the app.
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
  Monitor, Structured Text, Files, Devices, the right pane, Tab to the right, the Agent's tabs, Trend,
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
- **One pane of tabs, on the left, the Agent first** (Nick, 2026-11-13:
  two side panels by default was too many; one keeps the diagram wide and
  the focus where it is most of the time). All six tabs start there:
  Agent (shown), Code, I/O, Devices, Files, History. A tab's glyph
  dragged to the window's right edge (96 pt, lit while a tab is dragged)
  docks it in a right pane (236 pt) that exists only while it holds a
  tab; dragged back onto the left pane it returns. Alt Shift 5 sends the
  left pane's tab to the right and shows the one under it on the left.
  The tabs shrink to share one row with the chevron (24 pt at the least,
  then they wrap). Kept per browser (`dock=2` in the layout text; a
  layout from before is read with its panes reset).
- **Tabs reorder, hide, and the arrangement saves by name** (Nick,
  2026-11-15; `pane_layout.rs`). A tab's glyph dragged along a row of
  tabs takes the place marked there (in its own pane a reorder, in the
  other pane a dock at that place). A tab's right-click menu hides it
  (`hidden=` in the layout text); its View item, or anything that opens
  the tab, brings it back at the end of the left pane. View › Pane
  layouts… lists four ready-made layouts (Default, Programming,
  Commissioning, Monitoring) and the browser's own (at most 32, names up
  to 40 characters, `helmsman-canvas/pane-layouts`): each is where every
  tab sits, which tab each side shows, which panes are open, the bottom
  panel's tab, and the three pane sizes. Light mode, word wrap, trend
  colours and the diagram's arrangement are not part of one. Export
  downloads the browser's own as `helmsman-pane-layouts.json`
  (`helmsman-pane-layouts 1`); Import reads such a file as hostile (64 KB
  at most, unknown tabs dropped, sizes held to 120..1600 pt, a name
  already kept replaced, a ready-made layout's name taken in as "NAME
  (imported)"). Names match without regard to the case of ASCII letters.
  On a box's page the popup has a third list, On this box: the layouts
  the box keeps (`GET /api/pane-layouts`, read when the popup opens and
  after each change, and read as a file is), so any browser that opens
  the box offers them. An engineer puts one of their own there with To
  box and deletes one with Delete; for anyone else both are greyed and
  say why.
- **Panes collapse** from their chevron, their Alt key, or a double click
  on any of their tabs (Nick, 2026-11-07), on either side.
- **The tabs (left pane 320 pt):** Files (a heading naming where the
  projects are kept, "Files in this browser" or "Files on the box", then
  the projects in two sections that fold with a click and are
  remembered, My Files and Examples, none on a box; the
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
  Add: a small form asks one thing, the device's name, and says how many
  channels it has; Add copies the profile into the project as
  `NAME.device` (the box fetches and checks it, `GET
  /api/devices/library`; the simulation carries its E1213's) and writes
  `device NAME device:PROFILE IDENTITY` with the identity the scan saw.
  No signal is written and no protocol, address, byte order or cycle
  time is asked. **A device's I/O is added one point at a time**
  (`device_io.rs`, Nick, 2026-11-15: every channel is available, and
  none shows in the project until it is added). Under a device's name a
  row offers **+ Add I/O** with the count of channels left, known from
  the profile the project holds (or the form's catalog for an E1213 on a
  typed protocol). The picker lists them as Inputs and Outputs, narrowed
  by a search field (every word in the channel's name, direction or
  type); a click writes one `ch:` signal after the device's lines, named
  `NAME_channel`, and declares that variable in the program, and the
  picker stays open. "For" gives the next channel to a program variable
  that is on no device or on the box's own channels (its old line goes).
  A device nobody knows the channels of, and any device under Advanced,
  takes a point by its address. A point's right-click has Remove… (asks;
  the variable stays). A device with signals the program has no variable
  for (every channel, as Add wrote them until 2026-11-15) offers Remove N
  unused, which asks first. The New device and Edit forms hold settings
  only: Edit is the name alone unless Advanced is on, and Save writes
  the device's signals back as they were. The agent's listing in the
  playground names the channels not added (`device_io::describe_free`);
  the box's `list_devices` does not yet. A device the library does
  not know says so and offers Advanced, and, on a box with an agent, a
  button **Build from vendor file…** (`device_vendor.rs`, Nick,
  2026-11-15). It picks the vendor's file (an EDS, an IODD, a PDF manual
  or a text file: text up to 1 MB, a PDF up to 2 MB), posts it to the box
  as a drop-in (`POST /api/dropins`, the file name cut to `A-Z a-z 0-9 . _ -`
  and 100 characters, read as prose or as a PDF) and, once the box has
  answered 201 with the drop-in's id, sends the agent one chat message
  asking for a device profile with `dropins: [id]` (`POST /api/chat`, a
  new conversation). The message names the drop-in by its id and kind and
  the device by its numbers and address, never by the file's name or the
  name the device sent. Every refusal is said in words beside the button
  (too large, wrong type, not UTF-8, 401, 403, no answer) and says that
  nothing was sent to the agent; the button is offered to an engineer or
  integrator on a box whose agent can be asked, and everyone else reads
  why it is not. While the agent works the note says so, and when its turn
  ends the note carries what it said or why it failed. The profile it
  proposes appears in the section below. After that profile is saved, the
  found device's row offers **Add** with it (the project's own file, its
  channels listed in the same form as a library device's); the match is
  the device the file was sent for, kept for the page's session.
  **A profile the agent proposed**
  (`device_proposal.rs`): while the box holds one (`GET
  /api/device-profile/proposal`, read every 3 s while someone is signed in
  and the tab is open), a section at the top of the tab shows the device's
  maker and model, its name, Save profile and Discard, the agent's summary
  and the channels (name, direction, type, unit, safe value; each point on
  each protocol in the hover). The device's name and every channel's name
  are edited in place. Save sends the id it read, the open project, the
  name when changed and the renames (`POST .../accept`); the box reads the
  renamed text again and writes `NAME.device` as a new file, never over
  one that exists. The tab then reads the project again and lists the
  saved file's channels as the project holds them. A refusal is shown
  where it belongs: a problem at a line beside the channel on that line, a
  bad or taken name beside the name, the rest above the buttons; a newer
  proposal (409, or a different id on a read) replaces the one shown and
  keeps the names typed where the channel remains. Only an engineer is
  offered Save and Discard. **Device configuration**
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
- **More tabs:** I/O
  (values; inputs the diagram does not show) and Agent. A Palette tab of
  the glyph library's pieces to drag onto the diagram was there from
  2026-11-12 to 2026-11-14 (Nick: it goes against the agent-first
  design; it may come back for fine tuning or a hint to the Agent). (The Requirements
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
  and the Agent's one-sentence reason. In the simulation the answer area
  is on top and the box under it at the tab's bottom edge, one row
  growing upward to four and scrolling inside past that, eased over
  0.1 s, so the output does not move (Nick, 2026-11-23; from 2026-11-13
  to then the box was on top and took the tab's height). The output can
  be selected. The arrows restore the prompt and the output of the step
  shown and log nothing; the clock beside them turns the answer area into
  the searchable list of versions. No "Stop and reset" checkbox: a
  described change runs at once in the simulation unless View › Reset
  after a described change is on. Shift
  or Ctrl Enter goes on with a list (`- `, `* `, `1. `, ending it on an
  empty item) and Tab indents; while the Agent works the box shows the
  instruction sent, greyed. On a new project (a built-in example
  open) "From a spec…" opens a window (Nick, 2026-11-15, `crate::spec_mode`):
  paste a scope of work or add `.md` and `.txt` files, with the total
  against the one request's cap, and Build; the draft comes back on a
  review screen (the name, the I/O, each requirement passed or not, the
  untested ones, the reason) and nothing is made until Accept. On a box
  (Nick, 2026-11-15; `crate::box_spec`) the Describe tab offers it to an
  engineer with a project open or not, the window also takes `.pdf`
  files with the box's caps beside the total, the review lists the
  inputs that need a device, and Accept sends the project to the box as
  one zip. "Design a machine…" (`crate::design_step`, `crate::design_ui`;
  card `design-step-canvas`) asks a strong model for a whole machine's
  control system design: a window takes the description, the tab shows
  the elapsed time and the model's thinking summary while the turns come
  (minutes; Stop keeps nothing, a stream that stops early offers Carry
  on), then a card (parts, requirements, build steps, what stops the
  Agent building from it) and a window that reads the design a section at
  a time. Keep puts it in the project as `design.md`; the tab then offers
  Build steps 1 to 6 of 19, the next six on the next press, each step
  asked with the design's own brief. On a box it posts to `/api/design`;
  in the simulation it asks the relay's design entry and shows the
  entry only while that says it is open (and says when it is closed or
  the hour's allowance is used). A change that lands
  runs at once and is seen on the canvas, marked, with nothing written in
  the tab (its reason goes to the Messages). One that does not is an
  error in the Messages (their red count, and they open), the box is
  outlined red, and one red line opens the Messages. Undo is Before /
  After there: the file swaps at once between the version before the
  change and the change, compiled and running, each marked green and red
  against the other; nothing is saved (Save keeps the version shown), and
  an edit ends the swap. On a box the tab keeps Undo and the three-row
  box: a box's change goes through a deploy. Send updates the open file: one
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
- **Selecting blocks (Nick, 2026-11-16).** Drag on empty space to draw a
  box; the blocks it touches are selected (Shift or Ctrl/Cmd adds). Shift
  or Ctrl/Cmd and a click toggles one block. A plain click on a block
  selects that block alone; a click on empty space, a click anywhere off
  the diagram (the Describe tab aside, where the selection is used) or Esc
  deselects (Nick, 2026-11-17). Only a released click changes the
  selection: while the button is down the highlights stay, and a box
  being drawn shows what it touches. The proposal review's diagram takes
  the same gestures. The selected blocks are outlined blue and are the
  Agent's focus: they show as chips after the collected ones (an x
  deselects), go with the next instruction, and say how many more are
  selected than the eight an instruction carries. Dragging empty space no
  longer pans the diagram: Space and a drag, or the middle button, do; the
  arrow keys and W A S D too, and the wheel zooms. Hovering empty space
  says so.
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
- **I/O as cards (2026-11-16, `io_cards.rs`):** every input and output
  in the I/O tab is one card, not a fold on the diagram (an output's logic
  is spread over statements that are not next to each other). Nick,
  2026-11-16: "You just need to know their name, what triggers them in the
  real-world, and what the influence in the control scheme." So a card has
  the point, one line for what is wired to it, and one line for the first
  thing that drives it or that it affects, with "…" when there are more.
  A double-click opens the rest in place. The `< >` glyph finds the point
  in the code and steps through its uses; the diagram glyph lights and
  frames its blocks, running or not. A tap on an output lights the path
  that sets it, framed as the Q/E jump frames, no closer than the current
  zoom. List or tiles at the top; Inputs and Outputs fold behind their
  arrows. The diagram stays in source order.
- **Assign to device I/O (2026-11-15, `assign.rs`, `assign_ui.rs`):** a
  program's inputs and outputs start on simulated or on-screen I/O
  (`in start bool box di:0`, `out speed int screen speed safe=0`). A device
  is added alone, and its channels join the project one at a time under
  + Add I/O (`device_io.rs`); the box binds a program port to the map signal
  of the same name. So assigning a port to a channel is a change of io.map
  lines, and every other line stays as typed. A channel with no line yet
  (one the device has and + Add I/O still lists) gets its line from the
  same code the picker's *For* uses (`device_io::add`: the options of that
  channel, its place after the device's other lines), named for the port,
  and the port's simulated line is removed. A channel that has a line no
  port is named after (a point added earlier under a generated name) is
  renamed to the port's name, keeping its type, point, options and comment.
  Three places, in the simulation and on a box alike. (1) The I/O tab: a
  right-click on a program input's or output's row offers *Assign to device
  I/O…*, which lists the free channels of that direction and type, those
  with a line and those without. (2) The diagram: a right-click on the Read
  block of an input or the Write block of an output offers the same item,
  whatever the diagram's edit gate says: it changes io.map and not the
  code, so on a box, or while the code is ahead of the diagram, the menu
  holds only it and says why the code's own items are off. On an input's
  block the click reaches the menu through the switch (or the box's force
  button) drawn over it. (3) The Devices tab: a right-click on a point
  already in the project offers *Assign to program input…* or *…output…*,
  which lists the unassigned ports (no map line, or one on the box or the
  screen) of its direction and type. A channel without a line is not on
  that tab, so there the picker's *For* is the way and this item is for the
  point that is. A port that is on a device offers *Back to simulated I/O*
  (also on the point that feeds it): as Remove does, the channel's line goes
  out of io.map and the channel is under + Add I/O again, and the port gets a
  box line as a new project's map writes it at the end of the file (a bool
  the box's next free `di:N` or `do:N`, an output of another type a `screen`
  line). An int or real input gets no line: a box accepts none for a
  simulated one, and the simulation shows its control. An item that cannot
  work is shown off with the reason in its tooltip: no free channel (add a
  device in the Devices tab first), the port or channel already assigned, a
  read-only example, or no engineer signed in on a box. A refusal that is
  reached anyway is a sentence in the notice and changes nothing. In the
  simulation the new text goes into the project's io.map and the project is
  unsaved; on a box into the open project's io.map buffer, which goes to the
  box with Save or Deploy. An I/O tab row's tooltip says where the point is:
  "On rio, DI-04" or "Simulated: not assigned to a device yet".
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
  character instead.
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
| Alt Shift 0 | The Files tab | Devices view (Alt 1 to 9 are Firefox's tabs on Windows and Linux) |
| Alt Shift 1 | The Code tab (the Structured Text) | POUs view |
| Alt Shift 2 | Trend along the bottom | Modules view |
| Alt Shift 3 | The right pane, once a tab is docked there | Messages view |
| Alt Shift 4 | The Devices tab | |
| Alt Shift 5 | The left pane's tab to the right pane, the next one shown on the left | |
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
| E, Q | The next or the previous function block call, IF, CASE or loop on the diagram, zoomed in and framed (pointer over the diagram, nothing typed in) | (keys of our own, beside W A S D) |

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
