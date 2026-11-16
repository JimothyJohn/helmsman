 level or pressure sensor fault in the buffer tank |
| tank_press_ok | Internal | BOOL | Tank pressure within 1.6–2.8 bar |
| tank_low_warn | Internal | BOOL | Tank below the filling level (20 %, back at 30 %) in Automatic |
| tank_high_warn | Internal | BOOL | Tank high-level switch made |
| co2_slow_warn | Internal | BOOL | CO2 valve open 60 s without reaching 2.0 bar |
| inlet_slow_warn | Internal | BOOL | Beer inlet open 600 s without reaching 70 % |
| rinser_fault | Internal | BOOL | Latched: rinser water or air pressure lost |
| crowns_ok | Internal | BOOL | Crown chute has crowns (debounced) |
| crowns_low_warn | Internal | BOOL | Crown chute low in Automatic |
| filler_permit | Internal | BOOL | Filler drive may run |
| filler_empty | Internal | BOOL | No filler pocket holds a tracked bottle |
| filler_homed | Internal | BOOL | Filler position and pocket tracking are valid |
| filler_producing | Internal | BOOL | Filler producing with the gate open |
| filler_starved | Internal | BOOL | Filler stopped for lack of bottles |
| filler_fault | Internal | BOOL | Latched: pitch, home or broken-bottle fault |
| purge_req | Internal | BOOL | One-scan pulse: reject the next 20 bottles at inspection |
| broken_pulse | Internal | BOOL | One-scan pulse: a loaded bottle was missing at filler discharge |
| good_pulse | Internal | BOOL | One-scan pulse: a good bottle passed the reject nozzle |
| reject_pulse | Internal | BOOL | One-scan pulse: a bottle was blasted off |
| bin_ok | Internal | BOOL | Reject bin not full (debounced) |
| bin_full_warn | Internal | BOOL | Reject bin full |
| track_warn | Internal | BOOL | A bottle reached the reject nozzle with no inspection result |
| reject_fault | Internal | BOOL | Latched: reject not confirmed or tracking overflow |
| good_count | Internal | DINT | Good bottles this shift |
| reject_count | Internal | DINT | Rejected bottles this shift |
| broken_count | Internal | DINT | Broken bottles in the filler this shift |
| cip_state | Internal | INT | CIP step number, see cip part |
| cip_active | Internal | BOOL | Mode is CIP |
| cip_valves_open | Internal | BOOL | CIP asks for all fill valves open |
| cip_needs_rinse | Internal | BOOL | Chemical has entered tank/filler and no final rinse has completed since |
| cip_fault | Internal | BOOL | Latched: any CIP fault |
| cip_running | Internal | BOOL | CIP in steps 2–8 |
| cip_complete | Internal | BOOL | CIP finished all steps |

## Parts

### main

Job: The one PROGRAM. It declares the machine's I/O, calls every part once per scan and wires their signals together.

Reads: all inputs and the outputs of every part.

Writes: all outputs, by copying them from the part that owns each one, and the shared signals any_fault, any_warning, any_waiting and any_motor_fb.

Interface:
```st interface
VAR_INPUT
  start_pb : BOOL; stop_pb : BOOL; reset_pb : BOOL;
  mode_sel_manual : BOOL; mode_sel_auto : BOOL; mode_sel_cip : BOOL;
  jog_pb : ARRAY[1..8] OF BOOL;
  shift_reset_pb : BOOL;
  safety_relay_ok : BOOL; filler_guard_closed : BOOL; crowner_guard_closed : BOOL;
  io_healthy : BOOL;
  section_run_fb : ARRAY[1..8] OF BOOL;
  section_end_eye : ARRAY[1..8] OF BOOL;
  rinse_water_ok : BOOL; rinse_air_ok : BOOL;
  tank_level_pct : REAL; tank_pressure_bar : REAL; tank_high_sw : BOOL;
  filler_pitch_pulse : BOOL; filler_home_sw : BOOL; filler_infeed_eye : BOOL;
  crown_chute_ok : BOOL;
  inspect_eye : BOOL; fill_level_ok : BOOL; crown_ok : BOOL;
  reject_eye : BOOL; reject_bin_eye : BOOL; reject_bin_full : BOOL;
  cip_flow_ok : BOOL; cip_return_temp_c : REAL; cip_cups_fitted : BOOL;
END_VAR
VAR_OUTPUT
  section_motor : ARRAY[1..8] OF BOOL;
  rinse_water_vlv : BOOL; rinse_air_vlv : BOOL;
  beer_inlet_vlv : BOOL; co2_inlet_vlv : BOOL; tank_vent_vlv : BOOL;
  filler_gate_open : BOOL;
  fill_vlv : ARRAY[1..8] OF BOOL;
  reject_blast_vlv : BOOL;
  cip_pump_run : BOOL; cip_water_vlv : BOOL; cip_caustic_vlv : BOOL; cip_acid_vlv : BOOL;
  cip_return_vlv : BOOL; cip_drain_vlv : BOOL; cip_heater_on : BOOL;
  stack_red : BOOL; stack_amber : BOOL; stack_green : BOOL; horn : BOOL;
END_VAR
VAR
  mm : mode_manager;
  sect : ARRAY[1..8] OF conveyor_section;
  ann : annunciator;
  tank : buffer_tank;
  rins : rinser;
  crwn : crowner;
  cnt : shift_counter;
  fill : filler;
  insp : inspection;
  cipc : cip;
  any_fault : BOOL := FALSE;
  any_warning : BOOL := FALSE;
  any_waiting : BOOL := FALSE;
  any_motor_fb : BOOL := FALSE;
  section_fault : ARRAY[1..8] OF BOOL;
  i : INT;
END_VAR
```

Rules:
1. Each scan, main first computes any_fault, any_warning, any_waiting and any_motor_fb from the part outputs of the previous scan. It then calls the parts in this order: mm, tank, crwn, insp, fill, sect[1] to sect[8], rins, cnt, cipc, ann.
2. any_fault is the OR of:
   - mm.safety_fault and mm.io_fault;
   - every sect[i].fault;
   - tank.tank_fault, rins.rinser_fault, fill.fault, insp.reject_fault and cipc.fault.
3. any_warning is the OR of:
   - mm.selector_invalid;
   - tank.tank_low_warn, tank.tank_high_warn, tank.co2_slow_warn and tank.inlet_slow_warn;
   - crwn.crowns_low_warn, insp.bin_full_warn and insp.track_warn;
   - every sect[i].waiting_long.
4. any_waiting is TRUE when any sect[i].waiting is TRUE or fill.starved is TRUE.
5. any_motor_fb is the OR of section_run_fb[1..8].
6. Section i is called with:
   - run_enable := mm.sect_enable[i], jog_cmd := mm.jog_cmd[i], reset_req := mm.reset_req;
   - end_eye := section_end_eye[i], run_fb := section_run_fb[i].
7. section_motor[i] := sect[i].motor, and section_fault[i] := sect[i].fault.
8. downstream_running follows the next section, with two exceptions:
   - Sections 1, 2 and 4 to 7: downstream_running := section_run_fb[i+1].
   - Section 3: downstream_running := section_run_fb[4] AND fill.gate_open.
   - Section 8: downstream_running := FALSE, because the packing station is never "running".
9. upstream_bottle is the signal that shows a bottle arriving at section i:

   | Section | upstream_bottle |
   |---|---|
   | 1 | FALSE |
   | 2 | section_end_eye[1] |
   | 3 | section_end_eye[2] |
   | 4 | section_end_eye[3] |
   | 5 | section_end_eye[4] |
   | 6 | insp.good_pulse |
   | 7 | section_end_eye[6] |
   | 8 | section_end_eye[7] |

10. Section 1 is called with check_eye_clear := FALSE. Section 8 is called with check_jam := FALSE and block_time := T#3S. Every other section keeps the defaults.
11. Section 4 is called with permit := fill.permit. Every other section keeps permit := TRUE.
12. Outputs are copied from the part that owns them:
    - fill_vlv and filler_gate_open from fill;
    - beer_inlet_vlv, co2_inlet_vlv and tank_vent_vlv from tank;
    - rinse_water_vlv and rinse_air_vlv from rins;
    - reject_blast_vlv from insp;
    - all cip_* valve, pump and heater outputs from cipc;
    - stack_red, stack_amber, stack_green and horn from ann.
13. Until the build step that creates a part, main passes these fixed values in its place:

    | Signal | Value until built |
    |---|---|
    | mm.filler_empty | TRUE |
    | mm.cip_needs_rinse | FALSE |
    | fill.bin_ok | TRUE |
    | fill.crowns_ok | TRUE |
    | tank cip_active | mode = 3 |

### mode_manager

Job: It decodes the mode selector, latches the safety and I/O faults, runs the automatic line start, stop and run-out sequence, and turns jog buttons into jog commands.

Reads: start_pb, stop_pb, reset_pb, mode_sel_manual, mode_sel_auto, mode_sel_cip, safety_relay_ok, filler_guard_closed, crowner_guard_closed, io_healthy, jog_pb, any_fault, filler_empty, cip_needs_rinse, any_motor_fb.

Writes: mode, line_state, sect_enable, jog_cmd, feed_stop, start_edge, stop_edge, reset_req, power_up_pending, safety_fault, io_fault, horn_req, selector_invalid.

Interface:
```st interface
VAR_INPUT
  start_pb : BOOL; stop_pb : BOOL; reset_pb : BOOL;
  mode_sel_manual : BOOL; mode_sel_auto : BOOL; mode_sel_cip : BOOL;
  safety_relay_ok : BOOL; filler_guard_closed : BOOL; crowner_guard_closed : BOOL;
  io_healthy : BOOL;
  jog_pb : ARRAY[1..8] OF BOOL;
  any_fault : BOOL;
  filler_empty : BOOL := TRUE;
  cip_needs_rinse : BOOL := FALSE;
  any_motor_fb : BOOL;
END_VAR
VAR_OUTPUT
  mode : INT := 0;
  line_state : INT := 0;
  sect_enable : ARRAY[1..8] OF BOOL;
  jog_cmd : ARRAY[1..8] OF BOOL;
  feed_stop : BOOL := FALSE;
  start_edge : BOOL; stop_edge : BOOL; reset_req : BOOL;
  power_up_pending : BOOL := TRUE;
  safety_fault : BOOL := FALSE;
  io_fault : BOOL := FALSE;
  horn_req : BOOL := FALSE;
  selector_invalid : BOOL := FALSE;
END_VAR
VAR
  raw_mode : INT := 0;
  raw_invalid : BOOL := FALSE;
  last_raw : INT := 0;
  sel_tmr : TON;
  start_trig : R_TRIG; stop_trig : R_TRIG; reset_trig : R_TRIG;
  warn_tmr : TON;
  stop_tmr : TON;
  runout_tmr : TON;
  moved_recent : TOF;
  jog_horn_tmr : TON;
  jog_allowed : BOOL := FALSE;
  jog_any : BOOL := FALSE;
  k : INT;
END_VAR
```

States (line_state, used only while mode = 2; in every other mode line_state is held at 0):

| No. | State | Outputs | Leaves when | Time limit |
|---|---|---|---|---|
| 0 | Stopped | sect_enable[1..8] FALSE, feed_stop FALSE, horn_req FALSE | start_edge with mode = 2, any_fault FALSE, power_up_pending FALSE, cip_needs_rinse FALSE → 1 | none, waits for the operator |
| 1 | Warning | sect_enable all FALSE, horn_req TRUE | warn_tmr 5 s done → 2; stop_edge or any_fault → 0 | 5 s |
| 2 | Running | sect_enable[1..8] TRUE, feed_stop FALSE | stop_edge → 3; any_fault → 0 | none, production |
| 3 | Stopping | sect_enable[1..3] FALSE, sect_enable[4..8] TRUE, feed_stop TRUE | filler_empty TRUE → 4; stop_tmr 30 s → 4; start_edge → 2; any_fault → 0 | 30 s, longer than the 8 pitches × 1.5 s needed to empty the filler, with margin |
| 4 | Run-out | sect_enable[1..4] FALSE, sect_enable[5..8] TRUE, feed_stop TRUE | runout_tmr 5 s → 0; start_edge → 2; any_fault → 0 | 5 s, long enough to clear the last bottles out of the crowner |

Rules:
1. start_edge, stop_edge and reset_req are one-scan pulses on the rising edge of start_pb, stop_pb and reset_pb.
2. The selector is decoded into raw_mode:
   - Exactly one of mode_sel_manual, mode_sel_auto, mode_sel_cip TRUE gives 1, 2 or 3.
   - None TRUE gives 0.
   - More than one TRUE gives 0 and sets raw_invalid.
3. mode and selector_invalid take raw_mode and raw_invalid only after raw_mode has been unchanged for 500 ms. This rides over selector contact bounce.
4. Whenever mode is not 2, line_state is 0 and every sect_enable is FALSE. Leaving Automatic while running is therefore an immediate stop.
5. safety_fault latches TRUE whenever safety_relay_ok, filler_guard_closed or crowner_guard_closed is FALSE, in every mode. reset_req clears it only when all three are TRUE.
6. io_fault latches TRUE whenever io_healthy is FALSE. reset_req clears it only when io_healthy is TRUE.
7. power_up_pending is TRUE from the first scan after power returns. reset_req clears it when safety_relay_ok, both guards and io_healthy are TRUE.
8. When any_fault becomes TRUE, line_state goes to 0 on the same scan.
9. Before moving to 4, Stopping waits for filler_empty TRUE.
10. moved_recent is a TOF of 10 s on any_motor_fb, so it stays TRUE for 10 s after the last motor feedback.
11. jog_any is TRUE when any jog_pb[k] is TRUE. Jogging is only possible in mode 1 with any_fault FALSE and power_up_pending FALSE.
12. In mode 1 with jog_any TRUE and moved_recent FALSE, horn_req is TRUE and jog_horn_tmr runs. When it reaches 3 s, jog_allowed is set.
13. In mode 1 with jog_any TRUE and moved_recent TRUE, jog_allowed is set at once and the horn does not sound.
14. jog_allowed is reset when jog_any is FALSE, when mode is not 1, or when any_fault is TRUE.
15. jog_cmd[k] := jog_allowed AND jog_pb[k]. In every mode other than 1, every jog_cmd is FALSE.
16. horn_req is TRUE only in line_state 1 and during the 3 s jog warning.

### annunciator

Job: It drives the stack light and the horn from the line's mode, state, faults and warnings.

Reads: mode, line_state, any_fault, power_up_pending, any_warning, any_waiting, filler_producing, cip_running, cip_complete, horn_req.

Writes: stack_red, stack_amber, stack_green, horn.

Interface:
```st interface
VAR_INPUT
  mode : INT; line_state : INT;
  any_fault : BOOL; power_up_pending : BOOL; any_warning : BOOL; any_waiting : BOOL;
  filler_producing : BOOL; cip_running : BOOL; cip_complete : BOOL;
  horn_req : BOOL;
END_VAR
VAR_OUTPUT
  stack_red : BOOL; stack_amber : BOOL; stack_green : BOOL; horn : BOOL;
END_VAR
VAR
  red_blink : BLINK_PATTERN; amber_blink : BLINK_PATTERN; green_blink : BLINK_PATTERN;
  red_mode : INT := 0; amber_mode : INT := 0; green_mode : INT := 0;
  fault_trig : R_TRIG;
  fault_horn : TP;
END_VAR
```

Rules:
1. All three BLINK_PATTERN instances are called with EN := TRUE and PERIOD := T#1S. Each lamp output is the Q of its instance.
2. red_mode is set in this order of priority:
   - 3 (fast) while any_fault is TRUE;
   - otherwise 2 (slow) while power_up_pending is TRUE;
   - otherwise 0.
3. amber_mode is set in this order of priority:
   - 1 (steady) while any_warning is TRUE;
   - otherwise 2 (slow) while mode = 1;
   - otherwise 0.
4. green_mode is set by the first row that applies:

   | Condition | green_mode |
   |---|---|
   | mode = 2, line_state = 2, filler_producing TRUE, any_waiting FALSE | 1 (steady) |
   | mode = 2 and line_state is 1, 2, 3 or 4 | 2 (slow) |
   | mode = 3 and cip_running TRUE | 2 (slow) |
   | mode = 3 and cip_complete TRUE | 4 (double flash) |
   | anything else | 0 |

5. fault_horn is a TP of 3 s, started by the rising edge of any_fault.
6. horn := horn_req OR fault_horn.Q.

### conveyor_section

Job: It runs one conveyor section's motor. It stops the section when bottles back up at its end against a stopped next section, restarts it when the next section runs again, and detects motor and eye faults. There are eight instances, sect[1..8].

Reads: run_enable, permit, downstream_running, end_eye, run_fb, jog_cmd, reset_req, upstream_bottle, block_time, check_eye_clear, check_jam.

Writes: motor, state, fault, fb_fault, fb_stuck_fault, jam_fault, eye_clear_fault, waiting, waiting_long.

Interface:
```st interface
VAR_INPUT
  run_enable : BOOL;
  permit : BOOL := TRUE;
  downstream_running : BOOL;
  end_eye : BOOL;
  run_fb : BOOL;
  jog_cmd : BOOL;
  reset_req : BOOL;
  upstream_bottle : BOOL;
  block_time : TIME := T#500MS;
  check_eye_clear : BOOL := TRUE;
  check_jam : BOOL := TRUE;
END_VAR
VAR_OUTPUT
  motor : BOOL := FALSE;
  state : INT := 0;
  fault : BOOL := FALSE;
  fb_fault : BOOL := FALSE;
  fb_stuck_fault : BOOL := FALSE;
  jam_fault : BOOL := FALSE;
  eye_clear_fault : BOOL := FALSE;
  waiting : BOOL := FALSE;
  waiting_long : BOOL := FALSE;
END_VAR
VAR
  block_tmr : TON;
  restart_tmr : TON;
  clear_tmr : TON;
  fb_tmr : TON;
  stuck_tmr : TON;
  jam_tmr : TON;
  wait_tmr : TON;
  eye_trig : R_TRIG;
  up_trig : R_TRIG;
  bottles_unseen : INT := 0;
END_VAR
```

States:

| No. | State | Outputs | Leaves when | Time limit |
|---|---|---|---|---|
| 0 | Off | motor FALSE | fault → 9. jog_cmd → 4. run_enable AND permit → 2 if end_eye TRUE and downstream_running FALSE, else → 1 | none, idle |
| 1 | Running | motor TRUE | fault → 9. run_enable FALSE → 0. permit FALSE → 3. end_eye blocked continuously for block_time with downstream_running FALSE → 2 | none, production |
| 2 | Blocked | motor FALSE, waiting TRUE | fault → 9. run_enable FALSE → 0. restart_tmr (downstream_running TRUE for 1 s) or clear_tmr (end_eye FALSE for 1 s) done, with permit TRUE → 1 | none, because the next section decides; waiting_long warns after 300 s |
| 3 | Held | motor FALSE, waiting TRUE | fault → 9. run_enable FALSE → 0. permit TRUE → 1, or → 2 if end_eye TRUE and downstream_running FALSE | none, because the part owning permit times its own condition; waiting_long warns after 300 s |
| 4 | Jog | motor TRUE | jog_cmd FALSE → 0. fault → 9 | none, hold-to-run |
| 9 | Fault | motor FALSE | reset_req with every fault cause gone → 0 | none, the operator resets |

Rules:
1. fb_fault latches when motor is TRUE and run_fb is FALSE for 1.0 s, which covers both failing to start and tripping while running.
2. fb_stuck_fault latches when motor is FALSE and run_fb is TRUE for 2.0 s, which means a welded contactor or a stuck contact.
3. jam_fault latches when check_jam is TRUE, state is 1, end_eye is TRUE and downstream_running is TRUE, all continuously for 10 s. Bottles should leave in well under 10 s, so this means a jam or an eye stuck blocked.
4. bottles_unseen counts like this:
   - It goes up by 1 on each rising edge of upstream_bottle while run_enable is TRUE.
   - It returns to 0 on each rising edge of end_eye and on reset_req.
5. eye_clear_fault latches when check_eye_clear is TRUE and bottles_unseen reaches 30. That means 30 bottles went in and none were seen at the end: the eye is stuck clear, or bottles are being lost.
6. fault := fb_fault OR fb_stuck_fault OR jam_fault OR eye_clear_fault.
7. reset_req clears faults as follows:
   - fb_stuck_fault clears only when run_fb is FALSE.
   - fb_fault, jam_fault and eye_clear_fault are cleared by reset_req.
8. waiting_long becomes TRUE when state has been 2 or 3 for 300 s, and FALSE on leaving those states.
9. Every timer that is not used in the current state is called with IN := FALSE, so it starts from zero on its next use.
10. Because each blocked section restarts only 1 s after the next one's feedback, a cascade of stops clears from the downstream end first and restarts in reverse order.

### buffer_tank

Job: It holds the buffer tank between its levels with the beer inlet, holds CO2 counter-pressure, tells the filler whether filling is allowed, and detects tank sensor faults.

Reads: mode, cip_active, io_fault, tank_level_pct, tank_pressure_bar, tank_high_sw, reset_req.

Writes: beer_inlet_vlv, co2_inlet_vlv, tank_vent_vlv, tank_fill_ok, tank_press_ok, tank_low_warn, tank_high_warn, co2_slow_warn, inlet_slow_warn, tank_fault.

Interface:
```st interface
VAR_INPUT
  mode : INT;
  cip_active : BOOL;
  io_fault : BOOL;
  tank_level_pct : REAL;
  tank_pressure_bar : REAL;
  tank_high_sw : BOOL;
  reset_req : BOOL;
END_VAR
VAR_OUTPUT
  beer_inlet_vlv : BOOL := FALSE;
  co2_inlet_vlv : BOOL := FALSE;
  tank_vent_vlv : BOOL := FALSE;
  tank_fill_ok : BOOL := FALSE;
  tank_press_ok : BOOL := FALSE;
  tank_low_warn : BOOL := FALSE;
  tank_high_warn : BOOL := FALSE;
  co2_slow_warn : BOOL := FALSE;
  inlet_slow_warn : BOOL := FALSE;
  tank_fault : BOOL := FALSE;
  level_fault : BOOL := FALSE;
  press_fault : BOOL := FALSE;
END_VAR
VAR
  level_ok_hys : BOOL := FALSE;
  lvl_range_tmr : TON;
  lvl_hi_tmr : TON;
  prs_range_tmr : TON;
  prs_in_tmr : TON;
  prs_out_tmr : TON;
  co2_tmr : TON;
  inlet_tmr : TON;
END_VAR
```

Rules:
1. level_fault latches in either case:
   - tank_level_pct is below −5 or above 105 for 1 s;
   - tank_high_sw is TRUE while tank_level_pct is below 80 for 5 s, which means the two level readings disagree.
2. press_fault latches when tank_pressure_bar is below −0.5 or above 6.0 for 1 s.
3. tank_fault := level_fault OR press_fault. reset_req clears each one only when its cause is gone.
4. tank_press_ok is set and cleared with 0.5 s filters:
   - It becomes TRUE when pressure has been within 1.6 to 2.8 bar for 0.5 s.
   - It becomes FALSE when pressure has been outside that band for 0.5 s.
   - It is FALSE while press_fault is TRUE.
5. When cip_active is FALSE and press_fault is FALSE, the CO2 and vent valves control pressure in every mode, so beer in the tank is never left without counter-pressure:

   | Valve | Opens when | Closes when |
   |---|---|---|
   | co2_inlet_vlv | pressure < 1.8 bar | pressure ≥ 2.0 bar |
   | tank_vent_vlv | pressure > 2.6 bar | pressure ≤ 2.4 bar |

6. While press_fault is TRUE, co2_inlet_vlv and tank_vent_vlv are both closed. The mechanical relief valve protects the tank (see Open questions).
7. While cip_active is TRUE, co2_inlet_vlv is closed and tank_vent_vlv is open, so the tank is at atmosphere for cleaning.
8. beer_inlet_vlv may open only when all of these hold: mode = 2, cip_active FALSE, tank_fault FALSE, io_fault FALSE, tank_press_ok TRUE and tank_high_sw FALSE. When they hold:
   - It opens when tank_level_pct < 40.
   - It closes when tank_level_pct ≥ 70.
   - Between 40 and 70 it keeps its last state.
   - When any of the conditions fails, it is closed.
9. level_ok_hys becomes TRUE when level ≥ 30 % and FALSE when level < 20 %.
10. tank_fill_ok := level_ok_hys AND tank_press_ok AND NOT tank_fault AND NOT cip_active.
11. The warnings are:

    | Warning | TRUE when |
    |---|---|
    | tank_low_warn | mode = 2 and level_ok_hys FALSE |
    | tank_high_warn | tank_high_sw TRUE |
    | co2_slow_warn | co2_inlet_vlv open continuously for 60 s; clears when it closes |
    | inlet_slow_warn | beer_inlet_vlv open continuously for 600 s; clears when it closes |

### rinser

Job: It runs the rinser water and air jets while the rinser section runs in Automatic, and stops the line if either supply is lost.

Reads: mode, rinser_run_fb (= section_run_fb[3]), rinse_water_ok, rinse_air_ok, reset_req.

Writes: rinse_water_vlv, rinse_air_vlv, rinser_fault.

Interface:
```st interface
VAR_INPUT
  mode : INT;
  rinser_run_fb : BOOL;
  rinse_water_ok : BOOL;
  rinse_air_ok : BOOL;
  reset_req : BOOL;
END_VAR
VAR_OUTPUT
  rinse_water_vlv : BOOL := FALSE;
  rinse_air_vlv : BOOL := FALSE;
  rinser_fault : BOOL := FALSE;
END_VAR
VAR
  water_tmr : TON;
  air_tmr : TON;
END_VAR
```

Rules:
1. rinse_water_vlv and rinse_air_vlv are both TRUE when mode = 2, rinser_run_fb is TRUE and rinser_fault is FALSE. Otherwise both are FALSE.
2. rinser_fault latches in either case:
   - rinse_water_vlv is TRUE and rinse_water_ok is FALSE for 3 s;
   - rinse_air_vlv is TRUE and rinse_air_ok is FALSE for 3 s.

   The 3 s allows for the pressure to build after the valve opens.
3. reset_req clears rinser_fault. If the supply is still missing, the fault returns 3 s after the next start.

### crowner

Job: It watches the crown chute, and closes the filler feed when crowns run low so that no bottle is filled without a crown to put on it.

Reads: mode, crown_chute_ok.

Writes: crowns_ok, crowns_low_warn.

Interface:
```st interface
VAR_INPUT
  mode : INT;
  crown_chute_ok : BOOL;
END_VAR
VAR_OUTPUT
  crowns_ok : BOOL := TRUE;
  crowns_low_warn : BOOL := FALSE;
END_VAR
VAR
  low_tmr : TON;
  ok_tmr : TON;
END_VAR
```

Rules:
1. crowns_ok is debounced in both directions:
   - It becomes FALSE when crown_chute_ok has been FALSE for 2 s.
   - It becomes TRUE when crown_chute_ok has been TRUE for 2 s.

   The low switch sits above empty, so the crowns left in the chute finish the bottles already in the filler.
2. crowns_low_warn := (mode = 2) AND NOT crowns_ok.

### shift_counter

Job: It keeps the shift counts of good, rejected and broken bottles.

Reads: good_pulse, reject_pulse, broken_pulse, shift_reset_pb.

Writes: good_count, reject_count, broken_count.

Interface:
```st interface
VAR_INPUT
  good_pulse : BOOL;
  reject_pulse : BOOL;
  broken_pulse : BOOL;
  shift_reset_pb : BOOL;
END_VAR
VAR_OUTPUT
  good_count : DINT := 0;
  reject_count : DINT := 0;
  broken_count : DINT := 0;
END_VAR
VAR
  good_trig : R_TRIG; reject_trig : R_TRIG; broken_trig : R_TRIG;
  reset_tmr : TON;
  reset_done : BOOL := FALSE;
END_VAR
```

Rules:
1. Each rising edge of good_pulse, reject_pulse or broken_pulse adds 1 to its count.
2. When shift_reset_pb has been held for 2 s, all three counts are set to 0 once. The 2 s hold stops a brush against the button from clearing the shift.
3. reset_done stops the reset repeating while the button stays held. It clears when the button is released.
4. Modes, faults and line stops never change the counts.
5. All counts start at 0 after a power loss, because nothing is retained (see Open questions).

### filler

Job: It runs the rotary filler's position tracking, infeed gate and eight fill valves, homes and purges the filler after its position has been lost, and detects broken bottles and position faults.

Reads: mode, line_state, feed_stop, filler_pitch_pulse, filler_home_sw, filler_infeed_eye, section_end_eye[4] (discharge eye), section_run_fb[4], tank_fill_ok, crowns_ok, bin_ok, cip_valves_open, reset_req.

Writes: fill_vlv, filler_gate_open, filler_permit, filler_empty, filler_homed, filler_producing, filler_starved, purge_req, broken_pulse, filler_fault.

Interface:
```st interface
VAR_INPUT
  mode : INT;
  line_state : INT;
  feed_stop : BOOL;
  pitch_pulse : BOOL;
  home_sw : BOOL;
  infeed_eye : BOOL;
  discharge_eye : BOOL;
  run_fb : BOOL;
  tank_fill_ok : BOOL;
  crowns_ok : BOOL := TRUE;
  bin_ok : BOOL := TRUE;
  cip_valves_open : BOOL;
  reset_req : BOOL;
END_VAR
VAR_OUTPUT
  fill_vlv : ARRAY[1..8] OF BOOL;
  gate_open : BOOL := FALSE;
  permit : BOOL := FALSE;
  empty : BOOL := TRUE;
  homed : BOOL := FALSE;
  producing : BOOL := FALSE;
  starved : BOOL := FALSE;
  purge_req : BOOL := FALSE;
  broken_pulse : BOOL := FALSE;
  fault : BOOL := FALSE;
  pitch_fault : BOOL := FALSE;
  home_fault : BOOL := FALSE;
  broken_fault : BOOL := FALSE;
  state : INT := 0;
END_VAR
VAR
  pitch_idx : INT := 0;
  next_idx : INT := 0;
  slot_loaded : ARRAY[1..8] OF BOOL;
  slot_age : ARRAY[1..8] OF INT;
  since_home : INT := 0;
  purge_pitches : INT := 0;
  broken_run : INT := 0;
  feed_ok : BOOL := FALSE;
  pitch_trig : R_TRIG;
  pitch_tmr : TON;
  starve_tmr : TON;
  unstarve_tmr : TON;
  k : INT;
END_VAR
```

States:

| No. | State | Outputs | Leaves when | Time limit |
|---|---|---|---|---|
| 0 | Idle | permit FALSE, gate_open FALSE, fill_vlv all FALSE (except during CIP, rule 9) | mode = 2 and line_state = 2: → 1 if homed FALSE, else → 3. mode = 2, line_state = 3 and homed TRUE → 3 | none, idle |
| 1 | Homing | permit TRUE, gate_open FALSE, fill_vlv all FALSE | a pitch pulse with home_sw TRUE → 2 (purge_pitches := 0). line_state not 2 or 3, or mode not 2, → 0 | pitch_fault after 5 s without a pulse; home_fault after 10 pulses without home |
| 2 | Purge | permit TRUE, gate_open FALSE, fill_vlv all FALSE | 8 pitch pulses counted since home → 3 with homed := TRUE. line_state not 2 or 3, or mode not 2, → 0 | pitch_fault after 5 s without a pulse |
| 3 | Producing | gate_open := feed_ok; permit := tank_fill_ok AND NOT starved; fill_vlv as rule 7 | line_state not 2 or 3, or mode not 2, or fault → 0 (pockets keep their tracking) | pitch_fault after 5 s without a pulse while run_fb |

Rules:
1. feed_ok := NOT feed_stop AND crowns_ok AND bin_ok AND tank_fill_ok.
2. On the first scan after power-up, and whenever mode is 1 or 3, the tracking is thrown away:
   - homed := FALSE;
   - every slot_loaded := FALSE and every slot_age := 0.
3. On entering state 1, purge_req is TRUE for one scan and every slot_loaded is set FALSE.
4. On each rising edge of pitch_pulse, in any state, these steps run in this order:
   - a. If home_sw is TRUE: when state is 3 and (pitch_idx + 1) MOD 8 is not 0, latch home_fault (the position is lost). Then pitch_idx := 0 and since_home := 0.
   - b. If home_sw is FALSE: pitch_idx := (pitch_idx + 1) MOD 8 and since_home := since_home + 1. If since_home exceeds 10 while state is 1, 2 or 3, latch home_fault.
   - c. For each k from 1 to 8 with slot_loaded[k] TRUE, slot_age[k] := slot_age[k] + 1.
   - d. For each k whose slot_age[k] just became 7 (the discharge position):
     - If discharge_eye is TRUE, broken_run := 0.
     - If discharge_eye is FALSE, broken_pulse is TRUE for one scan and broken_run := broken_run + 1.
     - In both cases slot_loaded[k] := FALSE.
   - e. If broken_run reaches 3, latch broken_fault.
   - f. In state 2, purge_pitches := purge_pitches + 1.
   - g. In state 3, with gate_open TRUE and infeed_eye TRUE, the pocket at the infeed takes the bottle: next_idx := pitch_idx + 1, slot_loaded[next_idx] := TRUE, slot_age[next_idx] := 0.
5. pitch_fault latches when run_fb is TRUE and no rising edge of pitch_pulse has come for 5 s. One pitch takes 1.5 s at 40 bottles a minute.
6. fault := pitch_fault OR home_fault OR broken_fault.
   - Any of them sets homed := FALSE and clears every slot_loaded.
   - reset_req clears all three and sets broken_run := 0.
7. fill_vlv[k] := state = 3 AND slot_loaded[k] AND slot_age[k] ≥ 1 AND slot_age[k] ≤ 5 AND run_fb AND tank_fill_ok.
   - The valve is open through the five fill pitches, about 7.5 s at full speed.
   - It is closed at age 6 for snift.
   - Fill height is set by the vent tube.
8. A pocket that does not hold a bottle never has its valve opened.
9. In mode 3, fill_vlv[1..8] := cip_valves_open.
10. starved works only in state 3:
    - It becomes TRUE when no slot_loaded is TRUE and infeed_eye has been FALSE for 3 s.
    - It becomes FALSE when infeed_eye has been TRUE for 0.5 s.
    - Outside state 3 it is FALSE.
11. empty := no slot_loaded TRUE.
12. producing := state = 3 AND gate_open AND NOT starved.
13. A fault stop that is not a filler fault leaves homed and the pocket tracking intact. On the next start the filler goes straight to state 3 and the bottles in it carry on from their age.

### inspection

Job: It judges each bottle at the fill-level and crown head, follows each result to the reject nozzle, blasts bad bottles into the bin and confirms they went in, and makes the purge after a filler homing.

Reads: mode, inspect_eye, fill_level_ok, crown_ok, reject_eye, reject_bin_eye, reject_bin_full, purge_req, reset_req.

Writes: reject_blast_vlv, good_pulse, reject_pulse, bin_ok, bin_full_warn, track_warn, reject_fault.

Interface:
```st interface
VAR_INPUT
  mode : INT;
  inspect_eye : BOOL;
  fill_level_ok : BOOL;
  crown_ok : BOOL;
  reject_eye : BOOL;
  reject_bin_eye : BOOL;
  reject_bin_full : BOOL;
  purge_req : BOOL;
  reset_req : BOOL;
END_VAR
VAR_OUTPUT
  reject_blast_vlv : BOOL := FALSE;
  good_pulse : BOOL := FALSE;
  reject_pulse : BOOL := FALSE;
  bin_ok : BOOL := TRUE;
  bin_full_warn : BOOL := FALSE;
  track_warn : BOOL := FALSE;
  reject_fault : BOOL := FALSE;
  confirm_fault : BOOL := FALSE;
  overflow_fault : BOOL := FALSE;
  purge_left : INT := 0;
END_VAR
VAR
  fifo : ARRAY[0..15] OF BOOL;
  fifo_head : INT := 0;
  fifo_count : INT := 0;
  verdict : BOOL := FALSE;
  level_seen : BOOL := FALSE;
  crown_seen : BOOL := FALSE;
  insp_fall : F_TRIG;
  rej_rise : R_TRIG;
  bin_rise : R_TRIG;
  purge_trig : R_TRIG;
  blast_pending : BOOL := FALSE;
  blast_delay : TON;
  blast_pulse : TP;
  confirm_wait : BOOL := FALSE;
  confirm_tmr : TON;
  bin_full_tmr : TON;
  bin_clear_tmr : TON;
END_VAR
```

Rules:
1. Each rising edge of purge_req sets purge_left := 20. That covers 8 bottles in the filler and 12 between the filler discharge and the inspection head.
2. While inspect_eye is TRUE, level_seen is latched when fill_level_ok is TRUE and crown_seen when crown_ok is TRUE.
3. On each falling edge of inspect_eye, a result is pushed:
   - verdict := level_seen AND crown_seen AND (mode = 2) AND (purge_left = 0).
   - If purge_left > 0, it is decreased by 1.
   - If fifo_count < 16, verdict is pushed at index (fifo_head + fifo_count) MOD 16 and fifo_count increases by 1.
   - If fifo_count is already 16, overflow_fault latches.
   - level_seen and crown_seen are cleared.
4. On each rising edge of reject_eye, a result is taken:
   - If fifo_count > 0, verdict := fifo[fifo_head], fifo_head := (fifo_head + 1) MOD 16, fifo_count decreases by 1.
   - If fifo_count = 0, verdict := FALSE, and if purge_left = 0, track_warn latches.
5. Then, on the same edge:
   - If verdict is TRUE, good_pulse is TRUE for one scan.
   - If verdict is FALSE, reject_pulse is TRUE for one scan and blast_pending is set.
6. While blast_pending is TRUE, blast_delay runs for 20 ms. When it is done, it starts blast_pulse for 80 ms, sets confirm_wait and clears blast_pending. reject_blast_vlv := blast_pulse.Q.
7. The rising edge of reject_bin_eye clears confirm_wait.
8. confirm_fault latches when confirm_wait has been TRUE for 1.0 s.
9. reject_fault := confirm_fault OR overflow_fault. reset_req clears confirm_fault, overflow_fault, track_warn and confirm_wait.
10. On the first scan after power-up, fifo_count := 0 and fifo_head := 0.
11. The FIFO is not cleared on a mode change, so bottle order stays right across stops and jogs.
12. bin_ok is debounced in both directions:
    - It becomes FALSE when reject_bin_full has been TRUE for 2 s.
    - It becomes TRUE when reject_bin_full has been FALSE for 2 s.

    bin_full_warn := NOT bin_ok.
13. The inspection works in every mode. Outside mode 2 every result is bad, so bottles that were jogged are rejected.

### cip

Job: It runs the cleaning sequence through the buffer tank and the filler: caustic, rinse, acid, rinse and drain. Each chemical step counts time only at temperature.

Reads: mode, start_edge, stop_edge, reset_req, power_up_pending, safety_fault, cip_flow_ok, cip_return_temp_c, cip_cups_fitted, tank_level_pct.

Writes: cip_pump_run, cip_water_vlv, cip_caustic_vlv, cip_acid_vlv, cip_return_vlv, cip_drain_vlv, cip_heater_on, cip_state, cip_active, cip_valves_open, cip_needs_rinse, cip_running, cip_complete, cip_fault.

Interface:
```st interface
VAR_INPUT
  mode : INT;
  start_edge : BOOL;
  stop_edge : BOOL;
  reset_req : BOOL;
  power_up_pending : BOOL;
  safety_fault : BOOL;
  flow_ok : BOOL;
  return_temp_c : REAL;
  cups_fitted : BOOL;
  tank_level_pct : REAL;
END_VAR
VAR_OUTPUT
  pump_run : BOOL := FALSE;
  water_vlv : BOOL := FALSE;
  caustic_vlv : BOOL := FALSE;
  acid_vlv : BOOL := FALSE;
  return_vlv : BOOL := FALSE;
  drain_vlv : BOOL := FALSE;
  heater_on : BOOL := FALSE;
  state : INT := 0;
  active : BOOL := FALSE;
  valves_open : BOOL := FALSE;
  needs_rinse : BOOL := FALSE;
  running : BOOL := FALSE;
  complete : BOOL := FALSE;
  fault : BOOL := FALSE;
  flow_fault : BOOL := FALSE;
  heat_fault : BOOL := FALSE;
  temp_fault : BOOL := FALSE;
  sensor_fault : BOOL := FALSE;
END_VAR
VAR
  held_state : INT := 0;
  step_acc : TIME := T#0S;
  step_tmr : TON;
  heat_tmr : TON;
  low_tmr : TON;
  flow_tmr : TON;
  sensor_tmr : TON;
  start_ok : BOOL := FALSE;
END_VAR
```

States:

| No. | State | Outputs (all others FALSE) | Leaves when | Time limit |
|---|---|---|---|---|
| 0 | Idle | none | mode = 3 → 1 | none |
| 1 | Ready | none | start_edge with start_ok → 2, step_acc := T#0S | none, waits for the operator |
| 2 | Caustic heat | pump_run, caustic_vlv, return_vlv, valves_open; heater_on on below 75 °C, off at 77 °C | return_temp_c ≥ 75 → 3 | 30 min, then heat_fault → 20 |
| 3 | Caustic hold | as state 2 | step_acc ≥ 20 min → 4 (step_acc := T#0S) | temperature below 73 °C continuously for 10 min → temp_fault → 20 |
| 4 | Rinse 1 | pump_run, water_vlv, drain_vlv, valves_open | step_tmr 5 min → 5 | 5 min |
| 5 | Acid heat | pump_run, acid_vlv, return_vlv, valves_open; heater_on on below 60 °C, off at 62 °C | return_temp_c ≥ 60 → 6 | 30 min, then heat_fault → 20 |
| 6 | Acid hold | as state 5 | step_acc ≥ 15 min → 7 (step_acc := T#0S) | temperature below 58 °C continuously for 10 min → temp_fault → 20 |
| 7 | Final rinse | pump_run, water_vlv, drain_vlv, valves_open | step_tmr 10 min → 8, needs_rinse := FALSE | 10 min |
| 8 | Drain | drain_vlv | step_tmr 120 s → 9 | 120 s |
| 9 | Complete | complete TRUE | mode not 3 → 0 | none |
| 20 | Hold | all outputs off | start_edge with start_ok and fault FALSE → held_state, rewritten 3 → 2 and 6 → 5 so the temperature is proved again | none, the operator decides; the circuit is safe with the pump and heater off |

Rules:
1. start_ok := cups_fitted AND tank_level_pct < 2.0 AND NOT safety_fault AND NOT power_up_pending AND NOT fault. The tank must be empty of beer before chemical can go in.
2. In states 3 and 6, step_acc increases by T#10MS on each scan where return_temp_c is at or above the hold minimum (73 °C caustic, 58 °C acid). Below the minimum it stays where it is.
3. The 10 min low-temperature timer (low_tmr) runs while the temperature is below the hold minimum in states 3 and 6. It restarts from zero whenever the temperature recovers.
4. needs_rinse is set TRUE on entering state 2 or state 5. It is set FALSE only at the end of state 7.
5. flow_fault latches when pump_run is TRUE and flow_ok is FALSE for 15 s, which covers pump start-up.
6. sensor_fault latches when return_temp_c is below −10 or above 120 °C for 1 s.
7. These events send states 2 to 8 to state 20, saving the current state in held_state:
   - flow_fault, heat_fault, temp_fault or sensor_fault becoming TRUE;
   - stop_edge;
   - safety_fault TRUE.
8. step_acc keeps its value through Hold. step_tmr restarts from zero when a timed step (4, 7 or 8) is resumed.
9. When mode is not 3, from any state:
   - state := 0, held_state := 0 and step_acc := T#0S;
   - all outputs are off;
   - needs_rinse keeps its value.
10. fault := flow_fault OR heat_fault OR temp_fault OR sensor_fault. reset_req clears each one only when its cause is gone. sensor_fault needs the reading back in range; the others clear on reset.
11. The status outputs are:

    | Output | TRUE when |
    |---|---|
    | active | mode = 3 |
    | running | state is 2 to 8 |
    | valves_open | state is 2 to 7 |

## Faults

| Fault | Detected by | Reaction | Cleared by |
|---|---|---|---|
| Emergency stop or safety relay dropped | safety_relay_ok FALSE (mode_manager rule 5) | Safety relay removes motor power in hardware. Program: line_state 0, all motors off, gate closed, fill valves closed, red fast flash, horn 3 s; CIP goes to Hold | reset_pb with relay healthy, then start |
| Filler or crowner guard opened | filler_guard_closed or crowner_guard_closed FALSE | As above | reset_pb with both guards closed |
| Remote I/O lost | io_healthy FALSE | Line stops, beer inlet closed, red fast flash | reset_pb with io_healthy TRUE |
| Power returned | first scan | Everything off; red slow flash; start, jog and CIP blocked; on first start the filler homes and purges, and 20 bottles are rejected | reset_pb |
| Mode selector shows two positions | selector_invalid | Mode Off, amber steady | Selector in one position |
| Motor failed to start or tripped | sect[i].fb_fault: motor commanded, no feedback 1 s | Line stops, red fast flash | reset_pb |
| Contactor welded or feedback stuck | sect[i].fb_stuck_fault: feedback 2 s with motor off | Line stops, red fast flash | reset_pb once feedback is off |
| Jam or eye stuck blocked | sect[i].jam_fault: end eye blocked 10 s with both sections running | Line stops | reset_pb |
| Eye stuck clear or bottles lost | sect[i].eye_clear_fault: 30 bottles in, none seen at the end | Line stops | reset_pb |
| Section blocked or held 300 s | sect[i].waiting_long | Amber steady, section stays waiting | The section moves again |
| Tank level sensor out of range or disagreeing with the high switch | level_fault | Inlet closed, filling stopped, line stops | reset_pb with the sensor in range |
| Tank pressure sensor out of range | press_fault | CO2 and vent closed, filling stopped, line stops | reset_pb with the sensor in range |
| Tank below filling level | tank_fill_ok FALSE below 20 % | Fill valves close, filler stops, gate closes, amber | Level back to 30 % (automatic) |
| Tank pressure outside 1.6–2.8 bar | tank_press_ok FALSE | Fill valves close, filler stops, inlet closed | Pressure back in band for 0.5 s (automatic) |
| Tank high switch made | tank_high_warn | Inlet closed, amber | Switch clears |
| CO2 supply slow | co2_slow_warn: CO2 open 60 s | Amber; valve stays open | Pressure reaches 2.0 bar |
| Bright beer supply slow or empty | inlet_slow_warn: inlet open 600 s | Amber; valve stays open | Level reaches 70 % |
| Rinser water or air lost | rinser_fault: pressure switch off 3 s with valve open | Line stops | reset_pb |
| Crowns low | crowns_ok FALSE 2 s | Gate closes, the filler finishes its bottles and stops starved, amber | Chute refilled for 2 s (automatic) |
| Filler starved | filler.starved | Filler stops, green slow flash | Bottle at the infeed for 0.5 s (automatic) |
| Filler pitch pulse missing | pitch_fault: 5 s without a pulse while running | Line stops, homing required | reset_pb |
| Filler home missing or out of place | home_fault | Line stops, homing required | reset_pb |
| Three broken bottles in a row | broken_fault | Line stops, homing required | reset_pb |
| Single broken bottle | broken_pulse | broken_count + 1 | — |
| Bottle not into the reject bin | confirm_fault: no bin eye within 1 s of the blast | Line stops | reset_pb |
| Inspection tracking overflow | overflow_fault: 17th result pushed | Line stops | reset_pb |
| Bottle at the reject nozzle without a result | track_warn | Bottle rejected, amber | reset_pb |
| Reject bin full | bin_ok FALSE 2 s | Gate closes, amber | Bin emptied for 2 s |
| CIP temperature not reached in 30 min | heat_fault | CIP Hold: pump, heater and valves off | reset_pb, then start resumes |
| CIP temperature lost for 10 min | temp_fault | CIP Hold | reset_pb, then start |
| CIP flow lost | flow_fault: 15 s without flow with the pump on | CIP Hold | reset_pb, then start |
| CIP temperature sensor out of range | sensor_fault | CIP Hold | reset_pb with the reading in range |
| CIP aborted by leaving CIP mode | mode not 3 | All CIP outputs off; cip_needs_rinse stays and blocks automatic start | A CIP run through the final rinse |

## Requirements

1. On the first scan after power returns, every section_motor, every fill_vlv, filler_gate_open, beer_inlet_vlv and reject_blast_vlv is FALSE. stack_red flashes slowly until reset_pb is pressed with safety_relay_ok, both guards and io_healthy TRUE.
2. With power_up_pending TRUE, pressing start_pb in Automatic turns on no section_motor within 10 s.
3. With only mode_sel_auto TRUE for 0.5 s, mode becomes 2 within 0.6 s. Manual gives 1, CIP gives 3, and no position gives 0.
4. With two selector inputs TRUE, mode becomes 0 and selector_invalid becomes TRUE within 0.6 s.
5. In Automatic, stopped and with no faults, pressing start_pb turns horn on within 0.1 s, and no section_motor turns on during the next 4.9 s.
6. 5 s after start_pb, with all end eyes clear, all eight section_motor outputs are TRUE within 0.1 s, and horn is FALSE.
7. Pressing stop_pb during the 5 s warning turns horn off within 0.1 s, and no motor starts in the next 10 s.
8. Pressing stop_pb while running turns off section_motor[1..3] and filler_gate_open within 0.1 s.
9. During a stop, section_motor[4] stays on while the filler holds a tracked bottle. It turns off within 0.1 s of the filler becoming empty, and section_motor[5..8] turn off 5 s (±0.1 s) later.
10. If the filler is not empty 30 s after stop_pb, section_motor[4] turns off at 30 s and section_motor[5..8] at 35 s (±0.1 s).
11. safety_relay_ok FALSE turns every section_motor off within 0.1 s and stack_red flashes fast. Motors stay off after the relay recovers until reset_pb and start_pb are pressed.
12. filler_guard_closed or crowner_guard_closed FALSE has the same result as requirement 11, in Automatic and in Manual.
13. io_healthy FALSE turns every section_motor and beer_inlet_vlv off within 0.1 s. io_fault stays TRUE until reset_pb is pressed with io_healthy TRUE.
14. While any fault is latched, start_pb starts no motor and no horn.
15. While cip_needs_rinse is TRUE, start_pb in Automatic starts no motor.
16. Turning the selector from Automatic to Manual while running turns every section_motor off within 0.6 s.
17. In Manual, with no motor feedback for 10 s, holding jog_pb[i] sounds the horn for 3 s, then turns section_motor[i] on within 0.1 s. Releasing it turns the motor off within 0.1 s. Releasing during the horn moves nothing.
18. In Manual, within 10 s of the last motor feedback, holding jog_pb[i] turns section_motor[i] on within 0.1 s, with no horn.
19. In Off, Automatic and CIP, jog_pb[i] turns on no motor.
20. A running section turns its motor off when its end eye has been blocked for 0.5 s while the next section's run feedback is FALSE, within 0.1 s of the 0.5 s. Eye blocks shorter than 0.5 s do not stop it.
21. A blocked section restarts 1 s (±0.1 s) after the next section's run feedback becomes TRUE, or 1 s after its own end eye clears.
22. With sections 8 and 7 stopped in cascade, clearing section 8's end eye for 1 s starts section 8 first. Section 7 starts 1 s (±0.1 s) after section 8's run feedback.
23. Section 8 stops when its end eye has been blocked for 3 s, and restarts when the eye has been clear for 1 s.
24. Section 3 stops when its end eye has been blocked for 0.5 s while filler_gate_open is FALSE, even with the filler running.
25. A section whose motor is commanded with no run feedback for 1 s latches a fault, every motor turns off within 0.1 s, and stack_red flashes fast.
26. Run feedback TRUE for 2 s with the motor off latches a fault, which reset_pb does not clear while the feedback stays TRUE.
27. In sections 1 to 7, an end eye blocked for 10 s while the section and the next section run latches a jam fault and stops the line within 0.1 s.
28. Thirty bottles arriving at a section, with no bottle seen at its end eye, latch an eye fault and stop the line within 0.1 s.
29. reset_pb clears a section fault whose cause is gone, and no motor restarts until start_pb is pressed.
30. In Automatic, running with the filler producing and no section waiting, stack_green is steady. During warning, stopping, run-out, or with any section blocked, it flashes slowly.
31. Any warning lights stack_amber steady within 0.1 s. In Manual with no warning, stack_amber flashes slowly.
32. Each new fault sounds the horn for 3 s (±0.1 s), and stack_red flashes fast until the fault is reset.
33. In Automatic, with tank pressure in band, beer_inlet_vlv opens within 0.1 s when the level falls below 40 %, and closes within 0.1 s at 70 %.
34. With the level between 40 % and 70 %, beer_inlet_vlv keeps its previous state.
35. tank_high_sw TRUE closes beer_inlet_vlv within 0.1 s and lights stack_amber.
36. With tank pressure outside 1.6–2.8 bar for 0.5 s, beer_inlet_vlv stays closed at any level.
37. In Off and Manual, beer_inlet_vlv stays closed at any level.
38. In Off, Manual and Automatic, co2_inlet_vlv opens within 0.1 s below 1.8 bar and closes within 0.1 s at 2.0 bar.
39. tank_vent_vlv opens within 0.1 s above 2.6 bar and closes within 0.1 s at 2.4 bar.
40. When the level falls below 20 %, every fill_vlv closes, filler_gate_open closes and section_motor[4] turns off within 0.1 s. The filler runs again only after the level reaches 30 %.
41. With tank pressure outside 1.6–2.8 bar for 0.5 s, the result is the same as requirement 40. Filling resumes 0.5 s after the pressure is back in band.
42. A level reading below −5 % or above 105 % for 1 s, or the high switch made with the level below 80 % for 5 s, latches a tank fault that closes beer_inlet_vlv and stops the line within 0.1 s.
43. A pressure reading below −0.5 bar or above 6 bar for 1 s latches a tank fault that closes co2_inlet_vlv and tank_vent_vlv and stops the line within 0.1 s.
44. co2_inlet_vlv open for 60 s, or beer_inlet_vlv open for 600 s, lights stack_amber steady.
45. In CIP mode, beer_inlet_vlv and co2_inlet_vlv are closed and tank_vent_vlv is open, within 0.6 s of turning the selector.
46. In Automatic, rinse_water_vlv and rinse_air_vlv open within 0.1 s of section 3 run feedback, and close within 0.1 s of it ending.
47. With a rinser valve open and its pressure switch off for 3 s, the line stops with a rinser fault within 0.1 s.
48. In Automatic, crown_chute_ok FALSE for 2 s lights stack_amber steady within 0.1 s.
49. On the first start after power-up, filler_gate_open stays FALSE and no fill_vlv opens until a home pulse and 8 further pitch pulses. The gate then opens within 0.1 s of the 8th pulse.
50. Ten pitch pulses without home, while homing or producing, latch a filler fault and stop the line within 0.1 s.
51. A home pulse arriving at any position other than the expected one while producing latches a filler fault.
52. Section 4 running for 5 s with no pitch pulse latches a filler fault.
53. A bottle at filler_infeed_eye on a pitch pulse, with the gate open, makes that valve's fill_vlv open at the next pitch pulse and close at the 6th pitch pulse after loading. A valve with no bottle never opens.
54. Every fill_vlv closes within 0.1 s when section 4 run feedback or tank_fill_ok goes FALSE, and the valves of loaded pockets reopen when both return.
55. With no loaded pocket and filler_infeed_eye clear for 3 s, section_motor[4] turns off within 0.1 s. It restarts within 0.6 s after filler_infeed_eye has been blocked for 0.5 s.
56. A loaded pocket reaching its discharge pulse with the section 4 end eye clear adds 1 to broken_count. Three in a row latch a filler fault.
57. After a non-filler fault stop with bottles in the filler, then reset and start, the filler goes straight to producing without homing, and loaded pockets open their valves for their remaining fill pitches.
58. After any time in Manual or CIP, the next automatic start homes and purges the filler, as in requirement 49.
59. Crowns low (crown_chute_ok FALSE for 2 s) or the reject bin full (reject_bin_full TRUE for 2 s) closes filler_gate_open within 0.1 s, and loaded pockets still fill and discharge.
60. In Automatic, with no purge left, a bottle with fill_level_ok and crown_ok seen at the inspection head is not blasted, and good_count increases by 1 when it reaches the reject nozzle.
61. A bottle with low fill or no crown turns reject_blast_vlv on 20 ms (±10 ms) after its rising edge at reject_eye, for 80 ms (±10 ms), and reject_count increases by 1.
62. A blast with no reject_bin_eye edge within 1 s latches a reject fault and stops the line within 0.1 s.
63. After the filler homes, the next 20 bottles past the inspection head are rejected whatever their fill and crown.
64. A bottle reaching reject_eye with no inspection result is rejected, and stack_amber lights when no purge is in progress.
65. A 17th inspection result waiting for the reject nozzle latches a reject fault.
66. In any mode other than Automatic, every bottle passing inspection is rejected.
67. Holding shift_reset_pb for 2 s sets good_count, reject_count and broken_count to 0 within 0.1 s. Holding it for 1.5 s changes nothing.
68. In CIP, with cups fitted, tank below 2 %, no faults and reset done, start_pb turns on cip_pump_run, cip_caustic_vlv, cip_return_vlv, cip_heater_on and every fill_vlv within 0.1 s.
69. In CIP, start_pb with cups not fitted, or with the tank at 2 % or more, turns on no CIP output.
70. The caustic hold counts time only after the return reaches 75 °C, and only while it stays at or above 73 °C. Rinse 1 starts after 20 min of counted time.
71. During the heat and hold steps, cip_heater_on is TRUE below the step setpoint and FALSE at setpoint + 2 °C.
72. Rinse 1 runs water to drain with pump and fill valves on and heater off for 5 min (±0.1 s), then acid starts.
73. The acid hold counts time only after the return reaches 60 °C, and only while it stays at or above 58 °C. The final rinse starts after 15 min of counted time.
74. The final rinse runs 10 min to drain, the drain step opens only cip_drain_vlv for 120 s, and then every CIP output is off and stack_green double-flashes.
75. A heat step that does not reach its setpoint within 30 min goes to Hold, with pump, heater and valves off within 0.1 s and a CIP fault latched.
76. Return temperature below the hold minimum for 10 min continuously during a hold step goes to Hold with a CIP fault.
77. cip_pump_run TRUE with cip_flow_ok FALSE for 15 s goes to Hold, with pump and heater off within 0.1 s.
78. stop_pb during CIP goes to Hold. start_pb, after reset of any fault, resumes the same chemical at its heat step with the counted time kept.
79. A safety fault during CIP goes to Hold within 0.1 s.
80. A return temperature reading below −10 °C or above 120 °C for 1 s goes to Hold with a CIP fault.
81. Leaving CIP mode turns every CIP output and every fill_vlv off within 0.6 s. If caustic or acid has run, automatic start stays blocked until a CIP completes its final rinse.

## Build order

1. main and mode_manager: declare all signals and part instances; build selector decoding, edge pulses, power-up pending, safety and I/O fault latches. Requirements 3, 4.
2. annunciator: stack light patterns, new-fault horn pulse, horn_req pass-through; main wires any_fault and any_warning. Requirements 1, 31, 32.
3. mode_manager: line states 0–4 with 5 s warning horn, start conditions, stop and run-out timers. Requirements 2, 5, 7, 14.
4. conveyor_section: Off, Running, Jog and Fault states with feedback faults; eight instances wired to sect_enable and section_motor. Requirements 6, 8, 11, 12, 13, 16, 25, 26, 29.
5. mode_manager: jog with 10 s movement memory, 3 s horn warning and jog_cmd per section. Requirements 17, 18, 19.
6. conveyor_section: Blocked and Held states, block and restart timers; main wires downstream_running, block_time and permit. Requirements 20, 21, 22, 23, 24.
7. conveyor_section: jam timer, bottles-unseen counter, eye-clear fault, waiting_long warning; main wires upstream_bottle and check flags. Requirements 27, 28.
8. buffer_tank: pressure band, CO2 and vent control, pressure sensor fault, CIP venting; main wires cip_active as mode = 3. Requirements 38, 39, 43, 45.
9. buffer_tank: level sensor faults, inlet control, fill permit with hysteresis, low, high, CO2-slow and inlet-slow warnings. Requirements 33, 34, 35, 36, 37, 42, 44.
10. rinser: water and air valves with the rinser running, 3 s pressure-loss fault. Requirements 46, 47.
11. crowner: debounced crowns_ok and crowns low warning. Requirement 48.
12. shift_counter: three shift counts from pulses, 2 s held shift reset. Requirement 67.
13. filler: Idle, Homing, Purge and Producing states, pitch and home handling, pitch and home faults, gate and permit; main wires section 4 permit. Requirements 40, 41, 49, 50, 51, 52, 58.
14. filler: pocket tracking, fill valve rule, starved and empty; main wires filler_empty into mode_manager. Requirements 9, 10, 30, 53, 54, 55, 57.
15. filler: discharge check, broken pulse and three-in-a-row fault; main wires broken_pulse to shift_counter. Requirement 56.
16. inspection: result FIFO, purge count, reject blast timing, good and reject pulses, empty-tracking warning, overflow fault. Requirements 60, 61, 63, 64, 65, 66.
17. inspection: reject confirmation fault and bin-full debounce; main wires bin_ok and crowns_ok into filler. Requirements 59, 62.
18. cip: states 0–9 with heat, counted holds, rinses, drain, thermostat; main wires CIP outputs, cip_active and the fill valve request. Requirements 68, 69, 70, 71, 72, 73, 74.
19. cip: Hold state, flow, heat, temperature and sensor faults, resume, abort, cip_needs_rinse into mode_manager. Requirements 15, 75, 76, 77, 78, 79, 80, 81.

## Open questions

1. The controller retains nothing over a power loss. The shift counts and cip_needs_rinse are lost when power drops. A power cut during CIP could therefore leave caustic or acid in the tank and filler with nothing blocking production. Can retained memory be enabled for these four values, or a conductivity sensor be fitted on the CIP return? Assumed: counts restart at 0 and cip_needs_rinse starts FALSE after power-up.
2. Is the emergency stop wired in the safety relay chain, so that safety_relay_ok drops on an e-stop? Assumed: yes. There is no separate e-stop input to the program.
3. The filler needs I/O beyond the description:
   - a pitch sensor and a home sensor;
   - an infeed eye after a new infeed gate;
   - one fill-valve solenoid per valve that closes when de-energised;
   - a section 4 end eye that sees the bottle in the discharge pocket at the pitch pulse.

   Assumed: all exist as named. Fill zone is pitches 1–5, snift at 6, discharge at 7, and fill height is set by the vent tube.
4. io_healthy (remote I/O coupler status) and shift_reset_pb are new points. Assumed: both are available as digital inputs.
5. Purge size: assumed 20 bottles, 8 in the filler plus 12 between the filler discharge and the inspection head. Please count the actual pockets and conveyor length.
6. Buffer tank setpoints are all assumed and should be confirmed for your beer and filler:

   | Setting | Assumed value |
   |---|---|
   | Inlet open / close | 40 % / 70 % |
   | Filling stops / resumes | 20 % / 30 % |
   | CO2 open / close | 1.8 / 2.0 bar |
   | Vent open / close | 2.6 / 2.4 bar |
   | Filling pressure band | 1.6–2.8 bar |
   | CO2-slow and inlet-slow warnings | 60 s and 600 s |

7. Is the buffer tank protected by a mechanical relief valve? Assumed: yes. With a pressure sensor fault, both the CO2 and vent valves are closed.
8. CIP recipe, assumed:

   | Step | Temperature | Counted time |
   |---|---|---|
   | Caustic | 75 °C (hold minimum 73 °C) | 20 min |
   | Rinse 1 | — | 5 min |
   | Acid | 60 °C (hold minimum 58 °C) | 15 min |
   | Final rinse | — | 10 min |
   | Drain | — | 120 s |

   Other CIP limits, assumed: 30 min heat limit, 10 min low-temperature limit, 15 s flow limit, tank below 2 % to start. The design also assumes the CIP set is driven from this controller through the listed pump, valves and heater, with the filler drive stopped during CIP.
9. Reject timing, assumed: blast 20 ms after the reject eye for 80 ms, bin confirmation within 1 s, bin full after 2 s. Please confirm from the nozzle position and conveyor speed.
10. Accumulation table, assumed: it stops after its end eye is blocked for 3 s and restarts when the eye has been clear for 1 s. Is a fuller table wanted, with a second "table full" eye?
11. Section timings, assumed: block 0.5 s, restart 1 s, feedback 1 s / 2 s, jam 10 s, eye clear after 30 bottles, long-wait warning 300 s.
12. Are start_pb and stop_pb normally open? Assumed: both TRUE when pressed. A normally closed stop would be safer against a broken wire.
13. Labeller supervision (label low, label missing) and depalletiser sweep or layer handling have no signals in the description. Assumed: section 1 is a plain conveyor loaded by hand, and the labeller has no faults of its own.
14. Rinser supply, assumed: water and air pressure switches, with a 3 s loss stopping the line.