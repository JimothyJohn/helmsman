(* PLANT MODEL: not part of the controls. Stands in for the tank, valves and
   heater so the program moves with nothing wired. Delete this file when you
   wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  inlet : BOOL;
  outlet : BOOL;
  agitator : BOOL;
  heater : BOOL;
  inlet_stuck : BOOL;
END_VAR
VAR_OUTPUT
  level_raw : REAL;
  temp_raw : REAL;
  inlet_open : BOOL;
  inlet_closed : BOOL;
  outlet_open : BOOL;
  outlet_closed : BOOL;
  agitator_fb : BOOL;
END_VAR
VAR CONSTANT
  fill_m_per_s : REAL := 0.1;
  drain_m_per_s : REAL := 0.15;
  heat_c_per_s : REAL := 1.5;
  cool_c_per_s : REAL := 0.1;
  ambient_c : REAL := 20.0;
  stroke : TIME := T#500ms;       (* valve travel *)
  motor_delay : TIME := T#200ms;  (* contactor pull-in *)
  dt_s : REAL := 0.1;
END_VAR
VAR
  clock : TON;
  level_m : REAL;
  temp_c : REAL := 20.0;
  in_open : TON;
  in_shut : TON;
  out_open : TON;
  out_shut : TON;
  motor : TON;
END_VAR
in_open(IN := inlet AND NOT inlet_stuck, PT := stroke);
in_shut(IN := NOT inlet, PT := stroke);
out_open(IN := outlet, PT := stroke);
out_shut(IN := NOT outlet, PT := stroke);
motor(IN := agitator, PT := motor_delay);
inlet_open := in_open.Q;
inlet_closed := in_shut.Q;
outlet_open := out_open.Q;
outlet_closed := out_shut.Q;
agitator_fb := motor.Q;

clock(IN := NOT clock.Q, PT := T#100ms);
IF clock.Q THEN
  IF in_open.Q THEN
    level_m := level_m + fill_m_per_s * dt_s;
  END_IF;
  IF out_open.Q THEN
    level_m := MAX(0.0, level_m - drain_m_per_s * dt_s);
  END_IF;
  IF heater THEN
    temp_c := temp_c + heat_c_per_s * dt_s;
  ELSE
    temp_c := MAX(ambient_c, temp_c - cool_c_per_s * dt_s);
  END_IF;
END_IF;
level_raw := level_m / 3.0 * 32767.0;
temp_raw := temp_c / 120.0 * 32767.0;
END_FUNCTION_BLOCK
