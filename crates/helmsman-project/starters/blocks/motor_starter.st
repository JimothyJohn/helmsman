(* Motor starter with contactor feedback and a latched fault.
   feedback is the contactor's auxiliary contact, overload the thermal relay.
   fault_code: 0 none, 1 overload, 2 no feedback after a start or feedback
   lost while running, 3 feedback with no command (contactor stuck on).
   A fault drops the contactor and holds until reset. An emergency stop
   belongs in a hardwired safety circuit, not in this block. *)
FUNCTION_BLOCK motor_starter
VAR_INPUT
  run : BOOL;
  feedback : BOOL;
  overload : BOOL;
  reset : BOOL;
  feedback_time : TIME := T#2s;
END_VAR
VAR_OUTPUT
  contactor : BOOL;
  running : BOOL;
  fault : BOOL;
  fault_code : INT;
END_VAR
VAR
  no_feedback : TON;
  stuck_on : TON;
END_VAR
no_feedback(IN := contactor AND NOT feedback, PT := feedback_time);
stuck_on(IN := NOT contactor AND feedback, PT := feedback_time);
IF overload THEN
  fault_code := 1;
ELSIF no_feedback.Q THEN
  fault_code := 2;
ELSIF stuck_on.Q THEN
  fault_code := 3;
ELSIF reset THEN
  fault_code := 0;
END_IF;
fault := fault_code <> 0;
contactor := run AND NOT fault;
running := contactor AND feedback;
END_FUNCTION_BLOCK
