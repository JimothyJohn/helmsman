Design: The lamp lights when start is pressed, goes out when stop is pressed, and goes out by itself after it has been lit for 10 minutes.

## Machine

One indicator lamp, one start push button and one stop push button. There is no other equipment and nothing moves.

There is one mode. Pressing start lights the lamp. Pressing stop puts it out. If nobody presses stop, the lamp goes out by itself 10 minutes after it was lit. The operator works the two buttons at the panel where the lamp is mounted.

The lamp is always out after power-up and after a controller restart. Stop always wins over start.

## Signals

| Name | In/Out/Internal | Type | Meaning |
|---|---|---|---|
| start_pb | In | BOOL | TRUE while the start button is pressed |
| stop_pb | In | BOOL | TRUE while the stop button is pressed (assumed normally open wiring) |
| lamp_on | Out | BOOL | TRUE lights the lamp |

## Parts

### lamp_control
- Job: Lights and puts out the lamp from the two buttons and the 10-minute limit.
- Reads: start_pb, stop_pb
- Writes: lamp_on
- Interface:
```st interface
VAR_INPUT
    start_pb : BOOL := FALSE;
    stop_pb  : BOOL := FALSE;
END_VAR
VAR_OUTPUT
    lamp_on  : BOOL := FALSE;
END_VAR
VAR
    state         : INT := 0;          (* 0 = Off, 1 = On *)
    start_edge    : R_TRIG;
    on_timer      : TON;
    on_time_limit : TIME := T#10m;
END_VAR
```
- States:

| No. | State | Outputs | Leaves when | Time limit |
|---|---|---|---|---|
| 0 | Off | lamp_on FALSE | To 1 on a rising edge of start_pb while stop_pb is FALSE | None. It waits for the operator, not the plant. |
| 1 | On | lamp_on TRUE | To 0 when stop_pb is TRUE, or when on_timer.Q is TRUE | 10 min (on_time_limit), the stated auto-off time |

- Rules:
  1. The part starts in state 0 at power-up and after every controller restart. Nothing is retained.
  2. A rising edge of start_pb (start_edge) is the only way to enter state 1. Holding start does not relight the lamp after stop or after the timeout.
  3. While stop_pb is TRUE, the part goes to or stays in state 0, whatever start_pb does.
  4. on_timer has its input TRUE only in state 1, with PT := on_time_limit. Leaving state 1 resets it, so every lighting gets a full 10 minutes.
  5. A new press of start while the lamp is lit does nothing. The timer keeps running from when the lamp was first lit (assumed).
  6. lamp_on is TRUE exactly when state = 1.
  7. The PROGRAM calls one instance of lamp_control every scan. It passes start_pb and stop_pb in and lamp_on out.

## Faults

| Fault | Detected by | Reaction | Cleared by |
|---|---|---|---|
| Start and stop pressed together | stop_pb TRUE | Lamp goes out or stays out | Releasing stop and pressing start again |
| Start button stuck pressed | No rising edge of start_pb | Lamp cannot relight after it goes out | Freeing the button. The next press works. |
| Stop button stuck pressed | stop_pb held TRUE | Lamp stays out | Freeing the button |
| Lamp left lit | on_timer reaches 10 min | Lamp goes out | Pressing start again |
| Power returns or controller restarts | Initial values | Lamp out, state 0 | Pressing start |

## Requirements

1. With the lamp out and stop released, pressing start lights the lamp within 20 ms.
2. With the lamp lit, pressing stop puts it out within 20 ms.
3. With the lamp lit and no button pressed, the lamp is still lit at 9 min 59 s and is out by 10 min 0.1 s after it was lit.
4. With stop held, pressing start leaves the lamp out for at least 1 s.
5. With the lamp lit, pressing start and stop together puts the lamp out within 20 ms.
6. After stop puts the lamp out while start is held, the lamp stays out until start is released and pressed again.
7. After the lamp goes out on its 10-minute limit while start is held, the lamp stays out until start is released and pressed again.
8. With the lamp lit, pressing start again does not extend the time: the lamp is out by 10 min 0.1 s after it was first lit.
9. After the lamp is put out by stop and lit again, it stays lit for a full 10 min before going out by itself.
10. At controller start-up with no button pressed, the lamp is out and stays out for at least 1 s.

## Build order

1. lamp_control: declare all signals and the PROGRAM. Build states 0 and 1 with the start edge and stop priority, but no timer yet. Requirements 1, 2, 4, 5, 6, 10.
2. lamp_control: add on_timer and on_time_limit, run the timer only in state 1, and leave state 1 when it finishes. Requirements 3, 7, 8, 9.

## Open questions

1. Is the stop button wired normally closed, as is usual for stop buttons? Assumed normally open (stop_pb TRUE when pressed). If it is normally closed, the input becomes stop_ok, TRUE when not pressed, and the logic is inverted.
2. Should pressing start while the lamp is lit restart the 10 minutes? Assumed no.
3. Are the 10 minutes exact, or should the time be adjustable? Assumed fixed at 10 min.