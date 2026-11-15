(* If the level switches are lost while the tank is filling, the inlet closes. *)
PROGRAM test_switches_lost
VAR_INPUT
  fill : BOOL;
END_VAR
VAR_OUTPUT
  level_low_absent : BOOL;
  done : BOOL;
END_VAR
VAR
  t : TON;
  was_filling : BOOL;
  lost : TON;
  filling_before : ASSERT;  (* the tank was filling when the switches were lost *)
  inlet_closed : ASSERT;    (* the inlet closed *)
END_VAR
t(IN := TRUE, PT := T#4s);
IF fill AND NOT t.Q THEN
  was_filling := TRUE;
END_IF;
(* 4 s in, mid-fill, the module the switches are wired to goes away. *)
level_low_absent := t.Q;
lost(IN := t.Q, PT := T#1s);
IF lost.Q THEN
  filling_before(COND := was_filling);
  inlet_closed(COND := NOT fill);
  done := TRUE;
END_IF;
END_PROGRAM
