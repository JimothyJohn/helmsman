# The simulation

The simulation runs the controller inside the page, scan by scan, against
inputs you set yourself. It is what the playground is, and what a box's
page falls back to when no box answers.

New to it? The [guided tour](https://helmsman.advin.io/playground/?tour)
copies the Blink example, changes it, deploys it and reads its trend,
with each step pointing at the control it is about.

## Running

A project opens running. The glyphs over the diagram run or pause it
(<kbd>Ctrl</kbd> <kbd>Enter</kbd>), stop it (<kbd>Shift</kbd>
<kbd>F8</kbd>), step it (<kbd>F8</kbd>, while paused or stopped), and
restart it from initial values with the clock at zero. The same buttons
and keys are in the Monitor's header. The state beside the glyphs says
which it is; the clock with it is the program's own time.

Pause holds the program where it is: every variable and output keeps its
value and the clock stands until you run or step. Stop ends the run: the
variables go back to their initial values, the outputs go off, the clock
and the trend go to zero, and the diagram shows no values until the
program runs again.

Step runs to the next change. It scans until a value changes or the
program takes another path through its statements, and shows that scan.
A timer's elapsed time counting up does not count as a change, so a step
lands on the scan where the timer's output turns on. If nothing changes in 60 seconds of program time, the step stops
there and the Messages tab says so.

<img src="img/monitor-stopped.png" alt="The Monitor's header with the program paused: run, stop, step and restart beside the crumbs." width="700">

One program runs in the page at a time. Opening or deploying another
stops the one before.

## Breakpoints

Pick a block in the diagram and press <kbd>Shift</kbd> <kbd>F9</kbd> (or
**Edit › Breakpoint**): a red dot marks it. The next scan that reaches
the block stops just before it runs, the simulation stops with it, and
the diagram and every value show the scan as it stood at that moment.
The block is framed in orange and the Messages tab names its line. <kbd>F8</kbd> runs the rest of that scan.
<kbd>Ctrl</kbd> <kbd>Enter</kbd> goes on to the next stop: the same
block's next pass in a loop, another breakpoint, or the next scan. An
input you change while stopped shows at once in what the diagram shows.
A breakpoint inside a function block stops in every instance of it.
<kbd>Shift</kbd> <kbd>F9</kbd> on the block again clears it. Stopping at a
breakpoint changes nothing the program does: the scan runs exactly as it
would have.

## Editing

Type in the Structured Text pane. The editor checks what you typed after
a short pause and marks any error on its line; the diagram and the
program running stay as they were. **Deploy** (<kbd>Ctrl</kbd>
<kbd>D</kbd>) compiles your text and runs it, if it compiles. If it does
not, the errors are listed under the editor, a click on one goes there,
and the last good program keeps running.

Save and Deploy put each file you edited into one layout: two spaces a
level, keywords in capitals, one statement a line, a space round each
operator. Only spacing, line breaks and capitals change, and your
comments stay as you wrote them. A file you did not edit is not
rewritten, and text that cannot be read yet (a stray character, a
comment left open) is left as you typed it.

Undo and Redo (<kbd>Ctrl</kbd> <kbd>Z</kbd>, <kbd>Ctrl</kbd> <kbd>Y</kbd>)
work in the editor and from the Edit menu.

<kbd>Ctrl</kbd> <kbd>F</kbd> opens a find box over the Structured Text.
Every match is marked in yellow, the current one brighter, and the pane
scrolls to it; the box counts them. <kbd>Enter</kbd> or
<kbd>Ctrl</kbd> <kbd>G</kbd> goes to the next match,
<kbd>Shift</kbd> <kbd>Enter</kbd> or <kbd>Ctrl</kbd> <kbd>Shift</kbd>
<kbd>G</kbd> to the previous one, and letters match in either case, as
Structured Text does. <kbd>Esc</kbd> closes the box and leaves the match
selected in the editor. It works the same on a box's code.

## Inputs

Every input of the program has a control: on the diagram, where the
program reads it, or in the I/O panel for the ones the diagram does not
show.

- A BOOL input the program reads once in a while is a **switch**: click
  to flip. It shows TRUE in green, FALSE in red.
- A BOOL input the program reads as a button (`start_pb`, `stop_pb`,
  `ack_pb`) is a **push button**: on while you hold it.
- A number takes what you type; <kbd>Enter</kbd> or leaving the field
  commits it, <kbd>Esc</kbd> puts the old value back. The I/O panel's
  sliders apply as you drag.

With an input's block picked, <kbd>Ctrl</kbd> <kbd>Shift</kbd>
<kbd>F</kbd> flips a switch or opens a number's field, so you can set
inputs without the mouse.

A project can carry a plant model, a block that stands in for the tank,
the belt or the compressors, so that it moves with nothing wired.

## Devices

A project's `plant.sim` file stands in for the machine's devices, so the
program can be checked against something that answers it. One device a
line, the kind, a name, then what it is wired to and its settings:

```text
# The oven: heat_on drives temp_c toward 250 with a minute's lag.
heater oven heat=heat_on temp=temp_c ambient=20 max=250 tau=60s
sensor pt1 of=oven.temp raw=temp_ma range=0..300 signal=4-20mA
motor m1 run=motor_run speed=motor_rpm running=motor_running rated=1450 ramp=3s
valve v1 open=fill_valve opened=valve_open closed=valve_closed travel=2s
tank t1 fill=fill_valve drain=drain_pump level=level_m high=tank_high max=2
eye pe1 belt=belt_run seen=part_seen every=4s length=500ms
button b1 input=start_pb
source s1 value=line_pressure shape=sine from=4 to=6 period=30s
```

- **heater**: the temperature rises toward `max` while the output heats
  (a BOOL, or 0 to 100 %) and falls back to `ambient`; `tau` is the time
  to 63 % of a change.
- **tank**: the level rises at `fill_rate` while filling, falls at
  `drain_rate` while draining, with `high` and `low` switches.
- **motor**: the speed ramps to the command and back to a stop in
  `ramp`; `running` is on above 5 % of `rated`.
- **valve** (or **cylinder**): travels open in `travel`, with `opened`
  and `closed` limit switches; without a `close` output it springs
  closed.
- **eye**: a part blocks it for `length` every `every` of belt time.
- **button**, **switch**, **selector**: set by hand as before; listing
  them gives them the behaviours below.
- **source**: a signal that runs by itself: `ramp`, `exp`, `geometric`,
  `pulse`, `step`, `sine` or a random `walk`.
- **sensor**: a transmitter: what it measures (`of=oven.temp`, or an
  output) scaled from `range` to 4-20 mA, 0-10 V or counts.

Times take `ms`, `s`, `m` or `h`. A line the page cannot read, or one
that names a variable the program does not have, is a warning on
`plant.sim` under the editor; the rest still runs.

### Normal, abnormal and absent

Each input a device drives behaves normally unless a `mode` line says
otherwise:

```text
mode temp_c stuck 80      # frozen at 80 (no value: where it was)
mode temp_c drift 0.5     # 0.5 a second off, and growing
mode temp_c noise 2       # up to 2 either way
mode temp_c over          # past the top of its range (under: below)
mode motor_rpm slow 4     # the motor answers 4 times slower
mode part_seen chatter 50ms   # bounces for 50 ms on each change
mode temp_c absent        # the device is lost
```

An absent input holds its last value, as on a box whose link to the
device is down. A program that declares a BOOL input `NAME_ok`
(`temp_c_ok`) reads it TRUE while the input is there and FALSE while it
is absent, so the program can be written to handle a lost device. Noise,
chatter and the random walk come from `seed N` (1 unless the file says),
so a run is the same run every time.

## The trend

The Trend tab along the bottom plots the signals you pin, up to six,
each in its own lane with its last value. The Window slider sets how
much of the past the pane shows, from 100 ms to 10 minutes on a
logarithmic scale (it opens on 30 s), and the plot follows as you drag;
history is kept for the widest, so widening shows what already ran. The
lane's scale is at its right edge: it rounds out to an even step and
moves only when the values cross one. Pin from the tab's list. Every
input and output is recorded from the first scan, pinned or not, so a
signal you pin later arrives with its past and the lanes already there
carry on. Deploying again keeps the history; a cold restart starts it
over. To put the lanes in another order, drag a lane's name up or down
and drop it on the lane it should take. Click a lane's name to pick that signal's
colour from the palette; the browser keeps your choice. Until you pick
one, a signal named after a colour (`red`, `lamp_green`) is drawn in
that colour, the same as its lamp.

![The Trend tab: three pinned signals, the Window slider, a lane each with its scale at the right edge.](img/trend.png)

## Faults

A program that divides by zero, runs past its watchdog, or writes an
output out of range **faults**: it stops at the statement that did it,
outputs go to their safe values, the state reads `faulted` in red and
the Messages tab records what happened. Run or Step restarts it.
