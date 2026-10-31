(* Logix ladder semantics in IEC 61131-3 Structured Text, written by the
   L5X converter into every file it makes (only the parts it uses).
   Sources: Rockwell 1756-RM003Z-EN-P (General Instructions), page numbers
   in docs/SOURCES.md. Each block is stateless: a TIMER or COUNTER is
   copied in and out, so it stays an ordinary tag the rungs read and
   write. NOW is the scan clock in ms (it wraps like a DINT). *)

(* @TYPES *)
TYPE
  LOGIX_TIMER : STRUCT
    PRE : DINT; ACC : DINT; EN : BOOL; TT : BOOL; DN : BOOL;
    LAST : DINT; (* the clock at the last update: Logix's last_time_scanned *)
  END_STRUCT;
  LOGIX_COUNTER : STRUCT
    PRE : DINT; ACC : DINT; CU : BOOL; CD : BOOL; DN : BOOL; OV : BOOL; UN : BOOL;
  END_STRUCT;
END_TYPE

(* @TON *)
(* TON (RM003 pp.119-122): rung false resets the timer; rung true times
   until ACC >= PRE, then DN. A negative PRE or ACC is a major fault
   (type 4, code 34): here, a division by zero stops the program. *)
FUNCTION_BLOCK LOGIX_TON
  VAR_INPUT T : LOGIX_TIMER; RUNG : BOOL; NOW : DINT; END_VAR
  VAR_OUTPUT Q : LOGIX_TIMER; END_VAR
  VAR ZERO, FAULT, DELTA : DINT; END_VAR
  Q := T;
  IF NOT RUNG THEN
    Q.EN := FALSE; Q.TT := FALSE; Q.DN := FALSE; Q.ACC := 0;
    RETURN;
  END_IF;
  IF Q.PRE < 0 OR Q.ACC < 0 THEN
    FAULT := 1 / ZERO;
  END_IF;
  Q.TT := TRUE;
  IF Q.DN THEN
    Q.TT := FALSE; Q.LAST := NOW;
  ELSIF NOT Q.EN THEN
    Q.LAST := NOW;
  ELSE
    DELTA := NOW - Q.LAST; Q.LAST := NOW;
    IF DELTA > 2147483647 - Q.ACC THEN
      Q.ACC := 2147483647; Q.TT := FALSE; Q.DN := TRUE;
    ELSE
      Q.ACC := Q.ACC + DELTA;
      IF Q.ACC >= Q.PRE THEN Q.TT := FALSE; Q.DN := TRUE; END_IF;
    END_IF;
  END_IF;
  Q.EN := TRUE;
END_FUNCTION_BLOCK

(* @RTO *)
(* RTO (RM003 pp.103-106): rung false clears EN and TT and keeps ACC and
   DN; rung true times as TON does (RTO's own flow chart is missing from
   the manual; its text matches TON's). RES clears it. *)
FUNCTION_BLOCK LOGIX_RTO
  VAR_INPUT T : LOGIX_TIMER; RUNG : BOOL; NOW : DINT; END_VAR
  VAR_OUTPUT Q : LOGIX_TIMER; END_VAR
  VAR ZERO, FAULT, DELTA : DINT; END_VAR
  Q := T;
  IF NOT RUNG THEN
    Q.EN := FALSE; Q.TT := FALSE;
    RETURN;
  END_IF;
  IF Q.PRE < 0 OR Q.ACC < 0 THEN
    FAULT := 1 / ZERO;
  END_IF;
  Q.TT := TRUE;
  IF Q.DN THEN
    Q.TT := FALSE; Q.LAST := NOW;
  ELSIF NOT Q.EN THEN
    Q.LAST := NOW;
  ELSE
    DELTA := NOW - Q.LAST; Q.LAST := NOW;
    IF DELTA > 2147483647 - Q.ACC THEN
      Q.ACC := 2147483647; Q.TT := FALSE; Q.DN := TRUE;
    ELSE
      Q.ACC := Q.ACC + DELTA;
      IF Q.ACC >= Q.PRE THEN Q.TT := FALSE; Q.DN := TRUE; END_IF;
    END_IF;
  END_IF;
  Q.EN := TRUE;
END_FUNCTION_BLOCK

(* @TOF *)
(* TOF (RM003 pp.111-114): rung true sets EN and DN and clears ACC; rung
   false times until ACC >= PRE, then clears DN. *)
FUNCTION_BLOCK LOGIX_TOF
  VAR_INPUT T : LOGIX_TIMER; RUNG : BOOL; NOW : DINT; END_VAR
  VAR_OUTPUT Q : LOGIX_TIMER; END_VAR
  VAR ZERO, FAULT, DELTA : DINT; END_VAR
  Q := T;
  IF RUNG THEN
    Q.EN := TRUE; Q.TT := FALSE; Q.DN := TRUE; Q.ACC := 0;
    RETURN;
  END_IF;
  IF Q.PRE < 0 OR Q.ACC < 0 THEN
    FAULT := 1 / ZERO;
  END_IF;
  Q.TT := TRUE;
  IF NOT Q.DN THEN
    Q.TT := FALSE; Q.LAST := NOW;
  ELSIF Q.EN THEN
    Q.LAST := NOW;
  ELSE
    DELTA := NOW - Q.LAST; Q.LAST := NOW;
    IF DELTA > 2147483647 - Q.ACC THEN
      Q.ACC := 2147483647; Q.TT := FALSE; Q.DN := FALSE;
    ELSE
      Q.ACC := Q.ACC + DELTA;
      IF Q.ACC >= Q.PRE THEN Q.TT := FALSE; Q.DN := FALSE; END_IF;
    END_IF;
  END_IF;
  Q.EN := FALSE;
END_FUNCTION_BLOCK

(* @CTU *)
(* CTU (RM003 pp.92-96): counts on a false-to-true rung; past
   2147483647 it wraps and sets OV (or, after an underflow, clears both).
   DN is left alone while OV or UN is set. *)
FUNCTION_BLOCK LOGIX_CTU
  VAR_INPUT C : LOGIX_COUNTER; RUNG : BOOL; END_VAR
  VAR_OUTPUT Q : LOGIX_COUNTER; END_VAR
  Q := C;
  IF RUNG THEN
    IF NOT Q.CU THEN
      Q.CU := TRUE;
      IF Q.ACC = 2147483647 THEN
        Q.ACC := -2147483647 - 1;
        IF NOT Q.UN THEN Q.OV := TRUE; RETURN; END_IF;
        Q.UN := FALSE; Q.OV := FALSE;
      ELSE
        Q.ACC := Q.ACC + 1;
      END_IF;
    END_IF;
  ELSE
    Q.CU := FALSE;
  END_IF;
  IF NOT Q.OV AND NOT Q.UN THEN Q.DN := Q.ACC >= Q.PRE; END_IF;
END_FUNCTION_BLOCK

(* @CTD *)
(* CTD (RM003 pp.87-91): CTU's mirror; past -2147483648 it wraps and
   sets UN (or, after an overflow, clears both). *)
FUNCTION_BLOCK LOGIX_CTD
  VAR_INPUT C : LOGIX_COUNTER; RUNG : BOOL; END_VAR
  VAR_OUTPUT Q : LOGIX_COUNTER; END_VAR
  Q := C;
  IF RUNG THEN
    IF NOT Q.CD THEN
      Q.CD := TRUE;
      IF Q.ACC = -2147483647 - 1 THEN
        Q.ACC := 2147483647;
        IF NOT Q.OV THEN Q.UN := TRUE; RETURN; END_IF;
        Q.UN := FALSE; Q.OV := FALSE;
      ELSE
        Q.ACC := Q.ACC - 1;
      END_IF;
    END_IF;
  ELSE
    Q.CD := FALSE;
  END_IF;
  IF NOT Q.OV AND NOT Q.UN THEN Q.DN := Q.ACC >= Q.PRE; END_IF;
END_FUNCTION_BLOCK

(* @R2L *)
(* REAL to integer as Logix stores it (RM003 pp.853, 855-856): to the
   nearest, ties to even; the caller keeps the low bits. Values too large
   for any low bit to be set give 0. *)
FUNCTION LOGIX_R2L : LINT
  VAR_INPUT X : REAL; END_VAR
  VAR Y : LREAL; N : LINT; END_VAR
  Y := REAL_TO_LREAL(X);
  IF ABS(Y) >= 9.0E18 THEN LOGIX_R2L := 0; RETURN; END_IF;
  N := LREAL_TO_LINT(Y);
  IF ABS(Y - LINT_TO_LREAL(N)) = 0.5 AND (N MOD 2) <> 0 THEN
    IF Y > 0.0 THEN N := N - 1; ELSE N := N + 1; END_IF;
  END_IF;
  LOGIX_R2L := N;
END_FUNCTION

(* @DIVI *)
(* Integer DIV (RM003 pp.409-410): truncated; by zero, Source A. *)
FUNCTION LOGIX_DIVI : DINT
  VAR_INPUT A, B : DINT; END_VAR
  IF B = 0 THEN LOGIX_DIVI := A; ELSE LOGIX_DIVI := A / B; END_IF;
END_FUNCTION

(* @MODI *)
(* Integer MOD (RM003 pp.411, 416): A - TRUNC(A / B) * B; by zero, 0. *)
FUNCTION LOGIX_MODI : DINT
  VAR_INPUT A, B : DINT; END_VAR
  IF B = 0 THEN LOGIX_MODI := 0; ELSE LOGIX_MODI := A MOD B; END_IF;
END_FUNCTION

(* @MODR *)
(* REAL MOD (RM003 pp.411, 416): A - TRUNC(A / B) * B; by zero, 0. *)
FUNCTION LOGIX_MODR : REAL
  VAR_INPUT A, B : REAL; END_VAR
  VAR Q : REAL; END_VAR
  IF B = 0.0 THEN LOGIX_MODR := 0.0; RETURN; END_IF;
  Q := A / B;
  IF ABS(Q) < 8388608.0 THEN Q := DINT_TO_REAL(TRUNC(Q)); END_IF;
  LOGIX_MODR := A - Q * B;
END_FUNCTION
