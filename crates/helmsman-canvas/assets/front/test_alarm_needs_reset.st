(* An alarm stays on, and the inlet closed, until someone presses reset. *)
PROGRAM test_alarm_needs_reset
VAR_INPUT
  fill  : BOOL;
  alarm : BOOL;
END_VAR
VAR_OUTPUT
  level_high_stuck : BOOL;
  reset : BOOL;
  done : BOOL;
END_VAR
VAR
  seen : BOOL;
  after : TON;
  pressed : TON;
  still_on : ASSERT;      (* the alarm stayed on after the fault went away *)
  still_closed : ASSERT;  (* the inlet stayed closed *)
  cleared : ASSERT;       (* reset cleared the alarm *)
END_VAR
(* The high switch fails until the alarm comes on, then works again. *)
IF alarm THEN
  seen := TRUE;
END_IF;
level_high_stuck := NOT seen;
after(IN := seen, PT := T#3s);
IF after.Q AND NOT reset THEN
  still_on(COND := alarm);
  still_closed(COND := NOT fill);
  reset := TRUE;
END_IF;
pressed(IN := reset, PT := T#500ms);
IF pressed.Q THEN
  cleared(COND := NOT alarm);
  done := TRUE;
END_IF;
END_PROGRAM
