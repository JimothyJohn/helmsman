(* IEC 61131-3 standard function blocks, compiled with every program.
   Timers read __NOW(), the time since the program started, which advances
   only between scans, so a timer's outputs are stable within one scan.
   Counters saturate at the INT range. *)

FUNCTION_BLOCK R_TRIG
  VAR_INPUT CLK : BOOL; END_VAR
  VAR_OUTPUT Q : BOOL; END_VAR
  VAR M : BOOL; END_VAR
  Q := CLK AND NOT M;
  M := CLK;
END_FUNCTION_BLOCK

FUNCTION_BLOCK F_TRIG
  VAR_INPUT CLK : BOOL; END_VAR
  VAR_OUTPUT Q : BOOL; END_VAR
  (* Starts TRUE so a CLK that is FALSE from the first scan is no edge. *)
  VAR M : BOOL := TRUE; END_VAR
  Q := NOT CLK AND NOT M;
  M := NOT CLK;
END_FUNCTION_BLOCK

FUNCTION_BLOCK SR
  VAR_INPUT S1, R : BOOL; END_VAR
  VAR_OUTPUT Q1 : BOOL; END_VAR
  Q1 := S1 OR (NOT R AND Q1);
END_FUNCTION_BLOCK

FUNCTION_BLOCK RS
  VAR_INPUT S, R1 : BOOL; END_VAR
  VAR_OUTPUT Q1 : BOOL; END_VAR
  Q1 := NOT R1 AND (S OR Q1);
END_FUNCTION_BLOCK

FUNCTION_BLOCK CTU
  VAR_INPUT CU, R : BOOL; PV : INT; END_VAR
  VAR_OUTPUT Q : BOOL; CV : INT; END_VAR
  VAR CU_EDGE : R_TRIG; END_VAR
  CU_EDGE(CLK := CU);
  IF R THEN
    CV := 0;
  ELSIF CU_EDGE.Q AND CV < 32767 THEN
    CV := CV + 1;
  END_IF;
  Q := CV >= PV;
END_FUNCTION_BLOCK

FUNCTION_BLOCK CTD
  VAR_INPUT CD, LD : BOOL; PV : INT; END_VAR
  VAR_OUTPUT Q : BOOL; CV : INT; END_VAR
  VAR CD_EDGE : R_TRIG; END_VAR
  CD_EDGE(CLK := CD);
  IF LD THEN
    CV := PV;
  ELSIF CD_EDGE.Q AND CV > -32768 THEN
    CV := CV - 1;
  END_IF;
  Q := CV <= 0;
END_FUNCTION_BLOCK

FUNCTION_BLOCK CTUD
  VAR_INPUT CU, CD, R, LD : BOOL; PV : INT; END_VAR
  VAR_OUTPUT QU, QD : BOOL; CV : INT; END_VAR
  VAR CU_EDGE, CD_EDGE : R_TRIG; END_VAR
  CU_EDGE(CLK := CU);
  CD_EDGE(CLK := CD);
  IF R THEN
    CV := 0;
  ELSIF LD THEN
    CV := PV;
  ELSIF NOT (CU_EDGE.Q AND CD_EDGE.Q) THEN
    IF CU_EDGE.Q AND CV < 32767 THEN
      CV := CV + 1;
    ELSIF CD_EDGE.Q AND CV > -32768 THEN
      CV := CV - 1;
    END_IF;
  END_IF;
  QU := CV >= PV;
  QD := CV <= 0;
END_FUNCTION_BLOCK

(* On-delay: Q goes TRUE once IN has been TRUE for PT. *)
FUNCTION_BLOCK TON
  VAR_INPUT IN : BOOL; PT : TIME; END_VAR
  VAR_OUTPUT Q : BOOL; ET : TIME; END_VAR
  VAR RUNNING : BOOL; START : TIME; END_VAR
  IF NOT IN THEN
    Q := FALSE;
    ET := T#0s;
    RUNNING := FALSE;
  ELSE
    IF NOT RUNNING THEN
      RUNNING := TRUE;
      START := __NOW();
    END_IF;
    ET := __NOW() - START;
    IF ET >= PT THEN
      ET := PT;
      Q := TRUE;
    ELSE
      Q := FALSE;
    END_IF;
  END_IF;
END_FUNCTION_BLOCK

(* Off-delay: Q follows IN up, and stays TRUE for PT after IN falls. *)
FUNCTION_BLOCK TOF
  VAR_INPUT IN : BOOL; PT : TIME; END_VAR
  VAR_OUTPUT Q : BOOL; ET : TIME; END_VAR
  VAR RUNNING : BOOL; START : TIME; END_VAR
  IF IN THEN
    Q := TRUE;
    ET := T#0s;
    RUNNING := FALSE;
  ELSIF Q THEN
    IF NOT RUNNING THEN
      RUNNING := TRUE;
      START := __NOW();
    END_IF;
    ET := __NOW() - START;
    IF ET >= PT THEN
      ET := PT;
      Q := FALSE;
      RUNNING := FALSE;
    END_IF;
  END_IF;
END_FUNCTION_BLOCK

(* Pulse: a rising IN starts a PT-long pulse; IN is ignored while it runs.
   ET holds PT after the pulse until IN is FALSE. *)
FUNCTION_BLOCK TP
  VAR_INPUT IN : BOOL; PT : TIME; END_VAR
  VAR_OUTPUT Q : BOOL; ET : TIME; END_VAR
  VAR RUNNING, PREV : BOOL; START : TIME; END_VAR
  IF RUNNING THEN
    ET := __NOW() - START;
    IF ET >= PT THEN
      ET := PT;
      Q := FALSE;
      RUNNING := FALSE;
    END_IF;
  ELSIF IN AND NOT PREV THEN
    RUNNING := TRUE;
    START := __NOW();
    Q := TRUE;
    ET := T#0s;
  END_IF;
  IF NOT RUNNING AND NOT IN THEN
    ET := T#0s;
  END_IF;
  PREV := IN;
END_FUNCTION_BLOCK
