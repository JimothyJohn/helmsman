(* Lift station: two pumps on one wet well, duty and standby, run from a level
   transmitter between start and stop levels. The standby assists at a higher
   level and takes over from a pump that has faulted; duty swaps at each stop.

   plant_model.st stands in for the well and pumps so this runs with nothing
   wired. To wire real I/O: delete plant_model.st, delete the plant lines in
   the PROGRAM (each one says PLANT), and point io.map at your remote I/O
   module.

   A hardwired high-level float and motor protection belong in hardware;
   nothing here is a safety function. *)

(* Turns the well level into demand, assist and the level alarms. *)
FUNCTION_BLOCK well_control
VAR_INPUT
  level_m : REAL;
  level_bad : BOOL;
  low_float : BOOL;
  ack : BOOL;
END_VAR
VAR_OUTPUT
  demand : BOOL;
  assist : BOOL;
  dry_run_lockout : BOOL;
  alarm_new : BOOL;
  alarm_showing : BOOL;
END_VAR
VAR CONSTANT
  stop_at_m : REAL := 0.8;
  start_at_m : REAL := 2.0;
  assist_at_m : REAL := 2.6;
  high_at_m : REAL := 3.4;
  dry_at_m : REAL := 0.3;
END_VAR
VAR
  high : latched_alarm;
  dry : latched_alarm;
  transmitter : latched_alarm;
END_VAR
IF level_m >= start_at_m THEN
  demand := TRUE;
ELSIF level_m <= stop_at_m THEN
  demand := FALSE;
END_IF;
IF level_m >= assist_at_m THEN
  assist := TRUE;
ELSIF level_m <= start_at_m THEN
  assist := FALSE;
END_IF;
high(condition := level_m >= high_at_m, ack := ack, on_delay := T#5s);
dry(condition := low_float OR level_m <= dry_at_m, ack := ack, on_delay := T#0s);
transmitter(condition := level_bad, ack := ack, on_delay := T#2s);
dry_run_lockout := dry.active OR transmitter.active;
alarm_new := high.unacked OR dry.unacked OR transmitter.unacked;
alarm_showing := high.showing OR dry.showing OR transmitter.showing;
END_FUNCTION_BLOCK

FUNCTION_BLOCK pump_pair
VAR_INPUT
  demand : BOOL;
  assist : BOOL;
  ack : BOOL;
  fb_a : BOOL;
  fb_b : BOOL;
  overload_a : BOOL;
  overload_b : BOOL;
END_VAR
VAR_OUTPUT
  contactor_a : BOOL;
  contactor_b : BOOL;
  alarm_new : BOOL;
  alarm_showing : BOOL;
END_VAR
VAR
  duty : duty_standby;
  pump_a : motor_starter;
  pump_b : motor_starter;
  pump_fault : latched_alarm;
  none_left : latched_alarm;
END_VAR
duty(demand := demand, assist := assist, a_ok := NOT pump_a.fault, b_ok := NOT pump_b.fault);
pump_a(run := duty.run_a, feedback := fb_a, overload := overload_a, reset := ack);
pump_b(run := duty.run_b, feedback := fb_b, overload := overload_b, reset := ack);
pump_fault(condition := pump_a.fault OR pump_b.fault, ack := ack, on_delay := T#0s);
none_left(condition := duty.none_available, ack := ack, on_delay := T#0s);
contactor_a := pump_a.contactor;
contactor_b := pump_b.contactor;
alarm_new := pump_fault.unacked OR none_left.unacked;
alarm_showing := pump_fault.showing OR none_left.showing;
END_FUNCTION_BLOCK

PROGRAM pump_station
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  fb_a_in : BOOL;
  fb_b_in : BOOL;
  overload_a_in : BOOL;
  overload_b_in : BOOL;
  low_float_in : BOOL;
END_VAR
VAR_OUTPUT
  pump_a : BOOL;
  pump_b : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  level_m : REAL;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  level_tx : analog_scale;
  well : well_control;
  pumps : pump_pair;
  light : stack_light;
  run : BOOL;
  level_raw : REAL;
  fb_a : BOOL;
  fb_b : BOOL;
END_VAR

plant(pump_a := pump_a, pump_b := pump_b);  (* PLANT *)
level_raw := plant.level_raw;  (* PLANT: read your analog input here *)
fb_a := fb_a_in OR plant.fb_a;  (* PLANT: keep the _in input *)
fb_b := fb_b_in OR plant.fb_b;  (* PLANT: keep the _in input *)
run := (run OR start_pb) AND NOT stop_pb;

level_tx(raw := level_raw, eng_min := 0.0, eng_max := 4.0);
well(level_m := level_tx.value, level_bad := level_tx.bad, low_float := low_float_in, ack := ack_pb);
pumps(demand := run AND well.demand AND NOT well.dry_run_lockout,
      assist := well.assist, ack := ack_pb, fb_a := fb_a, fb_b := fb_b,
      overload_a := overload_a_in, overload_b := overload_b_in);
light(running := run, alarm_new := well.alarm_new OR pumps.alarm_new,
      alarm_showing := well.alarm_showing OR pumps.alarm_showing);

pump_a := pumps.contactor_a;
pump_b := pumps.contactor_b;
lamp_green := light.green;
lamp_red := light.red;
level_m := level_tx.value;
END_PROGRAM
