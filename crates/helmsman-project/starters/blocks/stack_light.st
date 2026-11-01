(* Green while running. Red flashes for an alarm nobody has acknowledged and
   is steady for one that is acknowledged but still there. *)
FUNCTION_BLOCK stack_light
VAR_INPUT
  running : BOOL;
  alarm_new : BOOL;
  alarm_showing : BOOL;
END_VAR
VAR_OUTPUT
  green : BOOL;
  red : BOOL;
END_VAR
VAR
  flash : BLINK_PATTERN;
  pattern : INT;
END_VAR
IF alarm_new THEN
  pattern := 2;
ELSIF alarm_showing THEN
  pattern := 1;
ELSE
  pattern := 0;
END_IF;
flash(EN := TRUE, MODE := pattern, PERIOD := T#1s);
green := running;
red := flash.Q;
END_FUNCTION_BLOCK
