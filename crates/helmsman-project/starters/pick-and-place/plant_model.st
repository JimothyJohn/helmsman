(* PLANT MODEL: not part of the controls. Stands in for the three cylinders and
   the parts arriving in the pick nest so the program moves with nothing wired.
   Delete this file when you wire real I/O. *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  lift : BOOL;
  slide : BOOL;
  grip : BOOL;
  lift_stuck : BOOL;
END_VAR
VAR_OUTPUT
  part : BOOL;
  down : BOOL;
  up : BOOL;
  over : BOOL;
  back : BOOL;
  gripped : BOOL;
  released : BOOL;
  operator_start : BOOL;
END_VAR
VAR CONSTANT
  lift_stroke : TIME := T#400ms;
  slide_stroke : TIME := T#700ms;
  grip_stroke : TIME := T#200ms;
  part_after : TIME := T#2s;  (* a new part this long after the last is taken *)
END_VAR
VAR
  operator : TP;
  lift_down : TON;
  lift_up : TON;
  slide_over : TON;
  slide_back : TON;
  grip_closed : TON;
  grip_open : TON;
  feeder : TON;
END_VAR
operator(IN := TRUE, PT := T#1s);  (* as if someone pressed start *)
operator_start := operator.Q;
lift_down(IN := lift AND NOT lift_stuck, PT := lift_stroke);
lift_up(IN := NOT lift, PT := lift_stroke);
slide_over(IN := slide, PT := slide_stroke);
slide_back(IN := NOT slide, PT := slide_stroke);
grip_closed(IN := grip, PT := grip_stroke);
grip_open(IN := NOT grip, PT := grip_stroke);
down := lift_down.Q;
up := lift_up.Q;
over := slide_over.Q;
back := slide_back.Q;
gripped := grip_closed.Q;
released := grip_open.Q;

(* The nest empties when the gripper has the part, and refills later. *)
feeder(IN := NOT part, PT := part_after);
IF feeder.Q THEN
  part := TRUE;
ELSIF gripped AND NOT over THEN
  part := FALSE;
END_IF;
END_FUNCTION_BLOCK
