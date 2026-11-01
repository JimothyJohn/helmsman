(* PLANT MODEL: not part of the controls. Stands in for the wet well and the
   pumps so the program moves with nothing wired. Inflow alternates between
   dry and storm weather. Delete this file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  pump_a : BOOL;
  pump_b : BOOL;
END_VAR
VAR_OUTPUT
  level_raw : REAL;
  fb_a : BOOL;
  fb_b : BOOL;
END_VAR
VAR CONSTANT
  dry_inflow : REAL := 0.01;      (* m/s of well level *)
  storm_inflow : REAL := 0.06;
  pump_outflow : REAL := 0.03;    (* m/s of well level, each pump *)
  weather_change : TIME := T#2m;
  motor_delay : TIME := T#500ms;
  dt_s : REAL := 0.1;
END_VAR
VAR
  motor_a : TON;
  motor_b : TON;
  clock : TON;
  weather : TON;
  storm : BOOL;
  level_m : REAL := 1.5;
  inflow : REAL;
END_VAR
motor_a(IN := pump_a, PT := motor_delay);
motor_b(IN := pump_b, PT := motor_delay);
fb_a := motor_a.Q;
fb_b := motor_b.Q;

weather(IN := NOT weather.Q, PT := weather_change);
IF weather.Q THEN
  storm := NOT storm;
END_IF;
IF storm THEN
  inflow := storm_inflow;
ELSE
  inflow := dry_inflow;
END_IF;
clock(IN := NOT clock.Q, PT := T#100ms);
IF clock.Q THEN
  level_m := level_m + inflow * dt_s;
  IF fb_a THEN
    level_m := level_m - pump_outflow * dt_s;
  END_IF;
  IF fb_b THEN
    level_m := level_m - pump_outflow * dt_s;
  END_IF;
  level_m := LIMIT(0.0, level_m, 4.0);
END_IF;
level_raw := level_m / 4.0 * 32767.0;
END_FUNCTION_BLOCK
