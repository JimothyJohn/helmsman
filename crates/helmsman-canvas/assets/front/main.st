PROGRAM tank
VAR_INPUT
  level_low : BOOL;
  level_high : BOOL;
  level : REAL := 1.0;    (* metres, tank is 2 m deep *)
  reset : BOOL;
END_VAR
VAR_OUTPUT
  fill : BOOL;            (* inlet valve *)
  alarm : BOOL;
  level_pct : REAL;       (* level for the operator's screen *)
END_VAR
VAR
  fill_time : TON;        (* how long the inlet has been open *)
  overrun_trip : BOOL;    (* a fill ran past 7 s: inlet shut, alarm up *)
  switches_disagree : BOOL; (* low and high both on: inlet shut, alarm up *)
  alarm_latch : BOOL;     (* alarm held until reset with the switches agreed *)
END_VAR

(* Level readout: 0 m -> 0 %, 2 m -> 100 %. *)
level_pct := LIMIT(0.0, level / 2.0 * 100.0, 100.0);

(* A fill takes under 6 s: one that runs 7 s trips the overrun. *)
fill_time(IN := fill, PT := T#7s);
IF fill_time.Q THEN
  overrun_trip := TRUE;
END_IF;
IF reset THEN
  overrun_trip := FALSE;
END_IF;

(* Both switches on at once: the level cannot be low and full, so
   close the inlet and alarm. *)
switches_disagree := level_low AND level_high;

(* Alarm hold: any cause sets the latch; only reset pressed while the
   switches agree clears it. *)
IF overrun_trip OR switches_disagree THEN
  alarm_latch := TRUE;
END_IF;
IF reset AND NOT switches_disagree THEN
  alarm_latch := FALSE;
END_IF;

(* Top-up: open the inlet when the tank runs low, close it when full,
   when a fill has run too long, when the switches disagree, or while
   the alarm is held. *)
fill := (level_low OR fill) AND NOT level_high
        AND NOT overrun_trip AND NOT switches_disagree
        AND NOT alarm_latch;

alarm := alarm_latch;
END_PROGRAM
