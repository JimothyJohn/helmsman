(* PLANT MODEL: not part of the controls. Stands in for the duct, coil, damper
   and fan so the program moves with nothing wired. Supply air settles towards
   outside temperature plus what the coil adds, and the filter slowly loads.
   Delete this file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  damper : BOOL;
  fan : BOOL;
  valve_pct : REAL;
  belt_broken : BOOL;
END_VAR
VAR_OUTPUT
  temp_raw : REAL;
  filter_pa : REAL;
  damper_open : BOOL;
  damper_shut : BOOL;
  fan_fb : BOOL;
  airflow : BOOL;
END_VAR
VAR CONSTANT
  outside_c : REAL := 5.0;
  coil_c_per_pct : REAL := 0.35;   (* degrees added at each % of valve *)
  settle_per_s : REAL := 0.05;     (* how fast the duct follows *)
  stroke : TIME := T#3s;           (* damper travel *)
  motor_delay : TIME := T#500ms;
  air_delay : TIME := T#3s;
  filter_rise : REAL := 0.2;       (* Pa/s with the fan on *)
  dt_s : REAL := 0.1;
END_VAR
VAR
  opening : TON;
  closing : TON;
  motor : TON;
  moving : TON;
  clock : TON;
  supply_c : REAL := 5.0;
  aim_c : REAL;
  dirt_pa : REAL := 120.0;
END_VAR
opening(IN := damper, PT := stroke);
closing(IN := NOT damper, PT := stroke);
motor(IN := fan, PT := motor_delay);
moving(IN := motor.Q AND NOT belt_broken, PT := air_delay);
damper_open := opening.Q;
damper_shut := closing.Q;
fan_fb := motor.Q;
airflow := moving.Q;

clock(IN := NOT clock.Q, PT := T#100ms);
IF clock.Q THEN
  IF moving.Q THEN
    aim_c := outside_c + valve_pct * coil_c_per_pct;
    dirt_pa := dirt_pa + filter_rise * dt_s;
  ELSE
    aim_c := outside_c;
  END_IF;
  supply_c := supply_c + (aim_c - supply_c) * settle_per_s * dt_s;
END_IF;
temp_raw := (supply_c + 20.0) / 80.0 * 32767.0;
IF moving.Q THEN
  filter_pa := dirt_pa;
ELSE
  filter_pa := 0.0;
END_IF;
END_FUNCTION_BLOCK
