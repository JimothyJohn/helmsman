(* A demo for the box UI: a line of two stations, each driving two motors,
   fed by parts that arrive on their own so the diagram moves with no I/O
   wired. Open the instances in the tree on the left, or double-click a
   block. A scanner can force station 1's infeed to jam on %IX0.0; the
   line's state is on %QX0.0..2.
   Deploy: scripts/pi-deploy-plc.sh user@host seeds a first install with it
   as project line, deployed. *)
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

FUNCTION_BLOCK feeder
VAR_INPUT
  period : TIME;
END_VAR
VAR_OUTPUT
  part : BOOL;
END_VAR
VAR
  clock : TON;
END_VAR
clock(IN := NOT clock.Q, PT := period);
IF clock.Q THEN
  part := NOT part;
END_IF;
END_FUNCTION_BLOCK

PROGRAM line
VAR
  feed1 : feeder;
  feed2 : feeder;
  s1 : station;
  s2 : station;
  jam_1 AT %IX0.0 : BOOL;
  station1_busy AT %QX0.0 : BOOL;
  station2_busy AT %QX0.1 : BOOL;
  alarm AT %QX0.2 : BOOL;
END_VAR
feed1(period := T#2s);
feed2(period := T#3s);
s1(enable := TRUE, part_present := feed1.part, jam := jam_1);
s2(enable := TRUE, part_present := feed2.part, jam := FALSE);
station1_busy := s1.busy;
station2_busy := s2.busy;
alarm := s1.alarm OR s2.alarm;
END_PROGRAM
