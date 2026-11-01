(* PLANT MODEL: not part of the controls. Stands in for the belt, the bottles on
   it and the two heads so the program moves with nothing wired. Delete this
   file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  belt : BOOL;
  nozzle : BOOL;
  press : BOOL;
  no_bottles : BOOL;
END_VAR
VAR_OUTPUT
  bottle_eye : BOOL;
  belt_fb : BOOL;
  nozzle_down : BOOL;
  nozzle_up : BOOL;
  press_down : BOOL;
  press_up : BOOL;
  operator_start : BOOL;
END_VAR
VAR CONSTANT
  pitch : TIME := T#1500ms;  (* belt time from one bottle to the next *)
  stroke : TIME := T#300ms;
  motor_delay : TIME := T#200ms;
END_VAR
VAR
  operator : TP;
  motor : TON;
  belt_started : R_TRIG;
  travel : TON;
  noz_down : TON;
  noz_up : TON;
  prs_down : TON;
  prs_up : TON;
END_VAR
operator(IN := TRUE, PT := T#1s);  (* as if someone pressed start *)
operator_start := operator.Q;
motor(IN := belt, PT := motor_delay);
belt_fb := motor.Q;
noz_down(IN := nozzle, PT := stroke);
noz_up(IN := NOT nozzle, PT := stroke);
prs_down(IN := press, PT := stroke);
prs_up(IN := NOT press, PT := stroke);
nozzle_down := noz_down.Q;
nozzle_up := noz_up.Q;
press_down := prs_down.Q;
press_up := prs_up.Q;

(* The bottle in front of the eye leaves as the belt starts, and the next one
   arrives one pitch later. *)
belt_started(CLK := belt_fb);
IF belt_started.Q THEN
  bottle_eye := FALSE;
END_IF;
travel(IN := belt_fb AND NOT no_bottles AND NOT bottle_eye, PT := pitch);
IF travel.Q THEN
  bottle_eye := TRUE;
END_IF;
END_FUNCTION_BLOCK
