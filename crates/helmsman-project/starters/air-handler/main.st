(* Air handling unit: open the outside-air damper, start the supply fan, prove
   airflow, then hold supply temperature with a PID on the heating coil valve.
   The freeze stat shuts the unit down with the coil valve wide open.

   plant_model.st stands in for the duct and coil so this runs with nothing
   wired. To wire real I/O: delete plant_model.st, delete the plant lines in the
   PROGRAM (each one says PLANT), and point io.map at your remote I/O module.

   Smoke shutdown and any fire damper belong in the hardwired life-safety
   circuit; nothing here is a safety function. *)

(* The supply temperature loop and its transmitter. *)
FUNCTION_BLOCK heating_loop
VAR_INPUT
  enable : BOOL;
  freeze : BOOL;
  temp_raw : REAL;
END_VAR
VAR_OUTPUT
  supply_c : REAL;
  valve_pct : REAL;
  sensor_bad : BOOL;
END_VAR
VAR CONSTANT
  setpoint_c : REAL := 18.0;
  kp : REAL := 4.0;      (* % valve per degree *)
  ti_s : REAL := 40.0;
END_VAR
VAR
  sensor : analog_scale;
  pid : pid_loop;
END_VAR
sensor(raw := temp_raw, eng_min := -20.0, eng_max := 60.0);
pid(enable := enable AND NOT sensor.bad, pv := sensor.value, sp := setpoint_c, kp := kp, ti_s := ti_s);
supply_c := sensor.value;
sensor_bad := sensor.bad;
IF freeze THEN
  valve_pct := 100.0;  (* keep water moving through the coil *)
ELSE
  valve_pct := pid.out;
END_IF;
END_FUNCTION_BLOCK

(* Damper first, then fan, then prove the air is moving. *)
FUNCTION_BLOCK fan_section
VAR_INPUT
  run : BOOL;
  ack : BOOL;
  damper_open_sw : BOOL;
  damper_shut_sw : BOOL;
  fan_fb : BOOL;
  fan_overload : BOOL;
  airflow_sw : BOOL;
END_VAR
VAR_OUTPUT
  damper_coil : BOOL;
  fan_contactor : BOOL;
  air_proven : BOOL;
  state : INT;  (* 0 off, 1 damper opening, 2 fan starting, 3 running *)
  fault : BOOL;
END_VAR
VAR CONSTANT
  prove_time : TIME := T#15s;  (* airflow must prove by then *)
END_VAR
VAR
  damper : solenoid_valve;
  fan : motor_starter;
  no_air : TON;
END_VAR
IF NOT run THEN
  state := 0;
END_IF;
no_air(IN := state >= 2 AND NOT airflow_sw, PT := prove_time);
CASE state OF
  0:
    IF run AND NOT fault THEN state := 1; END_IF;
  1:
    IF damper.is_open THEN state := 2; END_IF;
  2:
    IF airflow_sw THEN state := 3; END_IF;
END_CASE;
damper(open := state >= 1, at_open := damper_open_sw, at_closed := damper_shut_sw, reset := ack,
       travel_time := T#30s);
fan(run := state >= 2, feedback := fan_fb, overload := fan_overload, reset := ack);
IF no_air.Q THEN
  fault := TRUE;
ELSIF ack THEN
  fault := FALSE;
END_IF;
damper_coil := damper.coil;
fan_contactor := fan.contactor;
air_proven := state = 3;
fault := fault OR damper.fault OR fan.fault;
END_FUNCTION_BLOCK

PROGRAM ahu
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  ack_pb : BOOL;
  damper_open_in : BOOL;
  damper_shut_in : BOOL;
  fan_fb_in : BOOL;
  fan_overload_in : BOOL;
  airflow_in : BOOL;
  freeze_in : BOOL;
  sim_belt_broken : BOOL;  (* PLANT: the fan runs but moves no air *)
END_VAR
VAR_OUTPUT
  damper : BOOL;
  fan : BOOL;
  lamp_green : BOOL;
  lamp_red : BOOL;
  state : INT;
  valve_pct : REAL;
  supply_c : REAL;
  filter_pa : REAL;
END_VAR
VAR CONSTANT
  filter_dirty_pa : REAL := 250.0;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  section : fan_section;
  loop : heating_loop;
  light : stack_light;
  freeze : latched_alarm;
  fan_fault : latched_alarm;
  temp_sensor : latched_alarm;
  filter_dirty : latched_alarm;
  run : BOOL;
  tripped : BOOL;
  damper_open_sw : BOOL;
  damper_shut_sw : BOOL;
  fan_fb : BOOL;
  airflow_sw : BOOL;
  temp_raw : REAL;
END_VAR

plant(damper := damper, fan := fan, valve_pct := valve_pct, belt_broken := sim_belt_broken);  (* PLANT *)
temp_raw := plant.temp_raw;  (* PLANT: read your analog input here *)
filter_pa := plant.filter_pa;  (* PLANT: read your analog input here *)
damper_open_sw := damper_open_in OR plant.damper_open;  (* PLANT: keep the _in input *)
damper_shut_sw := damper_shut_in OR plant.damper_shut;  (* PLANT: keep the _in input *)
fan_fb := fan_fb_in OR plant.fan_fb;  (* PLANT: keep the _in input *)
airflow_sw := airflow_in OR plant.airflow;  (* PLANT: keep the _in input *)

freeze(condition := freeze_in, ack := ack_pb, on_delay := T#0s);
fan_fault(condition := section.fault, ack := ack_pb, on_delay := T#0s);
temp_sensor(condition := loop.sensor_bad, ack := ack_pb, on_delay := T#5s);
filter_dirty(condition := filter_pa >= filter_dirty_pa, ack := ack_pb, on_delay := T#30s);
tripped := freeze.showing OR fan_fault.showing OR temp_sensor.showing;
run := (run OR start_pb) AND NOT stop_pb AND NOT tripped;

section(run := run, ack := ack_pb, damper_open_sw := damper_open_sw, damper_shut_sw := damper_shut_sw,
        fan_fb := fan_fb, fan_overload := fan_overload_in, airflow_sw := airflow_sw);
loop(enable := section.air_proven, freeze := freeze.showing, temp_raw := temp_raw);
light(running := run, alarm_new := freeze.unacked OR fan_fault.unacked OR temp_sensor.unacked OR filter_dirty.unacked,
      alarm_showing := tripped OR filter_dirty.showing);

damper := section.damper_coil;
fan := section.fan_contactor;
lamp_green := light.green;
lamp_red := light.red;
state := section.state;
valve_pct := loop.valve_pct;
supply_c := loop.supply_c;
END_PROGRAM
