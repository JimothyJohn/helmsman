(* PLANT MODEL: not part of the controls. Stands in for the machine and its
   part sensor so the counters move with nothing wired: a part every two
   seconds while the machine runs, and after every twentieth part a pause
   long enough to show as idle. Delete this file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  running : BOOL;
END_VAR
VAR_OUTPUT
  part_present : BOOL;
END_VAR
VAR CONSTANT
  part_every : TIME := T#2s;
  dwell : TIME := T#300ms;      (* how long a part covers the sensor *)
  batch : INT := 20;            (* parts between pauses *)
  pause_for : TIME := T#15s;    (* the operator loads the next batch *)
END_VAR
VAR
  next_part : TON;
  covering : TP;
  pausing : TP;
  made : INT;
END_VAR
next_part(IN := running AND NOT pausing.Q AND NOT next_part.Q, PT := part_every);
IF next_part.Q THEN
  made := made + 1;
END_IF;
covering(IN := next_part.Q, PT := dwell);
part_present := covering.Q;
pausing(IN := made >= batch, PT := pause_for);
IF made >= batch AND NOT pausing.Q THEN
  made := 0;
END_IF;
END_FUNCTION_BLOCK
