# Load a program

On other controllers you *download* a program to the PLC, or *log in* and
*download*, then *go online* to watch it. On a box the same job is one
step, **Deploy**, and the page you are already looking at is the online
view: the diagram shows the program as it runs, with the real inputs and
outputs.

| What you may call it | Here |
| --- | --- |
| Download to the PLC, download to device | **Deploy** (<kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>Enter</kbd>, File › Deploy, or the → arrow beside the program) |
| Log in to the controller | **Sign in** (<kbd>Alt</kbd> <kbd>F8</kbd>, top right) |
| Go online, monitor | The diagram itself, and the **Monitor** screen |
| Upload from the PLC, save the project from the controller | **Download** (File › Download): the project as a `.zip` |
| Online edit, change while running | Edit, then Deploy; a logic change waits for an approval |
| Run mode, program mode | **Run** and **Stop** on the Monitor, for an engineer |

## Step by step

1. **Open the box's page** in a browser, at the box's address. The header
   says which box you are on and what it is running. If it says it is
   simulating in the browser, no box answered at that address: check the
   address and the network first.
2. **Sign in** as an engineer: Sign in in the header, or <kbd>Alt</kbd>
   <kbd>F8</kbd>. A fresh box with no accounts takes the engineer role
   and its PIN; once accounts exist, only accounts sign in. Any sign in
   can watch; loading needs an engineer's.
3. **Open the project** in Files, on the left. Pick one the box already
   holds, or click an example to get a running copy of your own under a
   name you give it, or **Upload** (File › Upload) a `.zip` of a whole
   project or a single `.st` file. See [Projects](projects.html).
4. **Deploy**: <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>Enter</kbd>, File › Deploy, or the → arrow
   right of the file name at the top of Code and beside the running
   program in Files. Deploy saves first, compiles the whole project, and loads it.
   A project that does not compile is refused, with each error by file
   and line, and the box keeps running what it had.
5. **Approve it, if it asks.** A box has an approval policy: a small
   change lands at once; a logic change is held as a proposal until an
   approver signs it off. The header's yellow **waiting** badge counts the
   proposals and opens the list; each one offers a replay in the
   simulation to look at before approving. Nothing runs that was not
   approved under the policy. See [On a box](box.html).
6. **Watch it run.** The diagram is live: inputs and outputs show their
   real state, and the trend along the bottom streams. The Monitor (the
   Editor | Monitor toggle) shows one tile per program with its state.

A deploy keeps every variable whose name and type did not change, so a
counter or a recipe step carries over; the rest start from their initial
values.

## If it did not load

- **Nothing is running after the first install.** That is normal: a new
  box seeds a project and runs nothing until an engineer deploys the
  first program.
- **"Sign in to deploy".** Only an engineer loads a program; an operator
  watches and acknowledges.
- **Errors by file and line.** The program did not compile. Fix them in
  the Structured Text (the editor marks them as you type) and deploy
  again. The Agent tab can fix them for you from a description.
- **A file someone else saved since you opened it** is refused alone and
  offers Reload; the other files land.
- **"stopped: its outputs are at their safe values"** in Messages, right
  after a deploy. The program loaded, but a signal it reads has no value,
  usually because the device that supplies it is down. The **Devices**
  tab shows each link, up or down, and why; once the device answers, the
  program runs on its own.
- **The box does not answer.** The header says so, the last diagram stays
  up and the page keeps trying. Check the box's power and network link.
