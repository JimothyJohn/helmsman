(* PLANT MODEL: not part of the controls. Simulates a pumped header so the
   program moves with nothing wired. Delete this file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  pump_a : BOOL;
  pump_b : BOOL;
  speed_pct : REAL;
END_VAR
VAR_OUTPUT
  raw : REAL;
  fb_a : BOOL;
  fb_b : BOOL;
  operator_start : BOOL;
END_VAR
VAR CONSTANT
  full_speed_value : REAL := 10.0;  (* what the header reaches at 100 % *)
  settle_per_s : REAL := 0.2;
  motor_delay : TIME := T#500ms;
  dt_s : REAL := 0.1;
END_VAR
VAR
  operator : TP;
  motor_a : TON;
  motor_b : TON;
  clock : TON;
  value : REAL;
  aim : REAL;
END_VAR
operator(IN := TRUE, PT := T#1s);  (* as if someone pressed start *)
operator_start := operator.Q;
motor_a(IN := pump_a, PT := motor_delay);
motor_b(IN := pump_b, PT := motor_delay);
fb_a := motor_a.Q;
fb_b := motor_b.Q;
clock(IN := NOT clock.Q, PT := T#100ms);
IF clock.Q THEN
  IF fb_a OR fb_b THEN
    aim := speed_pct / 100.0 * full_speed_value;
  ELSE
    aim := 0.0;
  END_IF;
  value := value + (aim - value) * settle_per_s * dt_s;
END_IF;
raw := value / 10.0 * 32767.0;
END_FUNCTION_BLOCK
