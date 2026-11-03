(* Loop check for a Moxa ioLogik E1213 wired back on itself: DO-00 to DI-00,
   DO-01 to DI-01, DO-02 to DI-02 (docs/devices/moxa-iologik-e1213.md).
   One output at a time is held on for step_time, then the three inputs
   are compared with the three outputs: a match is a pass, anything else a
   fault. cycles counts complete passes over the three loops. Force an
   input in the diagram to see a fault counted. *)
PROGRAM e1213_loop
VAR_INPUT
  di0 : BOOL;  (* DI-00, wired from DO-00 *)
  di1 : BOOL;  (* DI-01, wired from DO-01 *)
  di2 : BOOL;  (* DI-02, wired from DO-02 *)
END_VAR
VAR_OUTPUT
  do0 : BOOL;
  do1 : BOOL;
  do2 : BOOL;
  step : INT;    (* which output is on: 0, 1 or 2 *)
  cycles : INT;  (* passes over all three loops *)
  faults : INT;  (* checks where an input did not follow its output *)
  ok : BOOL;     (* the last check matched *)
END_VAR
VAR
  step_time : TIME := T#500ms;  (* longer than the E1213's 100 ms input filter *)
  tick : TON;
END_VAR
(* A timer that restarts itself: Q is TRUE for one scan every step_time. *)
tick(IN := NOT tick.Q, PT := step_time);
IF tick.Q THEN
  ok := (di0 = do0) AND (di1 = do1) AND (di2 = do2);
  IF ok THEN
    IF step = 2 THEN
      cycles := cycles + 1;
    END_IF;
  ELSE
    faults := faults + 1;
  END_IF;
  step := step + 1;
  IF step > 2 THEN
    step := 0;
  END_IF;
END_IF;
do0 := step = 0;
do1 := step = 1;
do2 := step = 2;
END_PROGRAM
