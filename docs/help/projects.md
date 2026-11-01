# Projects

A project is a folder of files: the program's Structured Text (one or
more `.st` files, one `PROGRAM`), an `io.map` that says which signal is
which channel, `project.settings`, and two the editor keeps for itself,
`diagram.layout` (a saved arrangement) and `icons.map` (the blocks'
icons, see [The Editor](editor.html)). The Files tab of the left pane
shows them.

## Files

- **New project** and **Copy project** are in the File menu; a right-click
  on the pane's blank space offers New project too, and a right-click on a
  project offers Copy, Rename and Delete. A copy of an example or a
  starter is how you begin from one. **Rename** and **Delete** of the
  open project are also buttons under the list.
- **New file**, **Rename file**, **Delete file** likewise. A file is
  Structured Text (`.st`), the I/O map, or one of the editor's own.
- **Right-click a file** for Copy, Paste, Duplicate, Rename and Delete.
  Copy keeps the file in the editor's own clipboard, and Paste puts it
  into whichever project is open, under the next free name (`main-2.st`
  after `main.st`), so moving a block between projects is a copy, an
  open, and a paste. Right-click the files heading to paste without a
  file under the pointer.
- The tree under the files is the program: the `PROGRAM`, then each
  function block instance it calls, nested. Click one to see its diagram.

## Save and Deploy

**Save** (<kbd>Ctrl</kbd> <kbd>S</kbd>, File › Save, the glyph) writes
every file with unsaved edits. A mark in the header shows there are some.
Saving changes nothing about what runs.

**Deploy** (<kbd>Ctrl</kbd> <kbd>D</kbd>, File › Deploy) saves first,
then compiles the project and runs it: in the page in the simulation, on
the box through its approval policy on a box. A project that does not
compile is refused with its errors by file and line.

On a box, a file someone else saved since you opened it is refused alone
and offers Reload; the other files land.

## Upload and download

**Download** (the glyph, File › Download) gives you the project as a
`.zip`. **Upload** takes a `.zip` (a whole project), an `.st` file (into
the open project), a PLCopen `.xml` export, or a ladder export (`.l5x`),
which is converted to Structured Text where the conversion is exact and
refused where it is not, saying why.

## Examples and starters

The simulation ships with short **examples** that teach one thing each,
and with **machines** and **templates**: worked programs for a conveyor
sorter, a pump station, a batch mixer and the like, each with a plant
model so it runs with nothing wired, and two templates to begin a
discrete or a process program from. They are read-only; Save copies one
into a project of your own. On a box, "Start from a machine or template"
in the Files pane asks for a name and sends the copy to the box.

[Examples and starters](starters.html) lists them, generated from the
program itself.

## The I/O map

`io.map` binds the program's inputs and outputs to channels: a line per
signal with its direction, name, type, the device and the channel, and
for an output its safe value. The starters' maps use the box's own
channels and say in a comment to check them before deploying to a box
with anything wired.
