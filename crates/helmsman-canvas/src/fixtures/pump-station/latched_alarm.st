(* One alarm: on after on_delay, stays showing until the condition has gone
   and someone has acknowledged it. *)
FUNCTION_BLOCK latched_alarm
VAR_INPUT
  condition : BOOL;
  ack : BOOL;
  on_delay : TIME;
END_VAR
VAR_OUTPUT
  active : BOOL;
  unacked : BOOL;
  showing : BOOL;
END_VAR
VAR
  delay : TON;
  rise : R_TRIG;
END_VAR
delay(IN := condition, PT := on_delay);
active := delay.Q;
rise(CLK := active);
IF rise.Q THEN
  unacked := TRUE;
ELSIF ack THEN
  unacked := FALSE;
END_IF;
showing := active OR unacked;
END_FUNCTION_BLOCK
