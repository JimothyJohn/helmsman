//! The programs the playground opened with before 2026-11-05 (blink with
//! an enable, a located-bit latch, a counter, a traffic light, a line of
//! nested blocks), kept as test fixtures: they exercise the diagram, the
//! tree, the Monitor and the I/O far more than the textbook examples the
//! page now offers (`playground::EXAMPLES`). Never shown to a user.

use crate::playground::Example;

pub const PROGRAMS: [Example; 5] = [
    Example {
        name: "Blink (TON)",
        src: "PROGRAM blink
VAR_INPUT
  enable : BOOL := TRUE;
  period : TIME := T#500ms;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
(* A timer that restarts itself: Q is TRUE for one scan every period. *)
pulse(IN := enable AND NOT pulse.Q, PT := period);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
IF NOT enable THEN
  lamp := FALSE;
END_IF;
END_PROGRAM
",
    },
    Example {
        name: "Start/stop latch",
        src: "PROGRAM latch
VAR
  start AT %IX0.0 : BOOL;  (* push button *)
  stop AT %IX0.1 : BOOL;   (* push button *)
  motor AT %QX0.0 : BOOL;
END_VAR
(* Seal-in: start latches the motor on, stop wins. *)
motor := (start OR motor) AND NOT stop;
END_PROGRAM
",
    },
    Example {
        name: "Counter",
        src: "PROGRAM counter
VAR_INPUT
  pulse : BOOL;
  reset : BOOL;
  target : INT := 5;
END_VAR
VAR_OUTPUT
  count : INT;
  done : BOOL;
END_VAR
VAR
  parts : CTU;
END_VAR
(* Counts rising edges of pulse until it reaches target. *)
parts(CU := pulse, R := reset, PV := target);
count := parts.CV;
done := parts.Q;
END_PROGRAM
",
    },
    Example {
        name: "Traffic light",
        src: "PROGRAM traffic
VAR_INPUT
  run : BOOL := TRUE;
  green_time : TIME := T#3s;
END_VAR
VAR_OUTPUT
  red : BOOL;
  yellow : BOOL;
  green : BOOL;
END_VAR
VAR
  phase : INT;  (* 0 green, 1 yellow, 2 red *)
  dwell : TIME;
  t : TON;
END_VAR
CASE phase OF
  0: dwell := green_time;
  1: dwell := T#1s;
ELSE
  dwell := T#3s;
END_CASE;
t(IN := run AND NOT t.Q, PT := dwell);
IF t.Q THEN
  phase := (phase + 1) MOD 3;
END_IF;
green := run AND (phase = 0);
yellow := run AND (phase = 1);
red := NOT run OR (phase = 2);
END_PROGRAM
",
    },
    Example {
        name: "Line of stations (nested blocks)",
        src: "(* A line of two stations, each driving two motors: open the
   instances in the tree on the left, or double-click a block. *)
FUNCTION_BLOCK motor
VAR_INPUT
  run : BOOL;
  fault : BOOL;
END_VAR
VAR_OUTPUT
  running : BOOL;
  tripped : BOOL;
END_VAR
VAR
  start_delay : TON;
END_VAR
start_delay(IN := run AND NOT tripped, PT := T#300ms);
IF fault THEN
  tripped := TRUE;
ELSIF NOT run THEN
  tripped := FALSE;
END_IF;
running := start_delay.Q AND NOT tripped;
END_FUNCTION_BLOCK

FUNCTION_BLOCK station
VAR_INPUT
  enable : BOOL;
  part_present : BOOL;
  jam : BOOL;
END_VAR
VAR_OUTPUT
  busy : BOOL;
  alarm : BOOL;
END_VAR
VAR
  infeed : motor;
  outfeed : motor;
END_VAR
infeed(run := enable AND NOT part_present, fault := jam);
outfeed(run := enable AND part_present, fault := FALSE);
busy := infeed.running OR outfeed.running;
alarm := infeed.tripped;
END_FUNCTION_BLOCK

PROGRAM line
VAR_INPUT
  enable : BOOL := TRUE;
  part_at_1 : BOOL;
  part_at_2 : BOOL;
  jam_1 : BOOL;
END_VAR
VAR_OUTPUT
  station1_busy : BOOL;
  station2_busy : BOOL;
  alarm : BOOL;
END_VAR
VAR
  s1 : station;
  s2 : station;
END_VAR
s1(enable := enable, part_present := part_at_1, jam := jam_1);
s2(enable := enable, part_present := part_at_2, jam := FALSE);
station1_busy := s1.busy;
station2_busy := s2.busy;
alarm := s1.alarm OR s2.alarm;
END_PROGRAM
",
    },
];

/// The old pump-station machine (two pumps on a wet well, with a plant
/// model), as a project: the fixture for plant-model features.
pub const PUMP_STATION: &[(&str, &str)] = &[
    ("main.st", include_str!("fixtures/pump-station/main.st")),
    (
        "plant_model.st",
        include_str!("fixtures/pump-station/plant_model.st"),
    ),
    ("io.map", include_str!("fixtures/pump-station/io.map")),
    (
        "analog_scale.st",
        include_str!("fixtures/pump-station/analog_scale.st"),
    ),
    (
        "duty_standby.st",
        include_str!("fixtures/pump-station/duty_standby.st"),
    ),
    (
        "latched_alarm.st",
        include_str!("fixtures/pump-station/latched_alarm.st"),
    ),
    (
        "motor_starter.st",
        include_str!("fixtures/pump-station/motor_starter.st"),
    ),
    (
        "stack_light.st",
        include_str!("fixtures/pump-station/stack_light.st"),
    ),
];
