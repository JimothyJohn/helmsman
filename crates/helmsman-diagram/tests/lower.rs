//! Lowering, statement kind by statement kind and expression form by
//! expression form, asserted on the diagram's structure (see
//! `common::shape`), not on pixels.

mod common;

use common::{build, prog, program_shape, shape};
use helmsman_diagram::{NodeKind, RegionKind};

fn lowered(vars: &str, body: &str) -> String {
    program_shape(&build(&prog(vars, body)).1)
}

#[test]
fn assignment_reads_left_operators_middle_write_right() {
    assert_eq!(
        lowered("a, b, c : BOOL;", "c := a AND NOT b;"),
        "\
#0 read A
#1 read B
#2 op NOT <- #1
#3 op AND <- #0 #2
#4 write C := <- #3
"
    );
}

#[test]
fn a_variable_read_twice_in_one_statement_is_one_node() {
    assert_eq!(
        lowered("a, b : INT;", "b := a * a;"),
        "\
#0 read A
#1 op * <- #0 #0
#2 write B := <- #1
"
    );
}

#[test]
fn literals_comparisons_and_functions() {
    assert_eq!(
        lowered(
            "x : REAL; n : INT; hot : BOOL; t : TIME;",
            "hot := ABS(x) >= 2.5 OR n <> -3;\nt := T#250ms;\nn := LIMIT(0, n, 10);"
        ),
        "\
#0 read X
#1 func ABS <- #0
#2 const 2.5
#3 op >= <- #1 #2
#4 read N
#5 const -3
#6 op <> <- #4 #5
#7 op OR <- #3 #6
#8 write HOT := <- #7
#9 const T#250ms
#10 write T := <- #9
#11 const 0
#12 read N
#13 const 10
#14 func LIMIT <- #11 #12 #13
#15 write N := <- #14
"
    );
}

#[test]
fn a_write_feeds_later_reads_of_the_same_path() {
    assert_eq!(
        lowered("a, b, c : INT;", "b := a + 1;\nc := b * 2;"),
        "\
#0 read A
#1 const 1
#2 op + <- #0 #1
#3 write B := <- #2
#4 const 2
#5 op * <- #3 #4
#6 write C := <- #5
"
    );
}

#[test]
fn fb_call_inputs_and_outputs_read_later_are_ports() {
    let (_, m) = build(&prog(
        "go, lamp : BOOL; t : TON; et : TIME;",
        "t(IN := go, PT := T#2s, ET => et);\nlamp := t.Q AND go;",
    ));
    assert_eq!(
        program_shape(&m),
        "\
#0 read GO
#1 const T#2s
#2 fb T : TON <- #0 #1
#3 write ET := <- #2.ET
#4 read GO
#5 op AND <- #2.Q #4
#6 write LAMP := <- #5
"
    );
    let d = &m.program().unwrap().diagram;
    assert_eq!(d.nodes[2].ins, ["IN", "PT"]);
    assert_eq!(d.nodes[2].outs, ["ET", "Q"]);
    assert_eq!(
        d.stmts[0].roots.len(),
        2,
        "the call and its => write end the statement"
    );
}

#[test]
fn an_fb_output_read_before_the_call_is_a_plain_read() {
    assert_eq!(
        lowered(
            "pulse : TON; on : BOOL;",
            "pulse(IN := NOT pulse.Q, PT := T#500ms);\nIF pulse.Q THEN on := NOT on; END_IF;"
        ),
        "\
#0 read PULSE.Q
#1 op NOT <- #0
#2 const T#500ms
#3 fb PULSE : TON <- #1 #2
#4 read ON [R0]
#5 op NOT [R0] <- #4
#6 write ON := [R0] <- #5
R0 IF PULSE.Q gate=#3 body=1
"
    );
}

#[test]
fn if_elsif_else_is_one_gated_region_per_arm() {
    let (_, m) = build(&prog(
        "a, b : BOOL; n : INT;",
        "IF a THEN\n n := 1;\nELSIF b AND n > 2 THEN\n n := 2;\nELSE\n n := 3;\nEND_IF;\nn := n + 1;",
    ));
    assert_eq!(
        program_shape(&m),
        "\
#0 read A
#1 const 1 [R0]
#2 write N := [R0] <- #1
#3 read B
#4 read N
#5 const 2
#6 op > <- #4 #5
#7 op AND <- #3 #6
#8 const 2 [R1]
#9 write N := [R1] <- #8
#10 const 3 [R2]
#11 write N := [R2] <- #10
#12 read N
#13 const 1
#14 op + <- #12 #13
#15 write N := <- #14
R0 IF A gate=#0 body=1
R1 ELSIF B AND N > 2 gate=#7 body=1
R2 ELSE gate=- body=1
",
        "after the IF, N is read as the variable: which arm wrote it is the trace's business"
    );
    let d = &m.program().unwrap().diagram;
    let kinds: Vec<RegionKind> = d.regions.iter().map(|r| r.kind).collect();
    assert_eq!(kinds, [RegionKind::If, RegionKind::Elsif, RegionKind::Else]);
    let arms: Vec<usize> = d.regions.iter().map(|r| r.arm).collect();
    assert_eq!(arms, [0, 1, 2]);
    assert!(d.regions.iter().all(|r| r.stmt == d.stmts[0].id));
}

#[test]
fn case_arms_share_the_selector_as_their_gate() {
    assert_eq!(
        lowered(
            "s, n : INT;",
            "CASE s + 1 OF\n 1, 3..5: n := 10;\n 2: n := 20;\nELSE\n n := 0;\nEND_CASE;"
        ),
        "\
#0 read S
#1 const 1
#2 op + <- #0 #1
#3 const 10 [R0]
#4 write N := [R0] <- #3
#5 const 20 [R1]
#6 write N := [R1] <- #5
#7 const 0 [R2]
#8 write N := [R2] <- #7
R0 CASE 1, 3..5 gate=#2 body=1
R1 CASE 2 gate=#2 body=1
R2 ELSE gate=#2 body=1
"
    );
}

#[test]
fn loops_are_regions_and_the_for_head_feeds_its_variable() {
    assert_eq!(
        lowered(
            "i, sum : INT; a : ARRAY[0..3] OF INT; go : BOOL;",
            "FOR i := 0 TO 3 BY 1 DO\n sum := sum + a[i];\n IF sum > 9 THEN EXIT; END_IF;\nEND_FOR;\n\
             WHILE go DO go := FALSE; END_WHILE;\nREPEAT sum := sum - 1; UNTIL sum < 0 END_REPEAT;"
        ),
        "\
#0 const 0
#1 const 3
#2 const 1
#3 for FOR I <- #0 #1 #2
#4 read SUM [R0]
#5 access A[I] [R0] <- #3.I
#6 op + [R0] <- #4 #5
#7 write SUM := [R0] <- #6
#8 const 9 [R0]
#9 op > [R0] <- #7 #8
#10 jump EXIT [R1]
#11 read GO
#12 const FALSE [R2]
#13 write GO := [R2] <- #12
#14 read SUM
#15 const 0
#16 op < <- #14 #15
#17 read SUM [R3]
#18 const 1 [R3]
#19 op - [R3] <- #17 #18
#20 write SUM := [R3] <- #19
R0 FOR I := 0 TO 3 BY 1 gate=#3 body=2
R1 IF SUM > 9 gate=#9 body=1
R2 WHILE GO gate=#11 body=1
R3 REPEAT … UNTIL SUM < 0 gate=#16 body=1
"
    );
}

#[test]
fn jumps_and_computed_writes() {
    let (_, m) = build(&prog(
        "i : INT; a : ARRAY[0..3] OF INT; g : ARRAY[0..1, 0..1] OF BOOL;",
        "a[i] := 5;\ng[1, 0] := TRUE;\na[2] := a[0];\nRETURN;",
    ));
    assert_eq!(
        program_shape(&m),
        "\
#0 const 5
#1 read I
#2 write A[I] := <- #0 #1
#3 const TRUE
#4 write G[1,0] := <- #3
#5 read A[0]
#6 write A[2] := <- #5
#7 jump RETURN
"
    );
    let d = &m.program().unwrap().diagram;
    assert_eq!(
        d.nodes[2].ins,
        ["", "I"],
        "the computed index is an input of the write"
    );
}

#[test]
fn a_computed_write_forgets_what_lowering_knew_about_the_array() {
    assert_eq!(
        lowered(
            "i, x : INT; a : ARRAY[0..3] OF INT;",
            "a[1] := 7;\na[i] := 0;\nx := a[1];"
        ),
        "\
#0 const 7
#1 write A[1] := <- #0
#2 const 0
#3 read I
#4 write A[I] := <- #2 #3
#5 read A[1]
#6 write X := <- #5
"
    );
}

#[test]
fn struct_members_bits_direct_addresses_and_enums() {
    let src = "TYPE color : (RED, GREEN); pt : STRUCT x : INT; y : INT; END_STRUCT; END_TYPE
PROGRAM t
VAR p : pt; w : WORD; c : color; b : BOOL; END_VAR
p.x := p.y;
b := w.3 AND %IX0.1;
c := GREEN;
%QX0.0 := c = color#RED;
END_PROGRAM";
    assert_eq!(
        program_shape(&build(src).1),
        "\
#0 read P.Y
#1 write P.X := <- #0
#2 read W.3
#3 read %IX0.1
#4 op AND <- #2 #3
#5 write B := <- #4
#6 const GREEN
#7 write C := <- #6
#8 const COLOR#RED
#9 op = <- #7 #8
#10 write %QX0.0 := <- #9
"
    );
}

#[test]
fn every_user_pou_is_lowered_and_function_blocks_know_their_type() {
    let src = "FUNCTION_BLOCK motor
VAR_INPUT run : BOOL; END_VAR
VAR_OUTPUT on : BOOL; END_VAR
on := run;
END_FUNCTION_BLOCK
FUNCTION twice : INT
VAR_INPUT x : INT; END_VAR
twice := x * 2;
END_FUNCTION
PROGRAM t
VAR m : motor; ms : ARRAY[0..1] OF motor; n : INT; END_VAR
m(run := TRUE);
ms[1](run := m.on);
n := twice(x := n);
END_PROGRAM";
    let (_, m) = build(src);
    let names: Vec<&str> = m.pous.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["MOTOR", "TWICE", "T"]);
    assert_eq!(
        shape(&m.pou("motor").unwrap().diagram),
        "#0 read RUN\n#1 write ON := <- #0\n"
    );
    assert_eq!(
        program_shape(&m),
        "\
#0 const TRUE
#1 fb M : MOTOR <- #0
#2 fb MS[1] : MOTOR <- #1.ON
#3 read N
#4 func TWICE <- #3
#5 write N := <- #4
"
    );
    assert_eq!(m.instances(), [("M".to_string(), "MOTOR".to_string())]);
    let d = &m.program().unwrap().diagram;
    assert!(
        matches!(&d.nodes[2].kind, NodeKind::Fb { instance, fb } if instance == "MS[1]" && fb == "MOTOR")
    );
}

#[test]
fn every_node_region_and_statement_maps_to_a_statement_of_its_pou() {
    let src = prog(
        "a, b : BOOL; n, i : INT; t : TON;",
        "t(IN := a);\nIF t.Q THEN\n CASE n OF 1: b := TRUE; ELSE FOR i := 1 TO 2 DO n := n + i; END_FOR; END_CASE;\nEND_IF;",
    );
    let (p, m) = build(&src);
    let map = p.trace_map();
    let d = &m.program().unwrap().diagram;
    let ours: Vec<_> = map.in_pou("t").collect();
    assert_eq!(
        d.stmts.iter().map(|s| s.id).collect::<Vec<_>>(),
        ours,
        "one per statement, source order"
    );
    for n in &d.nodes {
        assert!(ours.contains(&n.stmt), "node {n:?}");
    }
    for r in &d.regions {
        assert!(ours.contains(&r.stmt), "region {r:?}");
        let info = map.stmt(r.stmt).unwrap();
        assert_eq!(
            (info.line, info.col),
            (
                d.stmts.iter().find(|s| s.id == r.stmt).unwrap().pos.line,
                d.stmts.iter().find(|s| s.id == r.stmt).unwrap().pos.col
            )
        );
    }
    // A node inside a region was lowered from a statement inside it.
    for n in &d.nodes {
        if let Some(r) = n.region {
            let parent = map.stmt(n.stmt).unwrap().parent.unwrap();
            assert_eq!(parent, (d.region(r).stmt, d.region(r).arm), "node {n:?}");
        }
    }
}

#[test]
fn a_trace_map_from_another_source_is_refused_not_misread() {
    let (p, _) = build(&prog("a : BOOL;", "a := TRUE;"));
    let other = prog("a : BOOL;", "\n\na := FALSE;");
    let e = helmsman_diagram::lower(&other, &p.trace_map()).unwrap_err();
    assert!(e.to_string().contains("not in the trace map"), "{e}");
    let e = helmsman_diagram::lower("PROGRAM", &p.trace_map()).unwrap_err();
    assert!(matches!(e, helmsman_diagram::LowerError::Syntax(_)), "{e}");
}

#[test]
fn lowering_against_a_statement_table_matches_lowering_against_the_map() {
    use helmsman_diagram::{StmtPos, lower_table};
    let src = "FUNCTION_BLOCK m
VAR_INPUT run : BOOL; END_VAR
VAR_OUTPUT on : BOOL; END_VAR
IF run THEN on := TRUE; ELSE on := FALSE; END_IF;
END_FUNCTION_BLOCK
PROGRAM t
VAR a, b : BOOL; n : INT; x : m; t1 : TON; END_VAR
x(run := a);
t1(IN := x.on, PT := T#1s);
CASE n OF 0: b := t1.Q; ELSE b := FALSE; END_CASE;
FOR n := 0 TO 3 DO a := NOT a; END_FOR;
END_PROGRAM
";
    let (p, from_map) = build(src);
    let map = p.trace_map();
    let table: Vec<StmtPos> = map
        .stmts()
        .iter()
        .map(|s| StmtPos {
            pou: s.pou.clone(),
            system: s.system,
            line: s.line,
            col: s.col,
        })
        .collect();
    assert_eq!(lower_table(src, &table).unwrap(), from_map);
    // A table from another source does not map this one.
    let short = &table[..table.len() - 1];
    assert!(lower_table(src, short).is_err());
}
