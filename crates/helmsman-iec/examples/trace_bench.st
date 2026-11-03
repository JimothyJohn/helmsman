(* The scan-throughput workload: loops, IF/CASE, arithmetic and function
   block calls, about 4 000 statements per scan. Shared by the host bench
   (examples/trace_bench.rs) and the edge node's bench (crates/helmsman-node),
   so the two measure the same program. *)
FUNCTION_BLOCK acc
VAR_INPUT x : DINT; END_VAR
VAR_OUTPUT sum : DINT; END_VAR
  IF x > 0 THEN sum := sum + x; ELSE sum := sum - 1; END_IF;
END_FUNCTION_BLOCK
PROGRAM bench
VAR
  i, j, k, total : DINT;
  a : ARRAY[0..63] OF DINT;
  sums : ARRAY[0..3] OF acc;
  t : TON;
  on : BOOL;
END_VAR
  t(IN := NOT t.Q, PT := T#5ms);
  IF t.Q THEN on := NOT on; END_IF;
  total := 0;
  FOR i := 0 TO 63 DO
    a[i] := a[i] + i * 3 - total MOD 7;
    CASE i MOD 4 OF
      0: total := total + a[i];
      1, 2: total := total - 1;
    ELSE
      total := total + i / 2;
    END_CASE;
    FOR j := 0 TO 3 DO
      k := (k + j) MOD 1000;
      sums[j](x := k - 500);
    END_FOR;
  END_FOR;
  j := 0;
  WHILE j < 100 DO
    j := j + 1;
    IF j = 90 THEN EXIT; END_IF;
  END_WHILE;
  %QD0 := DINT_TO_DWORD(total);
END_PROGRAM
