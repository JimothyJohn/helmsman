# The Agent

The Agent is the way you program by describing the machine. The page
opens on the **Agent** tab, the first in the left pane: type what the open
Structured Text file should do, and press
**Send**, the yellow paper plane (or <kbd>Enter</kbd>). The file is updated in place and the editor shows it; there is
no conversation to read through.

<img src="img/agent-tab.png" alt="The Agent tab: the instruction box and Send." width="300">

While it works, the tab shows how long the instruction has been running
and the range it takes for a file of that size, from the least to the
usual most (5 to 30 seconds for a short file, a few minutes for a long
one). The time includes any second try, so it is the whole wait. The tab
also shows which step it is at: sending, planning the change, writing
the file, then writing the checks. Once the
Agent has said what the change does, that sentence shows under the step,
and the code shows as it is written, under the words "Draft, checked
before it is written". The Code tab shows the same draft in place
of the file. The file itself changes only once the draft compiles and its
checks pass; Stop, or a draft that fails, leaves it as it was.

A change too big for one answer goes in steps. A long instruction, one
in several parts or one on a long file is planned first, and so is any
change whose answer ran out of room. The tab lists the steps in plain
words and starts on the first at once; Stop drops them and leaves the
file as it was. Each step is checked like any answer, on a copy, and
builds on the steps before it. The file changes once, when every step
has passed, as one version in History. A step that fails its checks
pauses the plan with what went wrong: Try this step again, or Stop to
leave the file as it was.

An instruction can be a new behaviour ("the lamp latches until stop is
pressed"), a change ("add a two-second delay before the belt starts") or
a fix for the errors under the editor. When the Agent is done, the tab
shows one status line and the Agent's reason for the change, for
example:

> Updated lamp.st: 2 scenarios passed, kept as tests.
> Why: the lamp now latches until stop is pressed.

On a box nothing runs until you deploy it, as with any edit of your own.
In the simulation the changed program is loaded stopped, with every
variable, timer and output back at its start, so Run starts it from
scratch. To have it run at once, clear "Stop and reset after a change"
in the Agent tab; the choice is remembered in this browser.

What the change did is marked until your next edit. In the code, lines
it added are on a green ground and a red rule stands where it removed
lines (point at the rule to read them). On the diagram, blocks and wires
it added are outlined green and those it removed are drawn as faint red
ghosts where they were. In the simulation the diagram shows the change
at once; on a box the diagram is what runs, so its marks appear when you
deploy.

## Checked before it is written

With every change the Agent writes one or more scenarios: short
requirement tests, each stating in plain words what it checks, such as
"the lamp stays off while start is not pressed". Before anything is
written, the change is taken into a copy of the project, compiled, and
the scenarios run together with the project's own requirement tests.

If anything fails (a scenario, a requirement test, the compile, or the
whole-file rule below), the Agent is sent what failed and fixes the
answer, up to two more tries. An answer that still fails is never
written: the file stays as it was and the tab says why, one line per
failure. A change that comes without a scenario is asked for one once;
if none comes, it is written and the status line says no scenario checks
it. When the Agent answers without code (it cannot do what was asked, or
it needs one fact from you), the tab shows its words and the file is not
touched.

The scenarios of a change that is written are kept as requirement tests
and appear in the Requirements tab, each under its own words.

When an instruction changes behaviour that an existing requirement test
checks ("the motor runs only while start is held" against a test that
says it latches), the Agent replaces that test with one stating the new
requirement. The replacement has to pass like any scenario, every other
test still has to, and the status line names it:

> Updated main.st: 1 scenario passed, kept as a test; changed requirement
> test_latch: the motor runs while start is held.

The test as it was stays in History, and Undo puts both back.

## History and Undo

Every change the Agent writes is saved as one revision in the project's
History (the Files tab), named after the instruction. Its entry holds the
instruction, the Agent's reason, the scenarios that passed, the answers
that failed and were sent back, and the Agent's words. Edits of your own
you had not saved are saved first, as their own revision, so the version
before the change is always there.

**Undo**, the arrow hooked back beside Send, puts the file back as it was before the last
change (a new revision; the change stays in History). It is offered while
the file is still as the Agent left it; after an edit of your own, go
back through History instead.

In the simulation the arrows beside Send take Undo's place: the left
arrow steps back one revision, the right arrow forward one, as far as
the first revision and the newest. Each revision shown runs at once,
marked green and red with what it changed, and its tooltip names the
instruction behind it. Saving keeps the version shown; an edit of your
own puts the arrows away.

## Follow-ups

Each instruction goes to the Agent with the whole file and the last five
instructions carried out on that file, read from History. So "now make it
blink faster" works: the Agent knows what "it" is, even after a reload.
To start over, describe the whole behaviour in the instruction.

## What the Agent may change

The Agent edits what you have in focus and nothing else: the file open
in the Code tab, by default. When you select lines in the Code tab or
pick a block in the diagram, a line above the instruction box says so:

- **Editing: lines 12–30 of main.st**: the lines you selected.
- **Editing: block Motor1**: the statement of the block you picked.

**Whole file** next to it drops the selection or the pick. A change to
anything outside the scope (another file, lines outside the selection,
anything beside the block) is refused, and the rest of the project stays
exactly as it was.

The code always comes back as the whole open file with the change made
in it, even for a few selected lines or a block. Nothing may be left
out: an answer that skips part of the file, or writes "rest unchanged"
where code should be, is sent back.

## From a spec

To start a project from a written scope of work or product documentation,
open the **Agent** tab with a built-in example open and press
**From a spec…**. A window opens with a box to paste into and
**Add file…** for `.md` and `.txt` files (up to eight). The window lists
each file with its size and a × to take it out, and shows the total
against the most one request takes (120 KiB), because the Agent's answer
and a second try travel in the same request. A file that is empty, is not
UTF-8 text, or holds control characters is refused with the reason and
nothing of it is changed. Press **Build**.

The Agent reads the specification, finds the inputs and outputs, names
them, writes the program and one check for each requirement it can test.
Each check runs against the program on a copy, with the same second tries
as any instruction. What it comes back with is a draft on a review screen:
the project's name, one row for each input and output (its type and what
it is, from the comment on its declaration), one row for each requirement
with **passed** or **not tested yet**, then the requirements the
specification states that have no check, the Agent's reason and
anything that did not pass. Nothing is made until **Accept**; **Discard**
drops the draft.

Accept makes the project as one version in History, called "From a spec"
and the names of the documents. It holds `main.st`, a test for each check
that passed, an empty test for each requirement with no check (shown as
untested, never red), an `io.map` that puts every input and output on
simulated I/O, and `spec.md`, your documents under a heading each. A check
that did not pass is not kept as a test, but its requirement is: it is
listed as **not tested yet** with the empty test, and the reason it
failed is in its tooltip. Deploy runs it as for any project.

### From a spec on a box

On a box, sign in as an engineer and press **From a spec…** in the Agent
tab, with a project open or not; a spec always makes a new project. The
window also takes `.pdf` files. A PDF is kept on the box and the Agent
reads it there, so a scanned manual or a datasheet can be part of the
specification. A box takes a PDF of at most 2 MiB, five PDFs in a
conversation, and about 3.5 MiB in all (a PDF counts a third more than
its size), with room kept for the Agent's answer and a second try. The
window shows each document with its size and the totals against these.
The box says so beside a PDF it does not keep, and then nothing is sent
to the Agent.

The review is the same. Accept sends the whole project to the box in one
request. A name the box already has is refused beside the name field and
nothing is written. A PDF is not kept in the project, because a project
holds text: `spec.md` records it by name, size and SHA-256. History shows
the one version as an upload.

On a box a number input has no signal until you assign it to a channel of
a device, so its line is left out of `io.map` and the review lists it
under **Needs a device**. The project is made and its tests run, but
Deploy is refused until each such input is assigned: open the project and
use **Assign to device I/O** in the I/O tab. A true or false input and
any output start on the box's own channels.

## Design a machine

For a whole machine, describe it and ask for a design before any code.
**Design a machine…** is in the Agent tab, under the spec entry. It is
there when the design entry is served: on a box, for an engineer with a
project open; in the simulation, only when the relay says it serves it
(during the early release it may not). When it is closed for now, or this
hour's allowance is used up, the tab says so beside the button, with the
time to wait, and the chat carries on as before.

A window opens with a box for the description: the stations, the sensors
and actuators, what must happen when something goes wrong. Press **Ask for
a design**. A design takes minutes, and the tab shows the time, how much
has been written and the model's own summary of what it is working out as
it streams. **Stop** drops it and nothing is kept. If the stream stops
before the end, the tab says so and offers **Carry on**, which asks for
that turn again, or **Discard**. A large design takes several turns; they
follow each other by themselves. In the simulation the relay takes at most
five turns and 1 MiB in one go: past that the tab says the design is too
large for the playground, and a smaller machine, or one part of it, fits.

The design comes back with a card: how many parts, requirements and build
steps it holds, its one-sentence summary, and anything that stops the
Agent building from it (a step that names a requirement the design does
not have, a requirement no step covers, a section missing). **Read the
design** opens it one section at a time: Machine, Signals, Parts, Faults,
Requirements, Build order and Open questions. When there is a problem,
Build is not offered; ask again with the machine described more closely.

**Keep** puts the design in the project as `design.md`, as one version in
History. With a built-in example open, the simulation makes a project of
its own named for the machine. On a box the file route writes only the
files a project uses, so Keep makes a copy of the open project, as the box
holds it, with `design.md` in it, in one request; save your edits first.
The Agent tab then shows this project's design and **Build steps 1 to 6**.

Build hands the design's build order to the Agent a few steps at a time.
Each step is asked for with the design's own text: the step, the signal
table, the parts it names and the requirements it must pass. Anything collected with **Chat about this**
goes with every step of the batch, as it does with an instruction. The
steps run on a copy, each checked like any answer, and the file changes once, when
the batch has passed. A step that runs out of room pauses the run and is
not split, because a smaller step would lose the parts and requirements it
names. The next press builds the next batch, until every step is built. A
design of 19 steps is built in four batches.

## In the simulation

In the playground the Agent is free to try during the early release, with
no key and no account. What you type is sent to a hosted model through
our relay, so leave out anything confidential. The relay keeps no
instruction and no answer; it counts requests so that misuse can be
stopped. The free chat runs on a small daily allowance shared by everyone
who tries the playground. When that is used up the tab says so, and it
opens again by itself, usually within a day. The editor, the diagram and
the simulation work without it.

With a built-in example open (they are read-only), the program the Agent
writes becomes a project of its own: the first time, it asks what the
program is called, and later new programs land in the same project as
`main.st`, the one before kept beside it as `v1.st`, `v2.st` and so on.

## On a box

On a box the Agent goes through the box's relay, which holds the key, so
no box and no browser holds it. The tab works the same way: sign in as an
engineer, open a program file, type the instruction and press **Send** or <kbd>Enter</kbd>
(<kbd>Shift</kbd> <kbd>Enter</kbd> or <kbd>Ctrl</kbd> <kbd>Enter</kbd> starts a new line).
The checked change is saved on the box as a revision with its History
entry, by you, and **Undo** asks the box to restore the revision before
it. The dot beside the tab's heading says whether the agent is online
(green), offline (red) or not heard from yet (grey).

If the agent stores a **proposal** for review instead of sending the file
back, the file is not touched and the tab offers **Review** to open it on
the Proposals screen, where an approver signs it off under the box's
policy.

## Get help

On a box the Agent tab has a second mode, for someone who did not write
the program and has a machine that does not do what it should. The
toggle at the top right of the tab switches it: **< >** is **Change the
program**, **?** is **Get help**.

<img src="img/get-help.png" alt="Get help in the Agent tab: the question, Ask for help, and the issue with its Copy button." width="326">

Sign in (any role), say what you saw or what you want to do in your own
words, and press **Ask for help**, the yellow paper plane (<kbd>Enter</kbd> works too;
<kbd>Shift</kbd> <kbd>Enter</kbd> or <kbd>Ctrl</kbd> <kbd>Enter</kbd> starts a
new line). The page sends your words with its recent Messages, and the box
adds what it knows: its version, the program it runs, its devices and
their errors. Passwords, PINs and tokens are left out. Nothing on the box
changes and nothing is filed anywhere.

What comes back depends on what you asked:

- A report of trouble ("the conveyor stopped, the lamp is red") gets
  **What is likely wrong**, the Agent's diagnosis from the box's messages
  and devices, and **Issue to report it**, a Markdown issue that carries
  your words and the box's details. **Copy** takes the whole issue, to
  paste into a new GitHub issue or to give to another agent.
- A question about how things work ("how do I load a program?") gets
  **How to do it**, an answer written from these help pages, with the
  pages it came from linked under **Read more**. No issue is written.

When the agent cannot be reached (the box has no relay, the relay is
offline, or the agent service does not offer help yet), the box writes
the issue by itself from your words and its details, and a line above it
says why there is no diagnosis. For a question it also lists the help
sections whose headings match it, under **These help pages may answer
it**. The picture above shows this case.

**Stop**, the red square shown while the Agent is working, stops waiting for the answer.

## What it will not do

The Agent changes the open file; it never deploys. It cannot force an
input, change the I/O map on a box, or approve anything. Its code is
checked like yours: an error is marked, a deploy that does not compile
is refused.
