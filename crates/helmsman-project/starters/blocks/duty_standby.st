(* Two machines sharing one job. The duty unit runs on demand; the standby
   runs when asked to assist or when the duty unit is not available. Duty
   swaps at every stop, so both wear evenly. *)
FUNCTION_BLOCK duty_standby
VAR_INPUT
  demand : BOOL;
  assist : BOOL;
  a_ok : BOOL;
  b_ok : BOOL;
END_VAR
VAR_OUTPUT
  run_a : BOOL;
  run_b : BOOL;
  b_is_duty : BOOL;
  none_available : BOOL;
END_VAR
VAR
  stopped : F_TRIG;
END_VAR
stopped(CLK := demand);
IF stopped.Q THEN
  b_is_duty := NOT b_is_duty;
END_IF;
IF b_is_duty THEN
  run_b := demand AND b_ok;
  run_a := demand AND a_ok AND (assist OR NOT b_ok);
ELSE
  run_a := demand AND a_ok;
  run_b := demand AND b_ok AND (assist OR NOT a_ok);
END_IF;
none_available := demand AND NOT a_ok AND NOT b_ok;
END_FUNCTION_BLOCK
