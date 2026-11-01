(* Template for a process: an analog measurement, a PID loop and two pumps
   sharing the duty, using the blocks in this project. Save a copy, rename what
   is here and set your ranges and tuning. plant_model.st simulates the process
   until you delete it and the plant lines marked PLANT below. Declare new
   inputs and outputs in io.map as well as here.

   High-limit cut-outs and E-stop circuits belong in hardware; nothing here is
   a safety function. *)

FUNCTION_BLOCK pressure_loop
VAR_INPUT
  run : BOOL;
  ack : BOOL;
  raw : REAL;
  fb_a : BOOL;
  fb_b : BOOL;
  overload_a : BOOL;
  overload_b : BOOL;
END_VAR
VAR_OUTPUT
  pump_a_contactor : BOOL;
  pump_b_contactor : BOOL;
  speed_pct : REAL;
  value : REAL;
  alarm_new : BOOL;
  alarm_showing : BOOL;
END_VAR
VAR CONSTANT
  range_min : REAL := 0.0;
  range_max : REAL := 10.0;
  setpoint : REAL := 4.0;
  kp : REAL := 10.0;       (* % speed per unit of error *)
  ti_s : REAL := 15.0;
  high_limit : REAL := 8.0;
  high_delay : TIME := T#5s;
END_VAR
VAR
  sensor : analog_scale;
  pid : pid_loop;
  duty : duty_standby;
  pump_a : motor_starter;
  pump_b : motor_starter;
  high : latched_alarm;
  pump_fault : latched_alarm;
END_VAR
sensor(raw := raw, eng_min := range_min, eng_max := range_max);
high(condition := sensor.value >= high_limit, ack := ack, on_delay := high_delay);
pump_fault(condition := pump_a.fault OR pump_b.fault, ack := ack, on_delay := T#0s);
duty(demand := run AND NOT sensor.bad AND NOT high.active, assist := FALSE,
     a_ok := NOT pump_a.fault, b_ok := NOT pump_b.fault);
pump_a(run := duty.run_a, feedback := fb_a, overload := overload_a, reset := ack);
pump_b(run := duty.run_b, feedback := fb_b, overload := overload_b, reset := ack);
pid(enable := pump_a.running OR pump_b.running, pv := sensor.value, sp := setpoint, kp := kp, ti_s := ti_s);
pump_a_contactor := pump_a.contactor;
pump_b_contactor := pump_b.contactor;
speed_pct := pid.out;
value := sensor.value;
alarm_new := high.unacked OR pump_fault.unacked;
alarm_showing := high.showing OR pump_fault.showing;
END_FUNCTION_BLOCK

PROGRAM process
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  fb_a_in : BOOL;
  fb_b_in : BOOL;
  overload_a_in : BOOL;
  overload_b_in : BOOL;
END_VAR
VAR_OUTPUT
  pump_a : BOOL;
  pump_b : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  speed_pct : REAL;
  pressure : REAL;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  loop : pressure_loop;
  light : stack_light;
  run : BOOL;
  raw : REAL;
  fb_a : BOOL;
  fb_b : BOOL;
END_VAR

plant(pump_a := pump_a, pump_b := pump_b, speed_pct := speed_pct);  (* PLANT *)
raw := plant.raw;  (* PLANT: read your analog input here *)
fb_a := fb_a_in OR plant.fb_a;  (* PLANT: keep the _in input *)
fb_b := fb_b_in OR plant.fb_b;  (* PLANT: keep the _in input *)
run := (run OR start_pb OR plant.operator_start) AND NOT stop_pb;  (* PLANT: drop operator_start *)

loop(run := run, ack := ack_pb, raw := raw, fb_a := fb_a, fb_b := fb_b,
     overload_a := overload_a_in, overload_b := overload_b_in);
light(running := run, alarm_new := loop.alarm_new, alarm_showing := loop.alarm_showing);

pump_a := loop.pump_a_contactor;
pump_b := loop.pump_b_contactor;
lamp_green := light.green;
lamp_red := light.red;
speed_pct := loop.speed_pct;
pressure := loop.value;
END_PROGRAM
