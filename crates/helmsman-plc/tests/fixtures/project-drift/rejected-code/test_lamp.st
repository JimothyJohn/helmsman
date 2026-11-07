(* The lamp follows start. *)
PROGRAM test_lamp
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start : BOOL; done : BOOL; END_VAR
VAR n : INT; follows : ASSERT; END_VAR
n := n + 1;
start := TRUE;
IF n = 3 THEN
  follows(COND := lamp);
  done := TRUE;
END_IF;
END_PROGRAM
