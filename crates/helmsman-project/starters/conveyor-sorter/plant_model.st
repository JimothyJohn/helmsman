(* PLANT MODEL: not part of the controls. Stands in for the belt, the parts
   riding on it and the gate cylinder so the program moves with nothing wired.
   Delete this file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  belt : BOOL;
  gate : BOOL;
END_VAR
VAR_OUTPUT
  eye : BOOL;
  belt_fb : BOOL;
  gate_out : BOOL;
  gate_home : BOOL;
END_VAR
VAR CONSTANT
  part_every : TIME := T#3s;
  long_every : INT := 3;          (* every third part is long *)
  short_len : TIME := T#300ms;    (* time a short part blocks the eye *)
  long_len : TIME := T#900ms;
  motor_delay : TIME := T#200ms;
  stroke : TIME := T#300ms;       (* gate travel *)
END_VAR
VAR
  motor : TON;
  gate_extended : TON;
  gate_retracted : TON;
  next_part : TON;
  blocking : TP;
  parts : INT;
  part_len : TIME;
END_VAR
motor(IN := belt, PT := motor_delay);
belt_fb := motor.Q;
gate_extended(IN := gate, PT := stroke);
gate_retracted(IN := NOT gate, PT := stroke);
gate_out := gate_extended.Q;
gate_home := gate_retracted.Q;

next_part(IN := belt_fb AND NOT next_part.Q, PT := part_every);
IF next_part.Q THEN
  parts := (parts + 1) MOD long_every;
END_IF;
IF parts = 0 THEN
  part_len := long_len;
ELSE
  part_len := short_len;
END_IF;
blocking(IN := next_part.Q, PT := part_len);
eye := blocking.Q;
END_FUNCTION_BLOCK
