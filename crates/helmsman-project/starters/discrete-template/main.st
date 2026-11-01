(* Template for a discrete machine: one motor, one cylinder and a three-step
   cycle, using the blocks in this project. Save a copy, rename what is here
   and add your steps. plant_model.st simulates the machine until you delete it
   and the plant lines marked PLANT below. Declare new inputs and outputs in
   io.map as well as here.

   E-stop and guard circuits belong in hardwired safety circuits; nothing here
   is a safety function. *)

FUNCTION_BLOCK station
VAR_INPUT
  run : BOOL;
  ack : BOOL;
  motor_fb : BOOL;
  motor_overload : BOOL;
  out_sw : BOOL;
  home_sw : BOOL;
END_VAR
VAR_OUTPUT
  motor_contactor : BOOL;
  cylinder_coil : BOOL;
  step : INT;
  cycles : INT;
  tripped : BOOL;
  alarm_new : BOOL;
END_VAR
VAR CONSTANT
  work_time : TIME := T#2s;   (* motor runs this long in step 2 *)
  step_time : TIME := T#10s;  (* no step takes longer *)
END_VAR
VAR
  seq : step_sequencer;
  motor : motor_starter;
  cylinder : solenoid_valve;
  equipment : latched_alarm;
  step_late : latched_alarm;
  working : TON;
  step_done : BOOL;
END_VAR
equipment(condition := motor.fault OR cylinder.fault, ack := ack, on_delay := T#0s);
step_late(condition := seq.timed_out, ack := ack, on_delay := T#0s);
tripped := equipment.showing OR step_late.showing;
alarm_new := equipment.unacked OR step_late.unacked;

seq(start := run AND NOT tripped, advance := step_done, abort := NOT run OR tripped,
    last_step := 3, step_limit := step_time);
step := seq.step;
working(IN := step = 2, PT := work_time);
CASE step OF
  1: step_done := cylinder.is_open;   (* cylinder out *)
  2: step_done := working.Q;          (* motor runs *)
  3: step_done := cylinder.is_closed; (* cylinder home *)
ELSE
  step_done := FALSE;
END_CASE;
IF seq.done AND cycles < 32767 THEN
  cycles := cycles + 1;
END_IF;

motor(run := step = 2, feedback := motor_fb, overload := motor_overload, reset := ack);
cylinder(open := step = 1 OR step = 2, at_open := out_sw, at_closed := home_sw, reset := ack);
motor_contactor := motor.contactor;
cylinder_coil := cylinder.coil;
END_FUNCTION_BLOCK

PROGRAM machine
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  motor_fb_in : BOOL;
  motor_overload_in : BOOL;
  out_in : BOOL;
  home_in : BOOL;
END_VAR
VAR_OUTPUT
  motor : BOOL;
  cylinder : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  step : INT;
  cycles : INT;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  work : station;
  light : stack_light;
  run : BOOL;
  motor_fb : BOOL;
  out_sw : BOOL;
  home_sw : BOOL;
END_VAR

plant(motor := motor, cylinder := cylinder);  (* PLANT *)
motor_fb := motor_fb_in OR plant.motor_fb;  (* PLANT: keep the _in input *)
out_sw := out_in OR plant.cylinder_out;  (* PLANT: keep the _in input *)
home_sw := home_in OR plant.cylinder_home;  (* PLANT: keep the _in input *)
run := (run OR start_pb OR plant.operator_start) AND NOT stop_pb AND NOT work.tripped;  (* PLANT: drop operator_start *)

work(run := run, ack := ack_pb, motor_fb := motor_fb, motor_overload := motor_overload_in,
     out_sw := out_sw, home_sw := home_sw);
light(running := run, alarm_new := work.alarm_new, alarm_showing := work.tripped);

motor := work.motor_contactor;
cylinder := work.cylinder_coil;
lamp_green := light.green;
lamp_red := light.red;
step := work.step;
cycles := work.cycles;
END_PROGRAM
