(* Three compressors on one receiver, staged on header pressure. The lead
   rotates so they share the hours; a compressor that trips is skipped and the
   next takes its place. Each has a minimum run and a rest before restarting.

   plant_model.st stands in for the receiver and the compressors so this runs
   with nothing wired. To wire real I/O: delete plant_model.st, delete the plant
   lines in the PROGRAM (each one says PLANT), and point io.map at your remote
   I/O module.

   The relief valve and any high-pressure cut-out belong in hardware; nothing
   here is a safety function. *)

FUNCTION_BLOCK compressor
VAR_INPUT
  want : BOOL;
  inhibit : BOOL;
  ack : BOOL;
  feedback : BOOL;
  overload : BOOL;
  hot_sw : BOOL;
END_VAR
VAR_OUTPUT
  contactor : BOOL;
  available : BOOL;
  alarm_new : BOOL;
  alarm_showing : BOOL;
END_VAR
VAR CONSTANT
  min_run : TIME := T#20s;   (* no short cycling *)
  min_rest : TIME := T#10s;  (* motor rest between starts *)
END_VAR
VAR
  motor : motor_starter;
  hot : latched_alarm;
  equipment : latched_alarm;
  keep_running : TOF;
  rested : TON;
  ok : BOOL;
END_VAR
hot(condition := hot_sw, ack := ack, on_delay := T#0s);
equipment(condition := motor.fault, ack := ack, on_delay := T#0s);
ok := NOT hot.showing AND NOT equipment.showing;
keep_running(IN := want, PT := min_run);
rested(IN := NOT motor.contactor, PT := min_rest);
motor(run := ok AND NOT inhibit AND keep_running.Q AND (motor.contactor OR rested.Q), feedback := feedback,
      overload := overload, reset := ack);
contactor := motor.contactor;
available := ok;
alarm_new := hot.unacked OR equipment.unacked;
alarm_showing := hot.showing OR equipment.showing;
END_FUNCTION_BLOCK

(* How many compressors the header needs, one more or one fewer at a time. *)
FUNCTION_BLOCK stager
VAR_INPUT
  run : BOOL;
  bar : REAL;
END_VAR
VAR_OUTPUT
  stages : INT;
  lead : INT;
END_VAR
VAR CONSTANT
  cut_in_bar : REAL := 6.5;
  cut_out_bar : REAL := 7.5;
  stage_delay : TIME := T#15s;     (* wait to see the last stage take effect *)
  rotate_every : TIME := T#3m;     (* a real plant would use days *)
END_VAR
VAR
  add : TON;
  drop : TON;
  rotate : TON;
END_VAR
add(IN := run AND bar < cut_in_bar AND stages < 3 AND NOT add.Q, PT := stage_delay);
drop(IN := run AND bar > cut_out_bar AND stages > 0 AND NOT drop.Q, PT := stage_delay);
IF NOT run THEN
  stages := 0;
ELSIF add.Q THEN
  stages := stages + 1;
ELSIF drop.Q THEN
  stages := stages - 1;
END_IF;
rotate(IN := NOT rotate.Q, PT := rotate_every);
IF rotate.Q THEN
  lead := (lead + 1) MOD 3;
END_IF;
END_FUNCTION_BLOCK

PROGRAM compressors
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  fb_1_in : BOOL;
  fb_2_in : BOOL;
  fb_3_in : BOOL;
  overload_1_in : BOOL;
  overload_2_in : BOOL;
  overload_3_in : BOOL;
  hot_1_in : BOOL;
  hot_2_in : BOOL;
  hot_3_in : BOOL;
  overpressure_in : BOOL;
END_VAR
VAR_OUTPUT
  comp_1 : BOOL;
  comp_2 : BOOL;
  comp_3 : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  stages : INT;
  lead : INT;
  header_bar : REAL;
END_VAR
VAR CONSTANT
  over_bar : REAL := 9.0;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  header_tx : analog_scale;
  staging : stager;
  c1 : compressor;
  c2 : compressor;
  c3 : compressor;
  overpressure : latched_alarm;
  light : stack_light;
  run : BOOL;
  header_raw : REAL;
  avail : ARRAY[0..2] OF BOOL;
  want : ARRAY[0..2] OF BOOL;
  need : INT;
  k : INT;
  unit : INT;
  fb_1 : BOOL;
  fb_2 : BOOL;
  fb_3 : BOOL;
END_VAR

plant(comp_1 := comp_1, comp_2 := comp_2, comp_3 := comp_3);  (* PLANT *)
header_raw := plant.header_raw;  (* PLANT: read your analog input here *)
fb_1 := fb_1_in OR plant.fb_1;  (* PLANT: keep the _in input *)
fb_2 := fb_2_in OR plant.fb_2;  (* PLANT: keep the _in input *)
fb_3 := fb_3_in OR plant.fb_3;  (* PLANT: keep the _in input *)

header_tx(raw := header_raw, eng_min := 0.0, eng_max := 16.0);
overpressure(condition := overpressure_in OR header_tx.value >= over_bar, ack := ack_pb, on_delay := T#0s);
run := (run OR start_pb) AND NOT stop_pb AND NOT overpressure.showing;
staging(run := run, bar := header_tx.value);

(* Give the stages to whoever is available, starting from the lead. *)
avail[0] := c1.available;
avail[1] := c2.available;
avail[2] := c3.available;
need := staging.stages;
FOR k := 0 TO 2 DO
  unit := (staging.lead + k) MOD 3;
  want[unit] := need > 0 AND avail[unit];
  IF want[unit] THEN
    need := need - 1;
  END_IF;
END_FOR;

(* Over pressure drops all three at once; the minimum run gives way to it. *)
c1(want := want[0], inhibit := overpressure.showing, ack := ack_pb, feedback := fb_1, overload := overload_1_in, hot_sw := hot_1_in);
c2(want := want[1], inhibit := overpressure.showing, ack := ack_pb, feedback := fb_2, overload := overload_2_in, hot_sw := hot_2_in);
c3(want := want[2], inhibit := overpressure.showing, ack := ack_pb, feedback := fb_3, overload := overload_3_in, hot_sw := hot_3_in);
light(running := run, alarm_new := overpressure.unacked OR c1.alarm_new OR c2.alarm_new OR c3.alarm_new,
      alarm_showing := overpressure.showing OR c1.alarm_showing OR c2.alarm_showing OR c3.alarm_showing);

comp_1 := c1.contactor;
comp_2 := c2.contactor;
comp_3 := c3.contactor;
lamp_green := light.green;
lamp_red := light.red;
stages := staging.stages;
lead := staging.lead;
header_bar := header_tx.value;
END_PROGRAM
