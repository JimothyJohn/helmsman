(* Batch mixing tank: fill, heat while agitating, hold, drain, repeat.

   plant_model.st stands in for the tank so this runs with nothing wired. To
   wire real I/O: delete plant_model.st, delete the plant lines in the PROGRAM
   (each one says PLANT), and point io.map at your remote I/O module.

   A heater and agitator need a hardwired over-temperature cut-out and E-stop
   circuit. Those belong in hardware; nothing here is a safety function. *)

FUNCTION_BLOCK mixer_tank
VAR_INPUT
  run : BOOL;
  ack : BOOL;
  level_m : REAL;
  temp_c : REAL;
  inlet_open_sw : BOOL;
  inlet_closed_sw : BOOL;
  outlet_open_sw : BOOL;
  outlet_closed_sw : BOOL;
  agitator_fb : BOOL;
  agitator_overload : BOOL;
  overtemp_sw : BOOL;
END_VAR
VAR_OUTPUT
  inlet_coil : BOOL;
  outlet_coil : BOOL;
  agitator_contactor : BOOL;
  heater : BOOL;
  step : INT;
  batches : INT;
  tripped : BOOL;
  alarm_new : BOOL;
END_VAR
VAR CONSTANT
  fill_to_m : REAL := 2.0;        (* batch level *)
  empty_at_m : REAL := 0.05;      (* drained *)
  heater_min_m : REAL := 0.5;     (* element covered *)
  heat_to_c : REAL := 60.0;       (* batch temperature *)
  over_c : REAL := 80.0;          (* high temperature alarm *)
  hold_time : TIME := T#10s;      (* mix at temperature *)
  fill_max : TIME := T#60s;
  heat_max : TIME := T#120s;
  drain_max : TIME := T#60s;
END_VAR
VAR
  seq : step_sequencer;
  inlet : solenoid_valve;
  outlet : solenoid_valve;
  agitator : motor_starter;
  high_temp : latched_alarm;
  step_late : latched_alarm;
  equipment : latched_alarm;
  hold : TON;
  step_done : BOOL;
  step_max : TIME;
  want_inlet : BOOL;
  want_outlet : BOOL;
  want_agitator : BOOL;
  want_heat : BOOL;
END_VAR

high_temp(condition := overtemp_sw OR temp_c >= over_c, ack := ack, on_delay := T#0s);
step_late(condition := seq.timed_out, ack := ack, on_delay := T#0s);
equipment(condition := inlet.fault OR outlet.fault OR agitator.fault, ack := ack, on_delay := T#0s);
tripped := high_temp.showing OR step_late.showing OR equipment.showing;
alarm_new := high_temp.unacked OR step_late.unacked OR equipment.unacked;

seq(start := run AND NOT tripped, advance := step_done, abort := NOT run OR tripped,
    last_step := 4, step_limit := step_max);
step := seq.step;

want_inlet := FALSE;
want_outlet := FALSE;
want_agitator := FALSE;
want_heat := FALSE;
step_done := FALSE;
step_max := T#0s;
CASE step OF
  1: (* fill *)
    want_inlet := level_m < fill_to_m;
    step_done := level_m >= fill_to_m AND inlet.is_closed;
    step_max := fill_max;
  2: (* heat, agitating *)
    want_agitator := TRUE;
    want_heat := agitator.running;
    step_done := temp_c >= heat_to_c;
    step_max := heat_max;
  3: (* hold at temperature *)
    want_agitator := TRUE;
    want_heat := agitator.running AND temp_c < heat_to_c;
    step_done := hold.Q;
  4: (* drain *)
    want_outlet := level_m > empty_at_m;
    step_done := level_m <= empty_at_m AND outlet.is_closed;
    step_max := drain_max;
END_CASE;
hold(IN := step = 3, PT := hold_time);
IF seq.done AND batches < 32767 THEN
  batches := batches + 1;
END_IF;

inlet(open := want_inlet, at_open := inlet_open_sw, at_closed := inlet_closed_sw, reset := ack);
outlet(open := want_outlet, at_open := outlet_open_sw, at_closed := outlet_closed_sw, reset := ack);
agitator(run := want_agitator, feedback := agitator_fb, overload := agitator_overload, reset := ack);
inlet_coil := inlet.coil;
outlet_coil := outlet.coil;
agitator_contactor := agitator.contactor;
(* The heater never runs uncovered, without the agitator, or on an alarm. *)
heater := want_heat AND level_m >= heater_min_m AND NOT tripped;
END_FUNCTION_BLOCK

PROGRAM mixer
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  inlet_open_in : BOOL;
  inlet_closed_in : BOOL;
  outlet_open_in : BOOL;
  outlet_closed_in : BOOL;
  agitator_fb_in : BOOL;
  agitator_overload_in : BOOL;
  overtemp_in : BOOL;
  sim_inlet_stuck : BOOL;  (* PLANT: the inlet valve never opens *)
END_VAR
VAR_OUTPUT
  inlet_valve : BOOL;
  outlet_valve : BOOL;
  agitator : BOOL;
  heater : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  step : INT;
  batches : INT;
  level_m : REAL;
  temp_c : REAL;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  level_tx : analog_scale;
  temp_tx : analog_scale;
  tank : mixer_tank;
  light : stack_light;
  run : BOOL;
  level_raw : REAL;
  temp_raw : REAL;
  inlet_open : BOOL;
  inlet_closed : BOOL;
  outlet_open : BOOL;
  outlet_closed : BOOL;
  agitator_fb : BOOL;
END_VAR

plant(inlet := inlet_valve, outlet := outlet_valve, agitator := agitator, heater := heater,
      inlet_stuck := sim_inlet_stuck);  (* PLANT *)
level_raw := plant.level_raw;  (* PLANT: read your analog input here *)
temp_raw := plant.temp_raw;    (* PLANT: read your analog input here *)
inlet_open := inlet_open_in OR plant.inlet_open;  (* PLANT: keep the _in input *)
inlet_closed := inlet_closed_in OR plant.inlet_closed;  (* PLANT: keep the _in input *)
outlet_open := outlet_open_in OR plant.outlet_open;  (* PLANT: keep the _in input *)
outlet_closed := outlet_closed_in OR plant.outlet_closed;  (* PLANT: keep the _in input *)
agitator_fb := agitator_fb_in OR plant.agitator_fb;  (* PLANT: keep the _in input *)
run := (run OR start_pb OR plant.operator_start) AND NOT stop_pb AND NOT tank.tripped;  (* PLANT: drop operator_start *)

level_tx(raw := level_raw, eng_min := 0.0, eng_max := 3.0);
temp_tx(raw := temp_raw, eng_min := 0.0, eng_max := 120.0);
tank(run := run, ack := ack_pb, level_m := level_tx.value, temp_c := temp_tx.value,
     inlet_open_sw := inlet_open, inlet_closed_sw := inlet_closed,
     outlet_open_sw := outlet_open, outlet_closed_sw := outlet_closed,
     agitator_fb := agitator_fb, agitator_overload := agitator_overload_in,
     overtemp_sw := overtemp_in);
light(running := run, alarm_new := tank.alarm_new, alarm_showing := tank.tripped);

inlet_valve := tank.inlet_coil;
outlet_valve := tank.outlet_coil;
agitator := tank.agitator_contactor;
heater := tank.heater;
lamp_green := light.green;
lamp_red := light.red;
step := tank.step;
batches := tank.batches;
level_m := level_tx.value;
temp_c := temp_tx.value;
END_PROGRAM
