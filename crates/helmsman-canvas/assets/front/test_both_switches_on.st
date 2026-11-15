(* Nearly empty and full at once cannot be true: the inlet stays closed and the alarm comes on. *)
PROGRAM test_both_switches_on
VAR_INPUT
  fill  : BOOL;
  alarm : BOOL;
END_VAR
VAR_OUTPUT
  level_low  : BOOL;
  level_high : BOOL;
  done : BOOL;
END_VAR
VAR
  t : TON;
  inlet_closed : ASSERT;  (* the inlet is closed *)
  alarm_on : ASSERT;      (* the alarm is on *)
END_VAR
level_low := TRUE;
level_high := TRUE;
t(IN := TRUE, PT := T#1s);
IF t.Q THEN
  inlet_closed(COND := NOT fill);
  alarm_on(COND := alarm);
  done := TRUE;
END_IF;
END_PROGRAM
