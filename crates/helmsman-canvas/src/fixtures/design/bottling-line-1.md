Design: A bottling line for 330 ml bottles at about 40 a minute that runs eight conveyor sections in automatic with orderly blockage stops and restarts, fills through an 8-valve rotary filler fed from a CO2-pressurised buffer tank, rejects bad bottles, counts good, rejected and broken bottles per shift, recovers safely from a power loss mid-fill, and cleans tank and filler in a temperature-checked CIP mode.

## Machine

**What the line is.** The line takes empty 330 ml glass bottles from pallet layers and delivers filled, crowned and labelled bottles to a manual packing station, at about 40 bottles a minute. In this design every station is a numbered conveyor section. Each section has one motor contactor, one running feedback contact and one photo eye at its discharge end.

| Section | Station |
|---|---|
| 1 | Depalletiser table |
| 2 | Infeed conveyor |
| 3 | Twin rinser: bottles inverted, water jet then air jet |
| 4 | Rotary filler, main drive |
| 5 | Crowner |
| 6 | Inspection conveyor: fill-level and crown check, air-blast reject into a bin |
| 7 | Labeller |
| 8 | Accumulation table ahead of packing |

**Filler.** The filler has 8 valves on a carousel. A pitch sensor gives one pulse per valve passing the infeed, and a home sensor marks valve 1 at the infeed.

- An infeed gate (bottle stop) between the rinser and the filler infeed lets bottles in. An infeed eye just past the gate sees each bottle entering a pocket.
- The program tracks which pocket holds a bottle. A fill valve opens only for a pocket that holds a bottle, and only while the carousel turns.
- Fill height is set mechanically by the vent tube, so the program opens the valve through the fill zone and closes it before discharge.
- The eye at the end of section 4 sits at the discharge pocket. A bottle that was loaded but is not there at discharge is counted as broken.

**Buffer tank.** The buffer tank feeds the filler. It is kept between 40 % and 70 % by an inlet valve from the bright beer tank, and at 1.8 to 2.0 bar CO2 counter-pressure by a CO2 valve and a vent valve.

**Operating modes.** The mode selector at the one operator panel picks the mode:

- **Off:** nothing moves. Tank pressure is still held.
- **Manual:** each section can be jogged with its own hold-to-run button.
- **Automatic:** the start, stop and reset buttons run the line.
- **CIP:** caustic, rinse, acid, rinse and drain are pumped through the tank and the filler. The chemical steps count time only at temperature.

The panel also has a stack light (red, amber, green) and a horn. The horn sounds before anything moves.

**Safety hardware.** The emergency stop and the filler and crowner guard doors are wired through a safety relay that removes motor power in hardware. The program reads one contact per door and the relay's healthy contact, stops everything it controls and latches a fault. It is not the safety function.

**Power loss.** Nothing is retained over a power loss. On return:

- The line stays stopped until reset.
- On the first start the filler turns one full revolution with the gate closed and no valve open.
- The next 20 bottles reaching inspection are rejected, because the state of any bottle in the filler is unknown.

## Signals

| Name | In/Out/Internal | Type | Meaning |
|---|---|---|---|
| start_pb | In | BOOL | TRUE while the start button is pressed |
| stop_pb | In | BOOL | TRUE while the stop button is pressed |
| reset_pb | In | BOOL | TRUE while the reset button is pressed |
| mode_sel_manual | In | BOOL | Selector in Manual |
| mode_sel_auto | In | BOOL | Selector in Automatic |
| mode_sel_cip | In | BOOL | Selector in CIP (no position TRUE = Off) |
| jog_pb | In | ARRAY[1..8] OF BOOL | jog_pb[i] TRUE while the jog button of section i is held |
| shift_reset_pb | In | BOOL | TRUE while the shift-count reset button is pressed (new point) |
| safety_relay_ok | In | BOOL | Safety relay healthy contact: TRUE = e-stop chain and guards healthy |
| filler_guard_closed | In | BOOL | Filler guard door closed |
| crowner_guard_closed | In | BOOL | Crowner guard door closed |
| io_healthy | In | BOOL | Remote I/O coupler healthy (new point) |
| section_run_fb | In | ARRAY[1..8] OF BOOL | Motor contactor of section i is pulled in |
| section_end_eye | In | ARRAY[1..8] OF BOOL | Eye at the end of section i is blocked by a bottle |
| rinse_water_ok | In | BOOL | Rinser water pressure switch made |
| rinse_air_ok | In | BOOL | Rinser air pressure switch made |
| tank_level_pct | In | REAL | Buffer tank level, 0–100 % |
| tank_pressure_bar | In | REAL | Buffer tank head pressure, bar gauge, 0–6 |
| tank_high_sw | In | BOOL | Independent high-level switch wet |
| filler_pitch_pulse | In | BOOL | TRUE while a valve passes the pitch sensor |
| filler_home_sw | In | BOOL | TRUE while valve 1 is at the infeed |
| filler_infeed_eye | In | BOOL | Bottle present at the filler infeed, after the gate |
| crown_chute_ok | In | BOOL | Crown chute above its low switch |
| inspect_eye | In | BOOL | Bottle in front of the inspection head |
| fill_level_ok | In | BOOL | Fill-level sensor sees liquid at the correct height |
| crown_ok | In | BOOL | Crown sensor sees a crown |
| reject_eye | In | BOOL | Bottle in front of the reject nozzle |
| reject_bin_eye | In | BOOL | Bottle passing into the reject bin |
| reject_bin_full | In | BOOL | Reject bin full switch made |
| cip_flow_ok | In | BOOL | CIP supply flow switch made |
| cip_return_temp_c | In | REAL | CIP return temperature, °C, 0–100 |
| cip_cups_fitted | In | BOOL | CIP cups fitted on all filler valves |
| section_motor | Out | ARRAY[1..8] OF BOOL | Run section i motor |
| rinse_water_vlv | Out | BOOL | Rinser water valve open |
| rinse_air_vlv | Out | BOOL | Rinser air valve open |
| beer_inlet_vlv | Out | BOOL | Buffer tank inlet from bright beer tank open |
| co2_inlet_vlv | Out | BOOL | CO2 supply to buffer tank open |
| tank_vent_vlv | Out | BOOL | Buffer tank vent open |
| filler_gate_open | Out | BOOL | Filler infeed gate open, bottles may enter |
| fill_vlv | Out | ARRAY[1..8] OF BOOL | Fill valve of filler valve k open |
| reject_blast_vlv | Out | BOOL | Reject air blast on |
| cip_pump_run | Out | BOOL | CIP supply pump running |
| cip_water_vlv | Out | BOOL | CIP water supply valve open |
| cip_caustic_vlv | Out | BOOL | CIP caustic supply valve open |
| cip_acid_vlv | Out | BOOL | CIP acid supply valve open |
| cip_return_vlv | Out | BOOL | CIP return to CIP set (recirculate) open |
| cip_drain_vlv | Out | BOOL | CIP return to drain open |
| cip_heater_on | Out | BOOL | CIP heater on |
| stack_red | Out | BOOL | Red lamp lit |
| stack_amber | Out | BOOL | Amber lamp lit |
| stack_green | Out | BOOL | Green lamp lit |
| horn | Out | BOOL | Horn sounding |
| mode | Internal | INT | 0 Off, 1 Manual, 2 Automatic, 3 CIP |
| line_state | Internal | INT | 0 Stopped, 1 Warning, 2 Running, 3 Stopping, 4 Run-out |
| sect_enable | Internal | ARRAY[1..8] OF BOOL | Section i may run in automatic |
| jog_cmd | Internal | ARRAY[1..8] OF BOOL | Section i jog allowed and requested |
| feed_stop | Internal | BOOL | Line is stopping: filler gate must close |
| start_edge | Internal | BOOL | One-scan pulse on start_pb press |
| stop_edge | Internal | BOOL | One-scan pulse on stop_pb press |
| reset_req | Internal | BOOL | One-scan pulse on reset_pb press |
| power_up_pending | Internal | BOOL | Power has returned and no reset since |
| safety_fault | Internal | BOOL | Latched: safety relay dropped or guard opened |
| io_fault | Internal | BOOL | Latched: remote I/O lost |
| selector_invalid | Internal | BOOL | More than one selector position made |
| horn_req | Internal | BOOL | Mode manager asks for the horn (pre-start, pre-jog) |
| any_fault | Internal | BOOL | Any latched fault in any part |
| any_warning | Internal | BOOL | Any warning active |
| any_waiting | Internal | BOOL | Any section blocked or held, or filler starved |
| any_motor_fb | Internal | BOOL | Any section_run_fb TRUE |
| section_fault | Internal | ARRAY[1..8] OF BOOL | Section i has a latched fault |
| tank_fill_ok | Internal | BOOL | Tank level and pressure allow filling |
| tank_fault | Internal | BOOL | Latched