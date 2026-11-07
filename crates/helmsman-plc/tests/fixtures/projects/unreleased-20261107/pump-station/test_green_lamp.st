(* The green lamp is on while the station runs: start_pb turns it on and stop_pb turns it off. *)
PROGRAM test_green_lamp
VAR_INPUT lamp_green : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; stop_pb : BOOL; done : BOOL; END_VAR
VAR
  n : INT;
  on_after_start : ASSERT; (* green after start *)
  off_after_stop : ASSERT; (* dark after stop *)
END_VAR
n := n + 1;
start_pb := n >= 2 AND n < 5;
stop_pb := n >= 10 AND n < 13;
IF n = 8 THEN
  on_after_start(COND := lamp_green);
END_IF;
IF n = 16 THEN
  off_after_stop(COND := NOT lamp_green);
  done := TRUE;
END_IF;
END_PROGRAM
