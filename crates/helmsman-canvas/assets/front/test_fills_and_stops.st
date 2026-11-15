(* The inlet opens when the tank runs low and closes again when it is full. *)
PROGRAM test_fills_and_stops
VAR_INPUT
  fill  : BOOL;
  alarm : BOOL;
END_VAR
VAR_OUTPUT
  done : BOOL;
END_VAR
VAR
  t : TON;
  opened : BOOL;
  closed_again : BOOL;
  inlet_opened_when_low : ASSERT;   (* the inlet opened when the tank ran low *)
  inlet_closed_when_full : ASSERT;  (* the inlet closed when the tank was full *)
  no_alarm : ASSERT;                (* no alarm in a normal fill *)
END_VAR
t(IN := TRUE, PT := T#20s);
IF fill THEN
  opened := TRUE;
END_IF;
IF opened AND NOT fill THEN
  closed_again := TRUE;
END_IF;
IF t.Q THEN
  inlet_opened_when_low(COND := opened);
  inlet_closed_when_full(COND := closed_again);
  no_alarm(COND := NOT alarm);
  done := TRUE;
END_IF;
END_PROGRAM
