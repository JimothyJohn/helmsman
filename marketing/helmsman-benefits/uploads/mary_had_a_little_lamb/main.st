PROGRAM main
VAR_INPUT
  iologik_e1213_di_00 : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
  enable : INT;
  slew : DINT;
END_VAR
VAR CONSTANT
  STEPS_PER_REV : DINT := 51200;
  NOTE_SCANS : DINT := 40;
  NOTE_COUNT : DINT := 28;
  NOTE_C : DINT := 40637;
  NOTE_D : DINT := 45619;
  NOTE_G : DINT := 60892;
END_VAR
VAR
  tick : DINT;
  idx : DINT;
END_VAR
  lamp := iologik_e1213_di_00;
  IF iologik_e1213_di_00 THEN
    enable := 1;
    idx := (tick / NOTE_SCANS) MOD NOTE_COUNT;
    tick := tick + 1;
    IF tick >= NOTE_SCANS * NOTE_COUNT THEN
      tick := 0;
    END_IF;
    CASE idx OF
      0: slew := STEPS_PER_REV;
      1: slew := NOTE_D;
      2: slew := NOTE_C;
      3: slew := NOTE_D;
      4: slew := STEPS_PER_REV;
      5: slew := STEPS_PER_REV;
      6: slew := STEPS_PER_REV;
      7: slew := NOTE_D;
      8: slew := NOTE_D;
      9: slew := NOTE_D;
      10: slew := STEPS_PER_REV;
      11: slew := NOTE_G;
      12: slew := NOTE_G;
      13: slew := STEPS_PER_REV;
      14: slew := NOTE_D;
      15: slew := NOTE_C;
      16: slew := NOTE_D;
      17: slew := STEPS_PER_REV;
      18: slew := STEPS_PER_REV;
      19: slew := STEPS_PER_REV;
      20: slew := STEPS_PER_REV;
      21: slew := NOTE_D;
      22: slew := NOTE_D;
      23: slew := STEPS_PER_REV;
      24: slew := NOTE_D;
      25: slew := NOTE_C;
    ELSE
      slew := 0;
    END_CASE;
  ELSE
    tick := 0;
    idx := 0;
    enable := 0;
    slew := 0;
  END_IF;
END_PROGRAM
