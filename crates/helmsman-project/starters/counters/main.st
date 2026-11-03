(* Template for production counters: how many parts a machine made, whether
   it is running, idle or down right now, the totals for this shift, and its
   uptime as a percentage. Save a copy, point the inputs at your sensor and
   your machine's run signal, and read the numbers on the screen tiles or
   from the box API. plant_model.st stands in for the machine until you wire
   real I/O; delete it and the lines marked PLANT when you do. Declare new
   inputs and outputs in io.map as well as here.

   E-stop and guard circuits belong in hardwired safety circuits; nothing here
   is a safety function. *)

(* A contact that must hold for `settle` before it counts, so a bouncing
   sensor does not count one part twice. *)
FUNCTION_BLOCK debounce
VAR_INPUT
  raw : BOOL;
  settle : TIME;
END_VAR
VAR_OUTPUT
  clean : BOOL;
END_VAR
VAR
  on_for : TON;
  off_for : TON;
END_VAR
on_for(IN := raw, PT := settle);
off_for(IN := NOT raw, PT := settle);
IF on_for.Q THEN
  clean := TRUE;
ELSIF off_for.Q THEN
  clean := FALSE;
END_IF;
END_FUNCTION_BLOCK

(* The counters themselves. A part is one rising edge of the debounced
   sensor. The machine is down when it is not running or has faulted, idle
   when it runs but no part has passed for idle_after, running otherwise.
   Every second goes to the account of the state the machine is in;
   uptime_pct is the running share of all of them. A change of shift_id
   starts a new shift's totals; reset clears everything. *)
FUNCTION_BLOCK production_counters
VAR_INPUT
  part_sensor : BOOL;    (* the part sensor, as wired *)
  settle : TIME;         (* the sensor must hold this long to count *)
  running : BOOL;        (* the machine runs: a contactor, a drive's run bit *)
  faulted : BOOL;        (* a trip or an alarm: down even if it still runs *)
  shift_id : INT;        (* which shift it is; a change begins a new shift *)
  idle_after : TIME;     (* running this long with no part is idle *)
  reset : BOOL;          (* clears every total *)
END_VAR
VAR_OUTPUT
  cycles : DINT;         (* parts since reset *)
  shift_cycles : DINT;   (* parts this shift *)
  state : INT;           (* 0 down, 1 idle, 2 running *)
  run_s : DINT;          (* seconds running since reset *)
  idle_s : DINT;         (* seconds idle since reset *)
  down_s : DINT;         (* seconds down since reset *)
  shift_run_s : DINT;    (* seconds running this shift *)
  uptime_pct : REAL;     (* run_s as a share of all seconds, in percent *)
END_VAR
VAR
  sensor : debounce;
  part : R_TRIG;
  since_part : TON;
  second : TON;
  last_shift : INT;
  total_s : DINT;
END_VAR
sensor(raw := part_sensor, settle := settle);
part(CLK := sensor.clean);
IF part.Q THEN
  cycles := cycles + 1;
  shift_cycles := shift_cycles + 1;
END_IF;

(* Restarts on every part: Q means no part for idle_after. *)
since_part(IN := NOT part.Q, PT := idle_after);
IF faulted OR NOT running THEN
  state := 0;
ELSIF since_part.Q THEN
  state := 1;
ELSE
  state := 2;
END_IF;

(* A timer that restarts itself: Q is TRUE for one scan every second. *)
second(IN := NOT second.Q, PT := T#1s);
IF second.Q THEN
  CASE state OF
    2:
      run_s := run_s + 1;
      shift_run_s := shift_run_s + 1;
    1:
      idle_s := idle_s + 1;
  ELSE
    down_s := down_s + 1;
  END_CASE;
END_IF;
total_s := run_s + idle_s + down_s;
IF total_s > 0 THEN
  uptime_pct := 100.0 * DINT_TO_REAL(run_s) / DINT_TO_REAL(total_s);
ELSE
  uptime_pct := 0.0;
END_IF;

IF shift_id <> last_shift THEN
  last_shift := shift_id;
  shift_cycles := 0;
  shift_run_s := 0;
END_IF;
IF reset THEN
  cycles := 0;
  shift_cycles := 0;
  run_s := 0;
  idle_s := 0;
  down_s := 0;
  shift_run_s := 0;
  uptime_pct := 0.0;
END_IF;
END_FUNCTION_BLOCK

PROGRAM counters
VAR_INPUT
  start_pb : BOOL;
  stop_pb : BOOL;
  part_in : BOOL;       (* the part sensor *)
  fault_in : BOOL;      (* the machine's fault contact *)
  reset_pb : BOOL;      (* clears the totals *)
  new_shift_in : BOOL;  (* a pulse when a shift begins; see shift_id below *)
END_VAR
VAR_OUTPUT
  run_lamp : BOOL;
  idle_lamp : BOOL;
  down_lamp : BOOL;
  cycles : DINT;
  shift_cycles : DINT;
  state : INT;
  uptime_pct : REAL;
END_VAR
VAR
  plant : plant_model;  (* PLANT *)
  count : production_counters;
  shift_edge : R_TRIG;
  run : BOOL;
  part : BOOL;
  shift_id : INT;
END_VAR

run := (run OR start_pb) AND NOT stop_pb;
plant(running := run);  (* PLANT *)
part := part_in OR plant.part_present;  (* PLANT: keep the _in input *)

(* On a box with a calendar, delete new_shift_in and these two lines and
   declare `shift_id : INT;` under VAR_INPUT instead: the box's calendar
   provides it, and the counters see each shift begin on their own. *)
shift_edge(CLK := new_shift_in);
IF shift_edge.Q THEN
  shift_id := shift_id + 1;
END_IF;

count(part_sensor := part, settle := T#20ms, running := run, faulted := fault_in,
      shift_id := shift_id, idle_after := T#10s, reset := reset_pb);

run_lamp := count.state = 2;
idle_lamp := count.state = 1;
down_lamp := fault_in;
cycles := count.cycles;
shift_cycles := count.shift_cycles;
state := count.state;
uptime_pct := count.uptime_pct;
END_PROGRAM
