(* If the high switch fails, the inlet still closes before the tank overflows, and the alarm comes on. *)
PROGRAM test_high_switch_fails
VAR_INPUT
  fill  : BOOL;
  alarm : BOOL;
END_VAR
VAR_OUTPUT
  level_high_stuck : BOOL;
  done : BOOL;
END_VAR
VAR
  t : TON;
  overflow : TON;
  overflowed : BOOL;
  inlet_closed : ASSERT;      (* the inlet is closed *)
  alarm_on : ASSERT;          (* the alarm is on *)
  never_overflowed : ASSERT;  (* the inlet was never open long enough to overflow the tank *)
END_VAR
(* The high switch never makes. *)
level_high_stuck := TRUE;
t(IN := TRUE, PT := T#20s);
(* From nearly empty, 7.2 s of filling reaches the brim. *)
overflow(IN := fill, PT := T#7s200ms);
IF overflow.Q THEN
  overflowed := TRUE;
END_IF;
IF t.Q THEN
  inlet_closed(COND := NOT fill);
  alarm_on(COND := alarm);
  never_overflowed(COND := NOT overflowed);
  done := TRUE;
END_IF;
END_PROGRAM
