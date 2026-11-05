(* Raw analog count to engineering units, held to the range. bad is set when
   the raw value is outside raw_min..raw_max by more than the margin: a broken
   wire on a 4-20 mA loop, or a transmitter that has failed. *)
FUNCTION_BLOCK analog_scale
VAR_INPUT
  raw : REAL;
  raw_min : REAL := 0.0;
  raw_max : REAL := 32767.0;
  eng_min : REAL := 0.0;
  eng_max : REAL := 100.0;
  margin : REAL := 0.05;
END_VAR
VAR_OUTPUT
  value : REAL;
  bad : BOOL;
END_VAR
VAR
  span : REAL;
END_VAR
span := raw_max - raw_min;
bad := span <= 0.0 OR raw < raw_min - span * margin OR raw > raw_max + span * margin;
IF span > 0.0 THEN
  value := LIMIT(eng_min, eng_min + (raw - raw_min) / span * (eng_max - eng_min), eng_max);
END_IF;
END_FUNCTION_BLOCK
