(* PLANT MODEL: not part of the controls. Simulates the motor and cylinder so
   the program moves with nothing wired. Delete this file when you wire real
   I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  motor : BOOL;
  cylinder : BOOL;
END_VAR
VAR_OUTPUT
  motor_fb : BOOL;
  cylinder_out : BOOL;
  cylinder_home : BOOL;
END_VAR
VAR CONSTANT
  stroke : TIME := T#500ms;
  motor_delay : TIME := T#200ms;
END_VAR
VAR
  pull_in : TON;
  extended : TON;
  retracted : TON;
END_VAR
pull_in(IN := motor, PT := motor_delay);
extended(IN := cylinder, PT := stroke);
retracted(IN := NOT cylinder, PT := stroke);
motor_fb := pull_in.Q;
cylinder_out := extended.Q;
cylinder_home := retracted.Q;
END_FUNCTION_BLOCK
