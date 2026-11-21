(* A Universal Robot's state over Modbus TCP, and a heartbeat the robot's
   program can read (docs/devices/universal-robots.md).

   The robot gives a joint's angle in two registers: the angle inside one
   turn (0..6283 mrad) and the number of turns, signed. base_mrad puts
   them together: -1601 for a base at -1.601 rad, where the angle register
   alone says 4682.

   handshake counts up once a second while the robot is ready and is held
   at 0 when it is not; a robot program that sees it stop changing knows
   the controller is gone. *)
PROGRAM ur_status
VAR_INPUT
  robot_mode : INT;    (* 3 power off, 5 idle, 7 running *)
  protective : BOOL;
  estop : BOOL;
  base_angle : INT;    (* mrad, 0..6283 *)
  base_turns : INT;    (* whole turns, signed *)
  tcp_z : INT;         (* tenths of a mm *)
END_VAR
VAR_OUTPUT
  handshake : INT;
  base_mrad : DINT;
  robot_ready : BOOL;
  heartbeat : INT;
END_VAR
VAR
  tick : TON;
END_VAR
base_mrad := INT_TO_DINT(base_turns) * 6283 + INT_TO_DINT(base_angle);
robot_ready := robot_mode >= 5 AND NOT protective AND NOT estop;
tick(IN := NOT tick.Q, PT := T#1s);
IF tick.Q THEN
  heartbeat := heartbeat + 1;
  IF heartbeat > 1000 THEN
    heartbeat := 1;
  END_IF;
END_IF;
IF robot_ready THEN
  handshake := heartbeat;
ELSE
  handshake := 0;
END_IF;
END_PROGRAM
