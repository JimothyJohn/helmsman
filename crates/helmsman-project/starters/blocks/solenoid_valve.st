(* Single-solenoid valve or cylinder with an end switch each way.
   coil follows the command. fault latches when the switch for the commanded
   position has not made within travel_time, and clears on reset once the
   valve has arrived. Both switches on at once counts as not arrived. *)
FUNCTION_BLOCK solenoid_valve
VAR_INPUT
  open : BOOL;
  at_open : BOOL;
  at_closed : BOOL;
  reset : BOOL;
  travel_time : TIME := T#3s;
END_VAR
VAR_OUTPUT
  coil : BOOL;
  is_open : BOOL;
  is_closed : BOOL;
  fault : BOOL;
END_VAR
VAR
  late : TON;
END_VAR
coil := open;
is_open := at_open AND NOT at_closed;
is_closed := at_closed AND NOT at_open;
late(IN := (open AND NOT is_open) OR (NOT open AND NOT is_closed), PT := travel_time);
IF late.Q THEN
  fault := TRUE;
ELSIF reset THEN
  fault := FALSE;
END_IF;
END_FUNCTION_BLOCK
