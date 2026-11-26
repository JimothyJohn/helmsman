(* while the input is high the motor is enabled at 1 rev per second (51200 steps/s), and stopped when it drops *)
PROGRAM test_motor_one_rps
VAR_OUTPUT
  iologik_e1213_di_00 : BOOL;
  done : BOOL;
END_VAR
VAR_INPUT
  lamp : BOOL;
  enable : INT;
  slew : DINT;
END_VAR
VAR
  n : INT;
  speed_on : ASSERT;
  enabled_on : ASSERT;
  speed_off : ASSERT;
  enabled_off : ASSERT;
END_VAR
  n := n + 1;
  iologik_e1213_di_00 := (n >= 20) AND (n < 60);
  IF n = 40 THEN
    speed_on(COND := slew = 51200);
    enabled_on(COND := enable = 1);
  END_IF;
  IF n = 80 THEN
    speed_off(COND := slew = 0);
    enabled_off(COND := enable = 0);
    done := TRUE;
  END_IF;
END_PROGRAM
