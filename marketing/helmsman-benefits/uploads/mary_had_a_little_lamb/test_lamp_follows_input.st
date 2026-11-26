(* lamp turns on when the input goes high and off when it goes low *)
PROGRAM test_lamp_follows_input
VAR_OUTPUT
  iologik_e1213_di_00 : BOOL;
  done : BOOL;
END_VAR
VAR_INPUT
  lamp : BOOL;
END_VAR
VAR
  n : INT;
  lamp_off_first : ASSERT;
  lamp_on : ASSERT;
  lamp_off_after : ASSERT;
END_VAR
  n := n + 1;
  iologik_e1213_di_00 := (n >= 20) AND (n < 60);
  IF n = 10 THEN
    lamp_off_first(COND := NOT lamp);
  END_IF;
  IF n = 40 THEN
    lamp_on(COND := lamp);
  END_IF;
  IF n = 80 THEN
    lamp_off_after(COND := NOT lamp);
    done := TRUE;
  END_IF;
END_PROGRAM
