# Code checks

The editor reads your Structured Text for patterns that compile and still
cause trouble on a real machine. Each finding is listed under the editor
as a warning, with a stable ID, the file and the line, and it is sent to
Helmsman with the compile errors when you ask it for code. The IDs are
the same on the page, on a box and in the audit trail, so you can look
one up.

Two of the checks hold the code. A finding of either one stops **Use this
code** and **Deploy** until you fix it or override it on its line with a
reason. The others are advice: they never stop anything.

## The checks that hold the code

**HELMSMAN-L1: a function block called inside a condition.** A timer,
counter or edge detector that is called only in an IF or CASE branch does
not run on the scans where the branch is not taken. The timer stops
counting, the edge is missed, the counter misses its input. Call the
block on every scan and put the condition on its input (`IN := ...`). A
call inside a branch is allowed when every branch of that IF or CASE
calls the same block. A test's `ASSERT` is not a function block with
state and is never flagged. A timer in a state machine's CASE arm is
flagged on purpose: it stops with the state, so say in a reason why that
is what you want.

**PLCOPEN-CP12: an output written in more than one place.** A
`VAR_OUTPUT` of the program, a variable located at `%Q` or a `%Q` address
that is written twice in one scan, in sequence or once inside a branch and
once outside it. The two arms of one IF or CASE count as one place. Members
of a structure and different array elements are different outputs. The
rule is in the PLCopen Coding Guidelines, rule CP12: compute the output in
one place at the end of the scan.

## The advice

| ID | What it says |
|----|--------------|
| PLCOPEN-CP3 | A local variable is read and nothing gives it a value. A counter that is read before its first assignment and written later is fine. |
| PLCOPEN-CP8 | `=` or `<>` on a REAL or LREAL. A comparison with a literal zero is left alone. |
| PLCOPEN-L12 | A FOR variable, or something in its start, end or step, is changed inside the loop. |
| PLCOPEN-CP24 | A variable is declared and nothing uses it. Inputs, outputs and located variables are left alone. |
| PLCOPEN-CP2 | Code that can never run: after RETURN, EXIT or CONTINUE, or in a branch whose condition is the literal FALSE. |
| HELMSMAN-L2 | A lint pragma that overrides nothing (see below). |

## Overriding a finding

Put a pragma on the line the finding names, with the ID and a reason in
quotes:

```
t(IN := go, PT := T#2s); {lint: off HELMSMAN-L1 "the branch is the enable"}
```

The finding stays listed, marked overridden with your reason, and it no
longer holds the code. When you deploy to a box, the audit trail records
the ID, the file and line and your reason. A reason is required, one line
of plain text of at most 200 characters. A pragma with no reason, an ID
that does not exist, a finding that cannot be overridden or no such
finding on its line overrides nothing and is itself reported as
HELMSMAN-L2.

## Where the findings show

- In the editor, in the list under the code, and in the Log.
  A finding that holds the code says so on its row.
- In the answer: a finding the answer adds is sent back to Helmsman for another try. A finding the project already had is not blamed
  on the answer.
- On a box, a deploy that holds a finding is refused before anything
  changes, and the answer lists the finding with its line.

Recursion is not in this list: the compiler refuses it as an error.
