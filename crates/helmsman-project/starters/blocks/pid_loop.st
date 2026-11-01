(* PID on the process value, calculated every 100 ms, for slow loops such as
   temperature, pressure and level. kp is output per unit of error, ti_s and
   td_s are in seconds (ti_s of 0: no integral). reverse_acting: output rises
   as the process value rises (cooling). While not enabled the output sits at
   out_min, and it starts from there without a bump when enabled. *)
FUNCTION_BLOCK pid_loop
VAR_INPUT
  enable : BOOL;
  pv : REAL;
  sp : REAL;
  kp : REAL := 1.0;
  ti_s : REAL := 30.0;
  td_s : REAL := 0.0;
  out_min : REAL := 0.0;
  out_max : REAL := 100.0;
  reverse_acting : BOOL;
END_VAR
VAR_OUTPUT
  out : REAL;
  error : REAL;
END_VAR
VAR CONSTANT
  sample : TIME := T#100ms;
  dt_s : REAL := 0.1;
END_VAR
VAR
  clock : TON;
  integral : REAL;
  last_pv : REAL;
END_VAR
clock(IN := enable AND NOT clock.Q, PT := sample);
IF reverse_acting THEN
  error := pv - sp;
ELSE
  error := sp - pv;
END_IF;
IF NOT enable THEN
  out := out_min;
  integral := out_min;
  last_pv := pv;
ELSIF clock.Q THEN
  IF ti_s > 0.0 THEN
    integral := LIMIT(out_min, integral + kp * error * dt_s / ti_s, out_max);
  END_IF;
  IF reverse_acting THEN
    out := LIMIT(out_min, kp * error + integral + kp * td_s * (pv - last_pv) / dt_s, out_max);
  ELSE
    out := LIMIT(out_min, kp * error + integral - kp * td_s * (pv - last_pv) / dt_s, out_max);
  END_IF;
  last_pv := pv;
END_IF;
END_FUNCTION_BLOCK
