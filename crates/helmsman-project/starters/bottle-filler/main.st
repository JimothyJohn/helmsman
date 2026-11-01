(* Bottle filler and capper on an indexing conveyor: the belt brings a bottle
   to the eye and stops, then the fill head and the capper each do their work
   on the bottle in front of them, then the belt indexes again.

   plant_model.st stands in for the belt, heads and bottles so this runs with
   nothing wired. To wire real I/O: delete plant_model.st, delete the plant
   lines in the PROGRAM (each one says PLANT), and point io.map at your remote
   I/O module.

   The E-stop and guard interlocks belong in a hardwired safety circuit;
   nothing here is a safety function. *)

(* Lowers the nozzle, fills for a set time, raises it. *)
FUNCTION_BLOCK fill_head
VAR_INPUT
  go : BOOL;
  ack : BOOL;
  down_sw : BOOL;
  up_sw : BOOL;
END_VAR
VAR_OUTPUT
  nozzle_coil : BOOL;
  fill_valve : BOOL;
  finished : BOOL;
  fault : BOOL;
END_VAR
VAR CONSTANT
  fill_time : TIME := T#2s;  (* set for the bottle size and head pressure *)
END_VAR
VAR
  nozzle : solenoid_valve;
  filling : TON;
  state : INT;  (* 0 wait, 1 lower, 2 fill, 3 raise, 4 finished *)
END_VAR
IF NOT go THEN
  state := 0;
END_IF;
filling(IN := state = 2, PT := fill_time);
CASE state OF
  0:
    IF go THEN state := 1; END_IF;
  1:
    IF nozzle.is_open THEN state := 2; END_IF;
  2:
    IF filling.Q THEN state := 3; END_IF;
  3:
    IF nozzle.is_closed THEN state := 4; END_IF;
END_CASE;
nozzle(open := state = 1 OR state = 2, at_open := down_sw, at_closed := up_sw, reset := ack);
nozzle_coil := nozzle.coil;
fill_valve := state = 2;
finished := state = 4;
fault := nozzle.fault;
END_FUNCTION_BLOCK

(* Presses a cap on for a set time. *)
FUNCTION_BLOCK cap_head
VAR_INPUT
  go : BOOL;
  ack : BOOL;
  down_sw : BOOL;
  up_sw : BOOL;
END_VAR
VAR_OUTPUT
  press_coil : BOOL;
  finished : BOOL;
  fault : BOOL;
END_VAR
VAR CONSTANT
  press_time : TIME := T#500ms;
END_VAR
VAR
  press : solenoid_valve;
  pressing : TON;
  pressed : BOOL;
END_VAR
IF NOT go THEN
  pressed := FALSE;
END_IF;
pressing(IN := go AND press.is_open, PT := press_time);
IF pressing.Q THEN
  pressed := TRUE;
END_IF;
press(open := go AND NOT pressed, at_open := down_sw, at_closed := up_sw, reset := ack);
press_coil := press.coil;
finished := pressed AND press.is_closed;
fault := press.fault;
END_FUNCTION_BLOCK

PROGRAM filler
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  bottle_eye_in : BOOL;
  belt_fb_in : BOOL;
  belt_overload_in : BOOL;
  nozzle_down_in : BOOL;
  nozzle_up_in : BOOL;
  press_down_in : BOOL;
  press_up_in : BOOL;
  tank_low_in : BOOL;
  sim_no_bottles : BOOL;  (* PLANT: the infeed has run out *)
END_VAR
VAR_OUTPUT
  belt : BOOL;
  nozzle : BOOL;
  fill_valve : BOOL;
  cap_press : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  step : INT;
  bottles : INT;
END_VAR
VAR CONSTANT
  index_max : TIME := T#8s;   (* no bottle at the eye by now: jam or none left *)
  work_max : TIME := T#10s;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  conveyor : motor_starter;
  filling : fill_head;
  capping : cap_head;
  seq : step_sequencer;
  light : stack_light;
  no_bottle : latched_alarm;
  tank_low : latched_alarm;
  equipment : latched_alarm;
  run : BOOL;
  tripped : BOOL;
  step_done : BOOL;
  step_max : TIME;
  bottle_eye : BOOL;
  belt_fb : BOOL;
  nozzle_down : BOOL;
  nozzle_up : BOOL;
  press_down : BOOL;
  press_up : BOOL;
END_VAR

plant(belt := belt, nozzle := nozzle, press := cap_press, no_bottles := sim_no_bottles);  (* PLANT *)
bottle_eye := bottle_eye_in OR plant.bottle_eye;  (* PLANT: keep the _in input *)
belt_fb := belt_fb_in OR plant.belt_fb;  (* PLANT: keep the _in input *)
nozzle_down := nozzle_down_in OR plant.nozzle_down;  (* PLANT: keep the _in input *)
nozzle_up := nozzle_up_in OR plant.nozzle_up;  (* PLANT: keep the _in input *)
press_down := press_down_in OR plant.press_down;  (* PLANT: keep the _in input *)
press_up := press_up_in OR plant.press_up;  (* PLANT: keep the _in input *)

no_bottle(condition := seq.timed_out, ack := ack_pb, on_delay := T#0s);
tank_low(condition := tank_low_in, ack := ack_pb, on_delay := T#1s);
equipment(condition := conveyor.fault OR filling.fault OR capping.fault, ack := ack_pb, on_delay := T#0s);
tripped := no_bottle.showing OR tank_low.showing OR equipment.showing;
run := (run OR start_pb) AND NOT stop_pb AND NOT tripped;

seq(start := run, advance := step_done, abort := NOT run, last_step := 2, step_limit := step_max);
step := seq.step;
step_done := FALSE;
step_max := T#0s;
CASE step OF
  1: (* index to the next bottle *)
    step_done := bottle_eye AND conveyor.running;
    step_max := index_max;
  2: (* fill and cap *)
    step_done := filling.finished AND capping.finished;
    step_max := work_max;
END_CASE;
IF seq.done AND bottles < 32767 THEN
  bottles := bottles + 1;
END_IF;

conveyor(run := step = 1 AND NOT step_done, feedback := belt_fb, overload := belt_overload_in, reset := ack_pb);
filling(go := step = 2, ack := ack_pb, down_sw := nozzle_down, up_sw := nozzle_up);
capping(go := step = 2, ack := ack_pb, down_sw := press_down, up_sw := press_up);
light(running := run, alarm_new := no_bottle.unacked OR tank_low.unacked OR equipment.unacked,
      alarm_showing := tripped);

belt := conveyor.contactor;
nozzle := filling.nozzle_coil;
fill_valve := filling.fill_valve;
cap_press := capping.press_coil;
lamp_green := light.green;
lamp_red := light.red;
END_PROGRAM
