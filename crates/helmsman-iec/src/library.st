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

(* BLINK_PATTERN: a stack light's patterns. Not IEC 61131-3: a Helmsman
   extension that ruSTy and other compilers do not have. Not named BLINK,
   since a standard block's name is taken for every program and a module
   called blink is likely. While EN, Q follows MODE:
     0  off
     1  steady on
     2  slow blink: on for the first half of each PERIOD
     3  fast blink: four times as fast (on for the first half of each
        PERIOD / 4)
     4  double flash: on, off, on for PERIOD / 8 each, then off for the
        rest of PERIOD
   EN FALSE forces Q FALSE, and so does any other MODE. A blinking mode with
   PERIOD <= 0 (or one too short to divide into its steps) faults the
   module (FaultKind::BadParam, at the line that called the block): a bad
   param or an uninitialised TIME is loud, not a light that looks right.
   Off, steady, and EN FALSE never look at PERIOD, so they never fault. A
   pattern starts from its beginning (on) when EN rises, when MODE
   changes, and after a steady or off spell. Time is the
   scan clock (__NOW), like the timers'; the phase runs on across scans, so a
   long scan gap lands where the pattern would be, and a PERIOD change keeps
   the phase (within the new cycle). *)
FUNCTION_BLOCK BLINK_PATTERN
  VAR_INPUT EN : BOOL; MODE : INT; PERIOD : TIME; END_VAR
  VAR_OUTPUT Q : BOOL; END_VAR
  VAR RUNNING : BOOL; SHOWN : INT; LAST, PHASE, CYCLE, STEP : TIME; END_VAR
  IF NOT EN OR MODE < 1 OR MODE > 4 THEN
    Q := FALSE;
    RUNNING := FALSE;
  ELSIF MODE = 1 THEN
    Q := TRUE;
    RUNNING := FALSE;
  ELSE
    IF MODE = 3 THEN
      CYCLE := PERIOD / 4;
    ELSE
      CYCLE := PERIOD;
    END_IF;
    IF MODE = 4 THEN
      STEP := CYCLE / 8;
    ELSE
      STEP := CYCLE / 2;
    END_IF;
    IF STEP <= T#0s THEN
      Q := __BAD_BLINK_PERIOD();
    ELSE
      IF NOT RUNNING OR MODE <> SHOWN THEN
        RUNNING := TRUE;
        SHOWN := MODE;
        PHASE := T#0s;
      ELSE
        PHASE := PHASE + (__NOW() - LAST);
        IF PHASE >= CYCLE THEN
          (* The quotient from LREAL, then held to [0, CYCLE) exactly. *)
          PHASE := PHASE - CYCLE * LREAL_TO_LINT(TIME_TO_LREAL(PHASE) / TIME_TO_LREAL(CYCLE));
          IF PHASE < T#0s THEN
            PHASE := PHASE + CYCLE;
          ELSIF PHASE >= CYCLE THEN
            PHASE := PHASE - CYCLE;
          END_IF;
          IF PHASE < T#0s OR PHASE >= CYCLE THEN
            PHASE := T#0s;
          END_IF;
        END_IF;
      END_IF;
      LAST := __NOW();
      IF MODE = 4 THEN
        Q := PHASE < STEP OR (PHASE >= STEP * 2 AND PHASE < STEP * 3);
      ELSE
        Q := PHASE < STEP;
      END_IF;
    END_IF;
  END_IF;
END_FUNCTION_BLOCK
