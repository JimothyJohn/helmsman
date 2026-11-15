(* A tank that keeps itself topped up. The process draws it down all the
   time; the inlet valve fills it. *)
PROGRAM tank
VAR_INPUT
  level_low  : BOOL;  (* on when the tank is nearly empty *)
  level_high : BOOL;  (* on when the tank is full *)
  level      : REAL;  (* the level transmitter, in metres *)
  reset      : BOOL;  (* the operator's alarm reset *)
END_VAR
VAR_OUTPUT
  fill      : BOOL;  (* opens the inlet valve *)
  alarm     : BOOL;
  level_pct : REAL;  (* for the operator's screen *)
END_VAR
VAR
  too_long : TON;
END_VAR

(* Nearly empty and full at once cannot be true: a switch or its wiring
   has failed. *)
IF level_low AND level_high THEN
  alarm := TRUE;
END_IF;

(* A fill takes under 6 s. One that runs 7 s means the high switch never
   made, so stop before the tank overflows. *)
too_long(IN := fill, PT := T#7s);
IF too_long.Q THEN
  alarm := TRUE;
END_IF;

IF reset AND NOT (level_low AND level_high) THEN
  alarm := FALSE;
END_IF;

(* The tank is 2 m deep. *)
level_pct := level / 2.0 * 100.0;

IF alarm THEN
  fill := FALSE;
ELSIF level_low THEN
  fill := TRUE;
ELSIF level_high THEN
  fill := FALSE;
END_IF;
END_PROGRAM
