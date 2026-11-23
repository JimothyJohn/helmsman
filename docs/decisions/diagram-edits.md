# Editing the program from its diagram (built 2026-11-15 to 2026-11-18, removed in #516)

Written for an engineer who rebuilds this later. The code is gone from dev;
everything below comes from the PRs, their bodies and the removed code.

Since 2026-11-23 two of these are back in a smaller form, by Nick's
choice: a name and a set value can be changed on the diagram
(`crates/helmsman-canvas/src/by_hand.rs`, docs/UI.md, Principles). A rename
goes through the whole-project rename and a value replaces one literal's
characters; nothing here was brought back, and the logic still cannot be
changed on the diagram.

## Why it was removed

Nick, 2026-11-18: "on the diagram the user edits the layout and toggles
inputs; code changes are typed or asked of the Agent" (docs/UI.md,
Principles). Asked what should happen to the merged edits, he chose "Remove
V1 entirely". The right-click menu stayed, for Chat about this and the
device I/O items (Assign to device I/O…, Back to simulated I/O); none of its
items changes the code. See `crates/helmsman-canvas/src/diagram_menu.rs`.

## What was built, and where it is in history

| PR | Card | Merge commit | What |
|---|---|---|---|
| #441 | diagram-edit-spans | 6f6ec5a2 | `Expr.start`/`end`, `Stmt.end` in helmsman-iec's parser (kept) |
| #445 | diagram-edit-core | 4ea53e28 | `helmsman_diagram::edit::apply`: SetConst, ToggleNegation, SwapOp; `Span` and the maps `pins`, `gates`, `ops` on `Diagram` |
| #448 | diagram-edit-rewire | ec353a29 | Rewire, SetFbArg, DeleteStmt, Rename; `edit/graph.rs`, `edit/more.rs`; keyword fix in `rename_ident` (kept) |
| #454 | diagram-edit-canvas | a63fc2db | `diagram_edit.rs`, gestures in the simulation, deploy after a gesture |
| #505 | diagram-edit-box | 6f633889 | `BoxView::edit_gate` and `diagram_edit`, 6 tests in `box_projects_tests.rs` |
| #512 | diagram-edit-docs | ea605dc2 | help pages, docs/UI.md, docs/ARCHITECTURE.md |

#508 (STRING) and later work touched `edit.rs` and `edit/more.rs` only to
keep them compiling.

The last dev commit with all of it is `4a6d37aa` (merge of #509, the base of
#516's branch). To bring files back:

    git show 4a6d37aa:crates/helmsman-diagram/src/edit.rs
    git checkout 4a6d37aa -- crates/helmsman-diagram/src/edit.rs crates/helmsman-diagram/src/edit crates/helmsman-diagram/tests/edit.rs

The canvas side was renamed to `diagram_menu.rs` and cut down, so take
`crates/helmsman-canvas/src/diagram_edit.rs` and `ui_tests/diagram_edit.rs`
from that commit and merge by hand. `git diff <PR merge>^1 <PR merge>` gives
each PR's own change, which is the quickest map of what touched what.

## The design as it stood

**The engine.** `apply(src, &Module, Edit) -> Result<Patch, Refusal>`, pure
and wasm-safe, depending only on helmsman-iec. `Edit` was SetConst,
ToggleNegation (a pin or an IF/ELSIF/WHILE/REPEAT gate), SwapOp (AND/OR, a
comparison, `+ - * /`), Rewire (a pin takes a variable, literal or earlier
write), SetFbArg, DeleteStmt and Rename. A `Patch` is a byte range and the
new text; `Patch::inverse(src)` undoes it exactly.

**Span maps.** Lowering recorded a byte range per input pin
(`Diagram::pins`, the operand with its `NOT`s and parentheses), per gate
(`gates`) and per binary operator block (`ops`), converted from the parser's
line and column. A read wired from an earlier write kept its own text in the
statement, not the producer's. The fold layer carried the maps through
`View`.

**Write exactly the characters that change.** The source is the truth and
the parse tree drops comments, so an edit splices text: comments, layout and
keyword case survive (`and` becomes `or`). Brackets appear only where the
new precedence would regroup. A variable's name is written as its
declaration spells it.

**Checked before it is returned.** The patch is spliced, the result compiled
and lowered again, and the old and new diagrams compared. The first three
edits could be compared block by block; Rewire, SetFbArg and DeleteStmt
change block counts, so `edit/graph.rs` compared each statement as a tree
with the addressed pin as a hole. A Rename was checked as the same diagram
with both names blanked. Anything else that differed was refused in plain
words (`WouldChangeMore`, `WouldNotCompile`, `StaleDiagram`, and so on).

**One undo step per gesture.** The old text went into the editor's undoer as
its own entry (`editor::push_undo`), so Ctrl+Z took back exactly one gesture
and typing was unaffected.

**Simulation: a gesture deploys.** The playground's diagram shows the
deployed program and typing waits for Deploy, but the engine refuses unless
the diagram is of the text it patches. #454 offered two ways: gestures only
while in sync, giving one gesture per Deploy, or write and deploy at once.
The second was taken, with the Deploy button's checks, unformatted so the
file holds the splice and nothing else. A compile error left the edit in the
code and the program running as it was. The editor scrolled to the patch and
highlighted it for 1.5 s, and the rebuilt viewer kept cameras, scope, pick
and dragged positions (`Viewer::edited_blocks`, `Layers::carry`). Undo and
Redo redeployed only while the diagram was in sync before them.

**Box: the buffer only.** #505 patched the open buffer as typing would and
nothing else: no save, no deploy, no new route, nothing in helmsman-plc. Save
and Deploy stayed the engineer's, so the approval tier was set from the diff
exactly as for typed text. The existing policy decided the
tier, so the feature added no route of its own.

**The edit gate.** Gestures were on only while the diagram was the code in
the editor. In the simulation they were off when the code was typed on since
the last Deploy, when it did not compile, and for a project of several
program files. On a box they were off for anyone below engineer, when the
open project was not the running one, when the file changed since it was
deployed (so after one gesture until it ran or was undone), and for several
program files. The common reason: byte offsets are only valid against the
exact text the diagram was drawn from, and the engine needs one file to
splice into.

## What was learned the hard way

- `rename_ident` accepted `IF` as a name (#448, test
  `refuses_a_reserved_word_as_a_name`, written first and seen to fail). Kept
  in dev; `is_reserved_word` also covers TRUE and FALSE.
- A call used as an IF condition appears as a root of two statements. The
  tree check in `edit/graph.rs` found it through the new property test.
- Spans: `Expr.pos` is the operator for a binary, the `.` for a member and the
  `(` for a call, so #441 added `Expr.start` rather than changing `pos`. A
  parenthesised expression owns its parentheses.
- A diagram edit changes the text of the blocks it touches, and a block is
  named by its text. Without carrying the camera, the pick and the dragged
  positions across the rebuild, every gesture threw the view away.
  `edited_blocks` paired blocks by place when the shape was unchanged and by
  `diff::modules` otherwise.
- Hit-testing on folds: a fold has no block of its own, so a gesture on it is
  about a block inside it. A comparison folded with its literal was edited as
  the comparison and its literal; a fold's pin maps to the one block its wire
  enters (`base_pin`); a table's pin feeds several rows and was refused. The
  pin-reach test also had to stop a press on a pin starting a selection band.
- The menu was dry-run against the engine when it opened, so it never listed
  a change that would be refused. A change refused anyway was shown in the
  popup with the engine's words and the file left byte-identical.

## What was never solved

- `SetFbArg` and `Rewire` to a write had no gesture; the menu offered only
  Change operator, Negate input, Connect to… (variables), Edit value…,
  Rename… and Delete statement. Region bubbles (negating an IF gate) had none
  either.
- A long press on touch was only egui's secondary click; there was no
  touch-specific handling and no test on a touch screen.
- VAR_IN_OUT calls were never tried: the edits predate in-outs, and the
  lowering draws an in-out argument as a read only (card `diagram-in-out`).
- On a box, one gesture at a time, and only for one-file projects.
- Multi-file projects were off in both hosts.

## What V2 and V3 were going to be

From the removed card `diagram-edit-v2-v3` (est. V2 3 to 4 days, V3 about 3):
V2 was a palette to add a block, a new assignment or a new FB instance, which
also edits the VAR section. V3 was regions: wrap statements in an IF, edit a
condition, add ELSIF or ELSE, add CASE arms. Neither was started.

Two answers from Nick's interview of 2026-11-18 (in chat, not in any file).
Asked where a block added on the diagram should go in the program, he
chose "Where the wiring needs it" (the earliest place where every input is
already computed) over "after the clicked statement" and "end of the
program". Asked how blocks would be added once the Palette tab was gone
(#423), he answered that the user should only edit the layout and toggle the
inputs, which led to this removal.
