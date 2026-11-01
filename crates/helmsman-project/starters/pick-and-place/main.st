(* Pneumatic pick-and-place: lift, slide and gripper cylinders, each with end
   switches, moving a part from the pick nest to the place nest. A cylinder
   that does not reach its switch in time stops the cycle.

   plant_model.st stands in for the cylinders and parts so this runs with
   nothing wired. To wire real I/O: delete plant_model.st, delete the plant
   lines in the PROGRAM (each one says PLANT), and point io.map at your remote
   I/O module.

   Guarding and dumping the air on a stop belong in a hardwired safety circuit;
   nothing here is a safety function. *)

FUNCTION_BLOCK pneumatic_arm
VAR_INPUT
  want_down : BOOL;
  want_over : BOOL;
  want_grip : BOOL;
  ack : BOOL;
  down_sw : BOOL;
  up_sw : BOOL;
  over_sw : BOOL;
  back_sw : BOOL;
  gripped_sw : BOOL;
  released_sw : BOOL;
END_VAR
VAR_OUTPUT
  lift_coil : BOOL;
  slide_coil : BOOL;
  grip_coil : BOOL;
  is_down : BOOL;
  is_up : BOOL;
  is_over : BOOL;
  is_back : BOOL;
  is_gripped : BOOL;
  is_released : BOOL;
  fault : BOOL;
END_VAR
VAR CONSTANT
  travel_time : TIME := T#2s;  (* longest any stroke should take *)
END_VAR
VAR
  lift : solenoid_valve;
  slide : solenoid_valve;
  grip : solenoid_valve;
END_VAR
lift(open := want_down, at_open := down_sw, at_closed := up_sw, reset := ack, travel_time := travel_time);
slide(open := want_over, at_open := over_sw, at_closed := back_sw, reset := ack, travel_time := travel_time);
grip(open := want_grip, at_open := gripped_sw, at_closed := released_sw, reset := ack, travel_time := travel_time);
lift_coil := lift.coil;
slide_coil := slide.coil;
grip_coil := grip.coil;
is_down := lift.is_open;
is_up := lift.is_closed;
is_over := slide.is_open;
is_back := slide.is_closed;
is_gripped := grip.is_open;
is_released := grip.is_closed;
fault := lift.fault OR slide.fault OR grip.fault;
END_FUNCTION_BLOCK

PROGRAM pick_place
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  part_in : BOOL;
  low_air_in : BOOL;
  down_in : BOOL;
  up_in : BOOL;
  over_in : BOOL;
  back_in : BOOL;
  gripped_in : BOOL;
  released_in : BOOL;
  sim_lift_stuck : BOOL;  (* PLANT: the lift never reaches its down switch *)
END_VAR
VAR_OUTPUT
  lift_coil : BOOL;
  slide_coil : BOOL;
  grip_coil : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  step : INT;
  parts : INT;
END_VAR
VAR CONSTANT
  step_max : TIME := T#5s;  (* no step of the cycle takes longer *)
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  arm : pneumatic_arm;
  seq : step_sequencer;
  light : stack_light;
  cylinder : latched_alarm;
  low_air : latched_alarm;
  run : BOOL;
  tripped : BOOL;
  home : BOOL;
  step_done : BOOL;
  part_here : BOOL;
  down_sw : BOOL;
  up_sw : BOOL;
  over_sw : BOOL;
  back_sw : BOOL;
  gripped_sw : BOOL;
  released_sw : BOOL;
END_VAR

plant(lift := lift_coil, slide := slide_coil, grip := grip_coil, lift_stuck := sim_lift_stuck);  (* PLANT *)
part_here := part_in OR plant.part;  (* PLANT: keep the _in input *)
down_sw := down_in OR plant.down;  (* PLANT: keep the _in input *)
up_sw := up_in OR plant.up;  (* PLANT: keep the _in input *)
over_sw := over_in OR plant.over;  (* PLANT: keep the _in input *)
back_sw := back_in OR plant.back;  (* PLANT: keep the _in input *)
gripped_sw := gripped_in OR plant.gripped;  (* PLANT: keep the _in input *)
released_sw := released_in OR plant.released;  (* PLANT: keep the _in input *)

cylinder(condition := arm.fault OR seq.timed_out, ack := ack_pb, on_delay := T#0s);
low_air(condition := low_air_in, ack := ack_pb, on_delay := T#500ms);
tripped := cylinder.showing OR low_air.showing;
run := (run OR start_pb OR plant.operator_start) AND NOT stop_pb AND NOT tripped;  (* PLANT: drop operator_start *)
home := arm.is_up AND arm.is_back AND arm.is_released;

seq(start := run AND home AND part_here, advance := step_done, abort := NOT run,
    last_step := 8, step_limit := step_max);
step := seq.step;
CASE step OF
  1: step_done := arm.is_down;      (* down to the pick nest *)
  2: step_done := arm.is_gripped;   (* grip *)
  3: step_done := arm.is_up;        (* lift the part *)
  4: step_done := arm.is_over;      (* across *)
  5: step_done := arm.is_down;      (* down to the place nest *)
  6: step_done := arm.is_released;  (* let go *)
  7: step_done := arm.is_up;        (* lift clear *)
  8: step_done := arm.is_back;      (* home *)
ELSE
  step_done := FALSE;
END_CASE;
IF seq.done AND parts < 32767 THEN
  parts := parts + 1;
END_IF;

arm(want_down := step = 1 OR step = 2 OR step = 5 OR step = 6,
    want_grip := step >= 2 AND step <= 5,
    want_over := step >= 4 AND step <= 7,
    ack := ack_pb, down_sw := down_sw, up_sw := up_sw, over_sw := over_sw, back_sw := back_sw,
    gripped_sw := gripped_sw, released_sw := released_sw);
light(running := run, alarm_new := cylinder.unacked OR low_air.unacked, alarm_showing := tripped);

lift_coil := arm.lift_coil;
slide_coil := arm.slide_coil;
grip_coil := arm.grip_coil;
lamp_green := light.green;
lamp_red := light.red;
END_PROGRAM
