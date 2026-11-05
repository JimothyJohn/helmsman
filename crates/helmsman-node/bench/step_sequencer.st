(* A numbered sequence. Step 0 is idle; start moves to step 1. Each step sets
   advance when it is finished, and the last step goes back to idle with done
   TRUE for one scan. step_limit is how long the current step may take (T#0s:
   no limit); past it, timed_out is set and the step holds until abort. abort
   returns to idle at once. Write the steps in a CASE on the step output. *)
FUNCTION_BLOCK step_sequencer
VAR_INPUT
  start : BOOL;
  advance : BOOL;
  abort : BOOL;
  last_step : INT;
  step_limit : TIME;
END_VAR
VAR_OUTPUT
  step : INT;
  step_time : TIME;
  timed_out : BOOL;
  done : BOOL;
END_VAR
VAR
  watch : TON;
  entered : BOOL;
END_VAR
done := FALSE;
entered := FALSE;
IF abort THEN
  step := 0;
  timed_out := FALSE;
  entered := TRUE;
ELSIF step = 0 THEN
  IF start THEN
    step := 1;
    entered := TRUE;
  END_IF;
ELSIF advance AND NOT timed_out THEN
  entered := TRUE;
  IF step >= last_step THEN
    step := 0;
    done := TRUE;
  ELSE
    step := step + 1;
  END_IF;
END_IF;
watch(IN := step > 0 AND NOT entered, PT := step_limit);
step_time := watch.ET;
IF step > 0 AND step_limit > T#0s AND watch.Q THEN
  timed_out := TRUE;
END_IF;
END_FUNCTION_BLOCK
