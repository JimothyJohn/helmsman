# The simulation

The simulation runs the controller inside the page, scan by scan, against
inputs you set yourself. It is what the playground is, and what a box's
page falls back to when no box answers.

## Running

A project opens running. The glyphs over the diagram run or stop it
(<kbd>Ctrl</kbd> <kbd>Enter</kbd>; <kbd>Shift</kbd> <kbd>F8</kbd> stops),
step one scan while stopped (<kbd>F10</kbd>), and restart from initial
values with the clock at zero. The state in the header says which it is;
the clock beside it is the program's own time.

One program runs in the page at a time. Opening or deploying another
stops the one before.

## Editing

Type in the Structured Text pane. The editor checks what you typed after
a short pause and marks any error on its line; the diagram and the
program running stay as they were. **Deploy** (<kbd>Ctrl</kbd>
<kbd>D</kbd>) compiles your text and runs it, if it compiles. If it does
not, the errors are listed under the editor, a click on one goes there,
and the last good program keeps running.

Undo and Redo (<kbd>Ctrl</kbd> <kbd>Z</kbd>, <kbd>Ctrl</kbd> <kbd>Y</kbd>)
work in the editor and from the Edit menu.

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

The machine examples read a plant model, a block that stands in for the
tank, the belt or the compressors, so that they move with nothing wired.
Nothing moves until you press `start_pb`.

## The trend

The Trend tab along the bottom plots the signals you pin, up to six,
over the last ten seconds, each in its own lane with its last value. Pin
from the tab's list. History restarts when you change the set.

## Faults

A program that divides by zero, runs past its watchdog, or writes an
output out of range **faults**: it stops at the statement that did it,
outputs go to their safe values, the state reads `faulted` in red and
the Messages tab records what happened. Run or Step restarts it.
