(* Conveyor sorter: a photo eye measures each part, and a shift register
   carries the long ones down the belt to a pneumatic diverter gate.

   plant_model.st stands in for the belt and parts so this runs with nothing
   wired. To wire real I/O: delete plant_model.st, delete the plant lines in
   the PROGRAM (each one says PLANT), and point io.map at your remote I/O
   module.

   The E-stop and any guard on the gate belong in a hardwired safety circuit;
   nothing here is a safety function. *)

(* One bit per belt step between the eye and the gate. Fires at_gate when a
   long part reaches the gate, however many parts are on the belt. *)
FUNCTION_BLOCK part_tracker
VAR_INPUT
  belt_running : BOOL;
  eye : BOOL;
END_VAR
VAR_OUTPUT
  at_gate : BOOL;
  part_passed : BOOL;
  part_long : BOOL;
END_VAR
VAR CONSTANT
  belt_step : TIME := T#100ms;
  eye_to_gate_steps : INT := 15;  (* 1.5 s of belt travel; at most 15 *)
  long_part : TIME := T#600ms;    (* eye blocked this long: a long part *)
END_VAR
VAR
  tick : TON;
  measure : TON;
  eye_clear : F_TRIG;
  seen_long : BOOL;
  bits : WORD;
END_VAR
tick(IN := belt_running AND NOT tick.Q, PT := belt_step);
measure(IN := eye, PT := long_part);
eye_clear(CLK := eye);
IF measure.Q THEN
  seen_long := TRUE;
END_IF;
part_passed := eye_clear.Q;
part_long := eye_clear.Q AND seen_long;
IF part_long THEN
  bits := bits OR WORD#1;
END_IF;
IF eye_clear.Q THEN
  seen_long := FALSE;
END_IF;
IF tick.Q THEN
  bits := SHL(bits, 1);
END_IF;
at_gate := (bits AND SHL(WORD#1, eye_to_gate_steps)) <> WORD#0;
END_FUNCTION_BLOCK

FUNCTION_BLOCK diverter
VAR_INPUT
  belt_running : BOOL;
  eye : BOOL;
  gate_out_sw : BOOL;
  gate_home_sw : BOOL;
  ack : BOOL;
END_VAR
VAR_OUTPUT
  coil : BOOL;
  fault : BOOL;
  good : INT;
  rejected : INT;
END_VAR
VAR CONSTANT
  gate_hold : TIME := T#600ms;  (* gate stays out for the part to clear *)
END_VAR
VAR
  tracker : part_tracker;
  gate : solenoid_valve;
  hold : TOF;
  arrived : R_TRIG;
END_VAR
tracker(belt_running := belt_running, eye := eye);
hold(IN := tracker.at_gate, PT := gate_hold);
gate(open := hold.Q, at_open := gate_out_sw, at_closed := gate_home_sw, reset := ack);
arrived(CLK := tracker.at_gate);
IF arrived.Q AND rejected < 32767 THEN
  rejected := rejected + 1;
END_IF;
IF tracker.part_passed AND NOT tracker.part_long AND good < 32767 THEN
  good := good + 1;
END_IF;
coil := gate.coil;
fault := gate.fault;
END_FUNCTION_BLOCK

FUNCTION_BLOCK sort_line
VAR_INPUT
  run : BOOL;
  ack : BOOL;
  eye : BOOL;
  belt_fb : BOOL;
  belt_overload : BOOL;
  gate_out_sw : BOOL;
  gate_home_sw : BOOL;
END_VAR
VAR_OUTPUT
  belt_contactor : BOOL;
  gate_coil : BOOL;
  good : INT;
  rejected : INT;
  tripped : BOOL;
  alarm_new : BOOL;
END_VAR
VAR CONSTANT
  jam_time : TIME := T#2s;  (* eye blocked this long with the belt running *)
END_VAR
VAR
  belt : motor_starter;
  gate : diverter;
  jam : latched_alarm;
  equipment : latched_alarm;
END_VAR
belt(run := run, feedback := belt_fb, overload := belt_overload, reset := ack);
gate(belt_running := belt.running, eye := eye, gate_out_sw := gate_out_sw,
     gate_home_sw := gate_home_sw, ack := ack);
jam(condition := eye AND belt.running, ack := ack, on_delay := jam_time);
equipment(condition := belt.fault OR gate.fault, ack := ack, on_delay := T#0s);
tripped := jam.showing OR equipment.showing;
alarm_new := jam.unacked OR equipment.unacked;
belt_contactor := belt.contactor;
gate_coil := gate.coil;
good := gate.good;
rejected := gate.rejected;
END_FUNCTION_BLOCK

PROGRAM sorter
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  eye_in : BOOL;
  belt_fb_in : BOOL;
  belt_overload_in : BOOL;
  gate_out_in : BOOL;
  gate_home_in : BOOL;
END_VAR
VAR_OUTPUT
  belt : BOOL;
  gate : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  good : INT;
  rejected : INT;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  line : sort_line;
  light : stack_light;
  run : BOOL;
  eye : BOOL;
  belt_fb : BOOL;
  gate_out_sw : BOOL;
  gate_home_sw : BOOL;
END_VAR

plant(belt := belt, gate := gate);  (* PLANT *)
eye := eye_in OR plant.eye;  (* PLANT: keep the _in input *)
belt_fb := belt_fb_in OR plant.belt_fb;  (* PLANT: keep the _in input *)
gate_out_sw := gate_out_in OR plant.gate_out;  (* PLANT: keep the _in input *)
gate_home_sw := gate_home_in OR plant.gate_home;  (* PLANT: keep the _in input *)
run := (run OR start_pb OR plant.operator_start) AND NOT stop_pb AND NOT line.tripped;  (* PLANT: drop operator_start *)

line(run := run, ack := ack_pb, eye := eye, belt_fb := belt_fb, belt_overload := belt_overload_in,
     gate_out_sw := gate_out_sw, gate_home_sw := gate_home_sw);
light(running := run, alarm_new := line.alarm_new, alarm_showing := line.tripped);

belt := line.belt_contactor;
gate := line.gate_coil;
lamp_green := light.green;
lamp_red := light.red;
good := line.good;
rejected := line.rejected;
END_PROGRAM
