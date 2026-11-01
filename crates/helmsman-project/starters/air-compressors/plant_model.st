(* PLANT MODEL: not part of the controls. Stands in for the air receiver and the
   compressors so the program moves with nothing wired. Air use swings between
   light and heavy. Delete this file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  comp_1 : BOOL;
  comp_2 : BOOL;
  comp_3 : BOOL;
END_VAR
VAR_OUTPUT
  header_raw : REAL;
  fb_1 : BOOL;
  fb_2 : BOOL;
  fb_3 : BOOL;
END_VAR
VAR CONSTANT
  each_makes : REAL := 0.01;    (* bar/s from one compressor *)
  light_use : REAL := 0.005;    (* bar/s the plant draws *)
  heavy_use : REAL := 0.025;
  swing : TIME := T#2m;         (* time between light and heavy use *)
  motor_delay : TIME := T#500ms;
  dt_s : REAL := 0.1;
END_VAR
VAR
  motor_1 : TON;
  motor_2 : TON;
  motor_3 : TON;
  clock : TON;
  shift : TON;
  heavy : BOOL := TRUE;
  bar : REAL := 7.0;
  use : REAL;
END_VAR
motor_1(IN := comp_1, PT := motor_delay);
motor_2(IN := comp_2, PT := motor_delay);
motor_3(IN := comp_3, PT := motor_delay);
fb_1 := motor_1.Q;
fb_2 := motor_2.Q;
fb_3 := motor_3.Q;

shift(IN := NOT shift.Q, PT := swing);
IF shift.Q THEN
  heavy := NOT heavy;
END_IF;
IF heavy THEN
  use := heavy_use;
ELSE
  use := light_use;
END_IF;
clock(IN := NOT clock.Q, PT := T#100ms);
IF clock.Q THEN
  bar := bar - use * dt_s;
  IF fb_1 THEN bar := bar + each_makes * dt_s; END_IF;
  IF fb_2 THEN bar := bar + each_makes * dt_s; END_IF;
  IF fb_3 THEN bar := bar + each_makes * dt_s; END_IF;
  bar := LIMIT(0.0, bar, 16.0);
END_IF;
header_raw := bar / 16.0 * 32767.0;
END_FUNCTION_BLOCK
