//! The `PLCopen` guideline lints (`helmsman_iec::lint`): what each one flags
//! and what it leaves alone, the override pragma, and that the pass never
//! panics and gives the same findings in the same order.

use helmsman_iec::lint::{self, Lint, lints};
use proptest::prelude::*;

/// A PROGRAM of `vars` (a `go : BOOL;` among them is a module input) and
/// `body`; the body starts on line 5.
fn prog(vars: &str, body: &str) -> String {
    let input = if vars.contains("go : BOOL;") {
        "VAR_INPUT go : BOOL; END_VAR"
    } else {
        ""
    };
    let vars = vars.replace("go : BOOL;", "");
    format!("PROGRAM p {input}\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM\n")
}

fn only(src: &str, id: &str) -> Vec<Lint> {
    lints(src).into_iter().filter(|l| l.id == id).collect()
}

fn none_of(src: &str, id: &str) {
    let got = only(src, id);
    assert!(got.is_empty(), "{id} flagged {got:#?} in\n{src}");
}

// ---------------------------------------------------------------- the ids

#[test]
fn the_ids_are_stable() {
    assert_eq!(lint::FB_IN_CONDITIONAL, "HELMSMAN-L1");
    assert_eq!(lint::OUTPUT_WRITTEN_TWICE, "PLCOPEN-CP12");
    assert_eq!(lint::READ_BEFORE_WRITE, "PLCOPEN-CP3");
    assert_eq!(lint::REAL_EQUALITY, "PLCOPEN-CP8");
    assert_eq!(lint::FOR_CHANGED, "PLCOPEN-L12");
    assert_eq!(lint::UNUSED_VARIABLE, "PLCOPEN-CP24");
    assert_eq!(lint::UNREACHABLE, "PLCOPEN-CP2");
    assert_eq!(lint::PRAGMA_UNUSED, "HELMSMAN-L2");
    let blocking: Vec<&str> = lint::IDS
        .iter()
        .filter(|(_, b, _)| *b)
        .map(|(i, _, _)| *i)
        .collect();
    assert_eq!(
        blocking,
        ["HELMSMAN-L1", "PLCOPEN-CP12"],
        "only the first two block"
    );
}

#[test]
fn a_clean_program_has_no_findings() {
    let src = prog(
        "go : BOOL; lamp : BOOL; t : TON;",
        "t(IN := go, PT := T#1s);\nlamp := t.Q;",
    );
    assert_eq!(lints(&src), []);
}

#[test]
fn a_source_that_does_not_parse_has_no_findings() {
    assert_eq!(lints("PROGRAM p IF END"), []);
    assert_eq!(lints(""), []);
}

// ---------------------------------------------------------- HELMSMAN-L1

const L1: &str = "HELMSMAN-L1";

#[test]
fn a_timer_inside_an_if_is_flagged_and_blocks() {
    let src = prog(
        "go : BOOL; t : TON;",
        "IF go THEN\n  t(IN := TRUE, PT := T#1s);\nEND_IF;",
    );
    let f = only(&src, L1);
    assert_eq!(f.len(), 1);
    assert!(f[0].blocking && f[0].overridden.is_none());
    assert_eq!(f[0].pos.line, 6, "at the call");
    assert!(f[0].msg.contains("TON"), "{}", f[0].msg);
}

#[test]
fn an_unconditional_call_is_not_flagged() {
    let src = prog("go : BOOL; t : TON;", "t(IN := go, PT := T#1s);");
    none_of(&src, L1);
}

#[test]
fn a_call_in_both_arms_is_not_flagged() {
    let src = prog(
        "go : BOOL; t : TON;",
        "IF go THEN t(IN := TRUE, PT := T#1s); ELSE t(IN := FALSE, PT := T#1s); END_IF;",
    );
    none_of(&src, L1);
}

#[test]
fn a_call_in_only_some_arms_is_flagged() {
    let src = prog(
        "n : INT; t : TON;",
        "CASE n OF 1: t(IN := TRUE, PT := T#1s); 2: n := 3; ELSE n := 0; END_CASE;",
    );
    assert_eq!(only(&src, L1).len(), 1);
}

#[test]
fn a_call_in_every_case_arm_and_the_else_is_not_flagged() {
    let src = prog(
        "n : INT; t : TON;",
        "CASE n OF 1: t(IN := TRUE, PT := T#1s); ELSE t(IN := FALSE, PT := T#1s); END_CASE;",
    );
    none_of(&src, L1);
}

#[test]
fn a_timer_in_a_case_state_arm_is_flagged_by_design() {
    let src = prog(
        "state : INT; t : TON;",
        "CASE state OF 0: t(IN := FALSE, PT := T#2s); 1: t(IN := TRUE, PT := T#2s); END_CASE;",
    );
    assert_eq!(only(&src, L1).len(), 2);
}

#[test]
fn a_call_nested_in_if_in_case_in_for_is_flagged() {
    let src = prog(
        "n : INT; i : INT; go : BOOL; t : TON;",
        "FOR i := 0 TO 3 DO\n CASE n OF 1:\n  IF go THEN\n   t(IN := TRUE, PT := T#1s);\n  END_IF;\n END_CASE;\nEND_FOR;",
    );
    let f = only(&src, L1);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].pos.line, 8);
}

#[test]
fn a_call_in_a_loop_alone_is_not_flagged() {
    let src = prog(
        "i : INT; t : TON;",
        "FOR i := 0 TO 3 DO t(IN := TRUE, PT := T#1s); END_FOR;",
    );
    none_of(&src, L1);
}

#[test]
fn an_instance_in_a_structure_or_an_array_is_found() {
    let src = "TYPE rig : STRUCT t : TON; END_STRUCT; END_TYPE\n\
        PROGRAM p\nVAR r : rig; ts : ARRAY[0..1] OF TON; go : BOOL; END_VAR\n\
        IF go THEN\n r.t(IN := TRUE, PT := T#1s);\n ts[1](IN := TRUE, PT := T#1s);\nEND_IF;\nEND_PROGRAM\n";
    assert_eq!(only(src, L1).len(), 2, "{:#?}", lints(src));
}

#[test]
fn a_function_call_in_an_if_is_not_a_function_block() {
    let src = prog("go : BOOL; x : REAL;", "IF go THEN x := ABS(x); END_IF;");
    none_of(&src, L1);
}

#[test]
fn another_instance_called_outside_does_not_cover_it() {
    let src = prog(
        "go : BOOL; a : TON; b : TON;",
        "a(IN := go, PT := T#1s);\nIF go THEN b(IN := TRUE, PT := T#1s); END_IF;",
    );
    assert_eq!(only(&src, L1).len(), 1);
}

#[test]
fn a_user_function_block_called_in_an_if_is_flagged() {
    let src = "FUNCTION_BLOCK blk VAR_INPUT a : BOOL; END_VAR VAR_OUTPUT q : BOOL; END_VAR q := a; END_FUNCTION_BLOCK\n\
        PROGRAM p VAR b : blk; go : BOOL; END_VAR IF go THEN b(a := TRUE); END_IF; END_PROGRAM\n";
    assert_eq!(only(src, L1).len(), 1);
}

// ------------------------------------------------------- PLCOPEN-CP12

const CP12: &str = "PLCOPEN-CP12";

fn outprog(body: &str) -> String {
    format!(
        "PROGRAM p\nVAR_INPUT go : BOOL; n : INT; END_VAR\nVAR_OUTPUT out : BOOL; END_VAR\nVAR i : INT; END_VAR\n{body}\nEND_PROGRAM\n"
    )
}

#[test]
fn an_output_written_twice_in_sequence_is_flagged_and_blocks() {
    let src = outprog("out := FALSE;\nout := go;");
    let f = only(&src, CP12);
    assert_eq!(f.len(), 1);
    assert!(f[0].blocking);
    assert_eq!(f[0].pos.line, 6, "at the second place");
    assert!(f[0].msg.contains("line 5, 6"), "{}", f[0].msg);
}

#[test]
fn the_default_then_override_pattern_is_flagged() {
    let src = outprog("out := FALSE;\nIF go THEN out := TRUE; END_IF;");
    assert_eq!(only(&src, CP12).len(), 1);
}

#[test]
fn both_arms_of_one_if_are_one_place_per_path() {
    let src = outprog("IF go THEN out := TRUE; ELSIF n > 2 THEN out := FALSE; ELSE out := go; END_IF;");
    none_of(&src, CP12);
}

#[test]
fn the_arms_of_one_case_are_one_place_per_path() {
    let src = outprog("CASE n OF 1: out := TRUE; 2..3: out := FALSE; ELSE out := go; END_CASE;");
    none_of(&src, CP12);
}

#[test]
fn a_write_inside_a_branch_and_one_outside_are_two_places() {
    let src = outprog("IF go THEN out := TRUE; END_IF;\nout := NOT go;");
    assert_eq!(only(&src, CP12).len(), 1);
}

#[test]
fn nested_branches_of_different_arms_are_exclusive() {
    let src =
        outprog("IF go THEN IF n > 1 THEN out := TRUE; ELSE out := FALSE; END_IF; ELSE out := go; END_IF;");
    none_of(&src, CP12);
}

#[test]
fn a_return_in_the_branch_makes_the_later_write_exclusive() {
    let src = outprog("IF go THEN out := TRUE; RETURN; END_IF;\nout := FALSE;");
    none_of(&src, CP12);
}

#[test]
fn a_write_before_the_returning_branch_still_counts() {
    let src = outprog("out := FALSE;\nIF go THEN out := TRUE; RETURN; END_IF;");
    assert_eq!(only(&src, CP12).len(), 1);
}

#[test]
fn one_write_in_a_loop_is_one_place() {
    let src = outprog("FOR i := 0 TO 3 DO out := i > 1; END_FOR;");
    none_of(&src, CP12);
}

#[test]
fn a_local_written_twice_is_not_an_output() {
    let src = prog("x : INT;", "x := 1;\nx := 2;");
    none_of(&src, CP12);
}

#[test]
fn a_located_output_is_one_even_through_its_address() {
    let src = prog(
        "lamp AT %QX0.0 : BOOL; go : BOOL;",
        "lamp := go;\n%QX0.0 := NOT go;",
    );
    assert_eq!(only(&src, CP12).len(), 1, "{:#?}", lints(&src));
}

#[test]
fn a_located_variable_written_twice_is_flagged() {
    let src = prog("lamp AT %QX0.1 : BOOL; go : BOOL;", "lamp := go;\nlamp := FALSE;");
    assert_eq!(only(&src, CP12).len(), 1);
}

#[test]
fn a_located_input_is_not_an_output() {
    let src = prog("sw AT %IX0.0 : BOOL;", "sw := TRUE;\nsw := FALSE;");
    none_of(&src, CP12);
}

#[test]
fn members_of_an_output_are_apart_unless_the_same() {
    let src = "TYPE pair : STRUCT a : INT; b : INT; END_STRUCT; END_TYPE\n\
        PROGRAM p VAR_OUTPUT o : pair; END_VAR o.a := 1; o.b := 2; END_PROGRAM\n";
    none_of(src, CP12);
    let src = "TYPE pair : STRUCT a : INT; b : INT; END_STRUCT; END_TYPE\n\
        PROGRAM p VAR_OUTPUT o : pair; END_VAR o.a := 1; o.a := 2; END_PROGRAM\n";
    assert_eq!(only(src, CP12).len(), 1);
    let src = "TYPE pair : STRUCT a : INT; b : INT; END_STRUCT; END_TYPE\n\
        PROGRAM p VAR_OUTPUT o : pair; END_VAR VAR q : pair; END_VAR o.b := 2; o := q; END_PROGRAM\n";
    assert_eq!(only(src, CP12).len(), 1, "a whole write covers every member");
}

#[test]
fn array_elements_are_apart_unless_the_index_may_match() {
    let apart = "PROGRAM p VAR_OUTPUT o : ARRAY[0..3] OF INT; END_VAR o[0] := 1; o[1] := 2; END_PROGRAM\n";
    none_of(apart, CP12);
    let same = "PROGRAM p VAR_OUTPUT o : ARRAY[0..3] OF INT; END_VAR o[1] := 1; o[1] := 2; END_PROGRAM\n";
    assert_eq!(only(same, CP12).len(), 1);
    let unknown = "PROGRAM p VAR_OUTPUT o : ARRAY[0..3] OF INT; END_VAR VAR i : INT; END_VAR o[i] := 1; o[2] := 2; END_PROGRAM\n";
    assert_eq!(only(unknown, CP12).len(), 1);
}

#[test]
fn bits_of_an_output_word_are_apart() {
    let src = "PROGRAM p VAR_OUTPUT w : WORD; END_VAR w.0 := TRUE; w.1 := FALSE; END_PROGRAM\n";
    none_of(src, CP12);
    let src = "PROGRAM p VAR_OUTPUT w : WORD; END_VAR w.0 := TRUE; w := 0; END_PROGRAM\n";
    assert_eq!(only(src, CP12).len(), 1);
}

#[test]
fn an_output_bound_by_a_call_counts_as_a_write() {
    let src = outprog("out := FALSE;\ntr(CLK := go, Q => out);")
        .replace("VAR i : INT;", "VAR i : INT; tr : R_TRIG;");
    assert_eq!(only(&src, CP12).len(), 1, "{:#?}", lints(&src));
}

#[test]
fn an_output_passed_to_an_in_out_counts_as_a_write() {
    let src = "FUNCTION_BLOCK flip VAR_IN_OUT x : BOOL; END_VAR x := NOT x; END_FUNCTION_BLOCK\n\
        PROGRAM p VAR_OUTPUT out : BOOL; END_VAR VAR f : flip; END_VAR out := TRUE; f(x := out); END_PROGRAM\n";
    assert_eq!(only(src, CP12).len(), 1, "{:#?}", lints(src));
}

#[test]
fn function_block_outputs_are_not_physical_outputs() {
    let src = "FUNCTION_BLOCK f VAR_OUTPUT q : BOOL; END_VAR q := FALSE; q := TRUE; END_FUNCTION_BLOCK\n\
        PROGRAM p VAR x : f; END_VAR x(); END_PROGRAM\n";
    none_of(src, CP12);
}

#[test]
fn a_global_located_output_is_found_by_name() {
    let src = "VAR_GLOBAL lamp AT %QX0.2 : BOOL; END_VAR\n\
        PROGRAM p VAR_EXTERNAL lamp : BOOL; END_VAR VAR go : BOOL; END_VAR lamp := go; lamp := FALSE; END_PROGRAM\n";
    assert_eq!(only(src, CP12).len(), 1, "{:#?}", lints(src));
}

// -------------------------------------------------------- PLCOPEN-CP3

const CP3: &str = "PLCOPEN-CP3";

#[test]
fn a_variable_nothing_assigns_is_flagged() {
    let src = prog("x : INT; y : INT;", "y := x + 1;");
    let f = only(&src, CP3);
    assert_eq!(f.len(), 1);
    assert!(!f[0].blocking);
}

#[test]
fn an_initial_value_or_an_assignment_clears_it() {
    none_of(&prog("x : INT := 5; y : INT;", "y := x + 1;"), CP3);
    none_of(&prog("x : INT; y : INT;", "x := 1;\ny := x + 1;"), CP3);
}

#[test]
fn a_counter_that_reads_itself_is_not_flagged() {
    none_of(&prog("n : INT;", "n := n + 1;"), CP3);
    none_of(&prog("on : BOOL;", "on := NOT on;"), CP3);
}

#[test]
fn a_temp_read_before_it_is_assigned_is_flagged() {
    let src = "PROGRAM p VAR_TEMP t : INT; END_VAR VAR y : INT; END_VAR y := t; t := 1; END_PROGRAM\n";
    assert_eq!(only(src, CP3).len(), 1);
    let ok = "PROGRAM p VAR_TEMP t : INT; END_VAR VAR y : INT; END_VAR t := 1; y := t; END_PROGRAM\n";
    none_of(ok, CP3);
}

#[test]
fn a_call_output_counts_as_an_assignment() {
    let src = prog(
        "q : BOOL; y : BOOL; r : R_TRIG; go : BOOL;",
        "r(CLK := go, Q => q);\ny := q;",
    );
    none_of(&src, CP3);
}

#[test]
fn inputs_located_retained_and_typed_defaults_are_not_flagged() {
    let src = "TYPE lim : REAL := 250.0; END_TYPE\n\
        PROGRAM p VAR_INPUT a : INT; END_VAR VAR s AT %IX0.0 : BOOL; l : lim; k : INT; y : INT; END_VAR\n\
        VAR RETAIN r : INT; END_VAR y := a + k; y := r; IF s THEN y := REAL_TO_INT(l); END_IF; k := 1; END_PROGRAM\n";
    none_of(src, CP3);
}

// -------------------------------------------------------- PLCOPEN-CP8

const CP8: &str = "PLCOPEN-CP8";

#[test]
fn equality_on_real_is_flagged() {
    let src = prog("a : REAL; b : REAL; ok : BOOL;", "ok := a = b;\nok := a <> 1.5;");
    assert_eq!(only(&src, CP8).len(), 2);
}

#[test]
fn ordering_and_zero_and_integers_are_not_flagged() {
    let src = prog(
        "a : REAL; i : INT; ok : BOOL;",
        "ok := a < 1.5;\nok := a >= 2.0;\nok := a = 0.0;\nok := i = 3;\nok := 0 <> a;",
    );
    none_of(&src, CP8);
}

#[test]
fn equality_through_a_conversion_or_arithmetic_or_a_member_is_flagged() {
    let src = "TYPE m : STRUCT t : LREAL; END_STRUCT; END_TYPE\n\
        PROGRAM p VAR i : INT; r : REAL; s : m; arr : ARRAY[0..2] OF REAL; ok : BOOL; END_VAR\n\
        ok := INT_TO_REAL(i) = r;\nok := s.t = 3.3;\nok := arr[1] <> r;\nok := (r * 2.0) = 4.0;\nok := SQRT(r) = 2.0;\nEND_PROGRAM\n";
    assert_eq!(only(src, CP8).len(), 5, "{:#?}", lints(src));
}

#[test]
fn a_real_converted_to_an_integer_is_not_flagged() {
    let src = prog(
        "r : REAL; ok : BOOL;",
        "ok := REAL_TO_INT(r) = 3;\nok := TRUNC(r) = 3;",
    );
    none_of(&src, CP8);
}

#[test]
fn equality_through_an_alias_type_is_flagged() {
    let src =
        "TYPE temp : REAL; END_TYPE\nPROGRAM p VAR a : temp; ok : BOOL; END_VAR ok := a = 2.5; END_PROGRAM\n";
    assert_eq!(only(src, CP8).len(), 1);
}

// -------------------------------------------------------- PLCOPEN-L12

const L12: &str = "PLCOPEN-L12";

#[test]
fn changing_the_loop_variable_is_flagged() {
    let src = prog(
        "i : INT; s : INT;",
        "FOR i := 0 TO 10 DO\n s := s + 1;\n i := 10;\nEND_FOR;",
    );
    let f = only(&src, L12);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].pos.line, 7);
    assert!(!f[0].blocking);
}

#[test]
fn changing_a_bound_is_flagged() {
    let src = prog(
        "i : INT; n : INT := 5; s : INT;",
        "FOR i := 0 TO n DO\n n := n - 1;\nEND_FOR;\ns := n;",
    );
    assert_eq!(only(&src, L12).len(), 1);
    let by = prog(
        "i : INT; k : INT := 2;",
        "FOR i := 0 TO 10 BY k DO k := 3; END_FOR;",
    );
    assert_eq!(only(&by, L12).len(), 1);
    let from = prog("i : INT; k : INT := 2;", "FOR i := k TO 10 DO k := 3; END_FOR;");
    assert_eq!(only(&from, L12).len(), 1);
}

#[test]
fn a_bound_through_a_member_or_an_element_is_followed() {
    let src = "TYPE cfg : STRUCT n : INT; m : INT; END_STRUCT; END_TYPE\n\
        PROGRAM p VAR c : cfg; i : INT; a : ARRAY[0..3] OF INT; END_VAR\n\
        FOR i := 0 TO c.n DO c.n := 1; END_FOR;\n\
        FOR i := 0 TO c.n DO c.m := 1; END_FOR;\n\
        FOR i := 0 TO a[2] DO a[0] := 1; END_FOR;\n\
        FOR i := 0 TO a[2] DO a[2] := 1; END_FOR;\nEND_PROGRAM\n";
    let f = only(src, L12);
    let lines: Vec<u32> = f.iter().map(|l| l.pos.line).collect();
    assert_eq!(lines, [3, 6], "{f:#?}");
}

#[test]
fn a_bound_changed_through_a_second_name_at_the_same_address_is_flagged() {
    let src = prog(
        "i : INT; n AT %MW0 : INT; alias AT %MW0 : INT;",
        "FOR i := 0 TO n DO alias := 1; END_FOR;",
    );
    assert_eq!(only(&src, L12).len(), 1, "{:#?}", lints(&src));
}

#[test]
fn a_bound_changed_by_a_call_output_is_flagged() {
    let src = prog(
        "i : INT; n : INT; c : CTU; go : BOOL;",
        "FOR i := 0 TO n DO c(CU := go, PV := 3, CV => n); END_FOR;",
    );
    assert_eq!(only(&src, L12).len(), 1, "{:#?}", lints(&src));
}

#[test]
fn a_nested_for_on_the_same_variable_is_flagged() {
    let src = prog(
        "i : INT;",
        "FOR i := 0 TO 3 DO FOR i := 0 TO 2 DO END_FOR; END_FOR;",
    );
    assert_eq!(only(&src, L12).len(), 1);
}

#[test]
fn reading_and_unrelated_writes_are_fine() {
    let src = prog(
        "i : INT; n : INT := 5; s : INT; a : ARRAY[0..9] OF INT;",
        "FOR i := 0 TO n DO a[i] := i; s := s + i; END_FOR;",
    );
    none_of(&src, L12);
}

// ------------------------------------------------------- PLCOPEN-CP24

const CP24: &str = "PLCOPEN-CP24";

#[test]
fn an_unused_local_is_flagged_with_its_spelling() {
    let src = prog("spare : INT; used : INT;", "used := 1;");
    let f = only(&src, CP24);
    assert_eq!(f.len(), 1);
    assert!(f[0].msg.starts_with("spare "), "{}", f[0].msg);
    assert_eq!(f[0].pos.line, 3);
}

#[test]
fn interface_located_and_used_variables_are_not_flagged() {
    let src = "PROGRAM p VAR_INPUT a : BOOL; END_VAR VAR_OUTPUT b : BOOL; END_VAR VAR s AT %IX0.0 : BOOL; k : INT := 1; j : INT := k; t : TON; END_VAR\n\
        t(IN := TRUE, PT := T#1s);\nEND_PROGRAM\n";
    let f = only(src, CP24);
    assert_eq!(f.len(), 1, "{f:#?}");
    assert!(
        f[0].msg.starts_with("j "),
        "j is declared and not used; k is used by j's value"
    );
}

#[test]
fn an_unused_global_is_flagged_and_a_used_one_is_not() {
    let src =
        "VAR_GLOBAL g : INT; h : INT; END_VAR\nPROGRAM p VAR_EXTERNAL h : INT; END_VAR h := 1; END_PROGRAM\n";
    let f = only(src, CP24);
    assert_eq!(f.len(), 1);
    assert!(f[0].msg.starts_with("global g"), "{}", f[0].msg);
}

// -------------------------------------------------------- PLCOPEN-CP2

const CP2: &str = "PLCOPEN-CP2";

#[test]
fn code_after_return_is_unreachable() {
    let src = prog("x : INT;", "RETURN;\nx := 1;");
    let f = only(&src, CP2);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].pos.line, 6);
}

#[test]
fn code_after_an_if_that_always_returns_is_unreachable() {
    let src = prog(
        "x : INT; go : BOOL;",
        "IF go THEN RETURN; ELSE RETURN; END_IF;\nx := 1;",
    );
    assert_eq!(only(&src, CP2).len(), 1);
    let no_else = prog("x : INT; go : BOOL;", "IF go THEN RETURN; END_IF;\nx := 1;");
    none_of(&no_else, CP2);
}

#[test]
fn code_after_exit_in_a_loop_is_unreachable() {
    let src = prog("i : INT; x : INT;", "FOR i := 0 TO 3 DO EXIT; x := 1; END_FOR;");
    assert_eq!(only(&src, CP2).len(), 1);
}

#[test]
fn a_false_condition_and_what_follows_a_true_one_are_unreachable() {
    let src = prog("x : INT;", "IF FALSE THEN x := 1; END_IF;");
    assert_eq!(only(&src, CP2).len(), 1);
    let src = prog(
        "x : INT; go : BOOL;",
        "IF TRUE THEN x := 1; ELSIF go THEN x := 2; ELSE x := 3; END_IF;",
    );
    assert_eq!(only(&src, CP2).len(), 2);
    let src = prog("x : INT;", "WHILE FALSE DO x := 1; END_WHILE;");
    assert_eq!(only(&src, CP2).len(), 1);
}

#[test]
fn ordinary_code_is_reachable() {
    let src = prog(
        "x : INT; go : BOOL;",
        "IF go THEN x := 1; ELSE x := 2; END_IF;\nx := 3;",
    );
    none_of(&src, CP2);
}

// ----------------------------------------------------------- recursion

#[test]
fn recursion_across_three_pous_is_still_an_error_not_a_lint() {
    let src = "FUNCTION a : INT VAR_INPUT x : INT; END_VAR a := b(x); END_FUNCTION\n\
        FUNCTION b : INT VAR_INPUT x : INT; END_VAR b := c(x); END_FUNCTION\n\
        FUNCTION c : INT VAR_INPUT x : INT; END_VAR c := a(x); END_FUNCTION\n\
        PROGRAM p VAR y : INT; END_VAR y := a(1); END_PROGRAM\n";
    let sizes = helmsman_iec::ImageSizes { i: 8, q: 8, m: 8 };
    let Err(errs) = helmsman_iec::compile(src, sizes) else {
        panic!("recursion is refused");
    };
    assert!(
        errs.iter().any(|e| e.msg.contains("recursion is not allowed")),
        "{errs:?}"
    );
}

// ------------------------------------------------------------- pragmas

fn timer_in_if(pragma: &str) -> String {
    prog(
        "go : BOOL; t : TON;",
        &format!("IF go THEN\n  t(IN := TRUE, PT := T#1s); {pragma}\nEND_IF;"),
    )
}

#[test]
fn a_pragma_with_a_reason_overrides_the_blocking_finding() {
    let src = timer_in_if("{lint: off HELMSMAN-L1 \"imported, the branch is the enable\"}");
    let all = lints(&src);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(all[0].id, "HELMSMAN-L1");
    assert!(!all[0].blocking, "no longer blocks");
    assert_eq!(
        all[0].overridden.as_deref(),
        Some("imported, the branch is the enable")
    );
    assert!(all[0].text().contains("overridden: imported"), "still listed");
}

#[test]
fn the_pragma_is_read_in_any_case_and_spacing() {
    let src = timer_in_if("{ LINT:OFF   helmsman-l1   \"  why not  \" }");
    let all = lints(&src);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(all[0].overridden.as_deref(), Some("why not"));
}

#[test]
fn a_pragma_without_a_reason_does_not_override_and_is_reported() {
    for p in [
        "{lint: off HELMSMAN-L1}",
        "{lint: off HELMSMAN-L1 \"\"}",
        "{lint: off HELMSMAN-L1 \"   \"}",
        "{lint: off HELMSMAN-L1 why}",
    ] {
        let all = lints(&timer_in_if(p));
        assert!(
            all.iter()
                .any(|l| l.id == "HELMSMAN-L1" && l.blocking && l.overridden.is_none()),
            "{p}: {all:#?}"
        );
        assert!(
            all.iter().any(|l| l.id == "HELMSMAN-L2" && !l.blocking),
            "{p}: {all:#?}"
        );
    }
}

#[test]
fn a_pragma_with_an_unknown_id_does_not_override_and_is_reported() {
    let all = lints(&timer_in_if("{lint: off HELMSMAN-L99 \"x\"}"));
    assert!(all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking));
    let said = all.iter().find(|l| l.id == "HELMSMAN-L2").expect("reported");
    assert!(
        said.msg.contains("HELMSMAN-L99") && said.msg.contains("PLCOPEN-CP12"),
        "{}",
        said.msg
    );
}

#[test]
fn a_pragma_for_another_lint_does_not_override() {
    let all = lints(&timer_in_if("{lint: off PLCOPEN-CP12 \"x\"}"));
    assert!(all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking));
    assert!(
        all.iter().any(|l| l.id == "HELMSMAN-L2"),
        "no CP12 finding on its line: {all:#?}"
    );
}

#[test]
fn a_pragma_on_the_wrong_line_does_not_override() {
    let src = prog(
        "go : BOOL; t : TON;",
        "{lint: off HELMSMAN-L1 \"above\"}\nIF go THEN\n  t(IN := TRUE, PT := T#1s);\nEND_IF;",
    );
    let all = lints(&src);
    assert!(
        all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking),
        "{all:#?}"
    );
    assert!(all.iter().any(|l| l.id == "HELMSMAN-L2"));
}

#[test]
fn a_pragma_inside_a_comment_or_not_ours_is_ignored() {
    let src = timer_in_if("(* {lint: off HELMSMAN-L1 \"in a comment\"} *)");
    assert!(
        lints(&src)
            .iter()
            .all(|l| l.id != "HELMSMAN-L2" && l.overridden.is_none())
    );
    let src = timer_in_if("{attribute 'x'} // {lint: off HELMSMAN-L1 \"in a line comment\"}");
    let all = lints(&src);
    assert!(
        all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking),
        "{all:#?}"
    );
    assert!(all.iter().all(|l| l.id != "HELMSMAN-L2"), "{all:#?}");
}

#[test]
fn an_unterminated_pragma_is_a_compile_error_and_no_finding() {
    let src = timer_in_if("{lint: off HELMSMAN-L1 \"never closed\"");
    assert_eq!(lints(&src), []);
    let errs = helmsman_iec::compile(&src, helmsman_iec::ImageSizes { i: 8, q: 8, m: 8 }).err();
    assert!(errs.is_some_and(|e| e[0].msg.contains("unterminated pragma")));
}

#[test]
fn a_reason_of_ten_kilobytes_is_refused() {
    let long = "x".repeat(10_000);
    let all = lints(&timer_in_if(&format!("{{lint: off HELMSMAN-L1 \"{long}\"}}")));
    assert!(all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking));
    let said = all.iter().find(|l| l.id == "HELMSMAN-L2").expect("reported");
    assert!(
        said.msg.len() < 400,
        "the report is bounded: {} bytes",
        said.msg.len()
    );
    let at_limit = "x".repeat(lint::MAX_REASON);
    let all = lints(&timer_in_if(&format!("{{lint: off HELMSMAN-L1 \"{at_limit}\"}}")));
    assert!(
        all.iter()
            .any(|l| l.overridden.as_deref() == Some(at_limit.as_str()))
    );
}

#[test]
fn a_reason_with_a_control_character_is_refused() {
    let all = lints(&timer_in_if("{lint: off HELMSMAN-L1 \"two\nlines\"}"));
    assert!(
        all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking),
        "{all:#?}"
    );
    let all = lints(&timer_in_if("{lint: off HELMSMAN-L1 \"tab\there\"}"));
    assert!(
        all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking),
        "{all:#?}"
    );
}

#[test]
fn the_pragma_report_cannot_itself_be_overridden() {
    let all = lints(&timer_in_if("{lint: off HELMSMAN-L2 \"hush\"}"));
    assert!(
        all.iter()
            .any(|l| l.id == "HELMSMAN-L2" && l.overridden.is_none())
    );
    assert!(all.iter().any(|l| l.id == "HELMSMAN-L1" && l.blocking));
}

#[test]
fn a_pragma_overrides_an_advisory_lint_too() {
    let src = prog(
        "a : REAL := 1.0; ok : BOOL;",
        "ok := a = 1.5; {lint: off PLCOPEN-CP8 \"exact: a is a setpoint echo\"}",
    );
    let all = lints(&src);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert!(all[0].overridden.is_some());
}

#[test]
fn a_pragma_on_the_cp12_line_overrides_that_output() {
    let src = outprog("out := FALSE;\nout := go; {lint: off PLCOPEN-CP12 \"safety path writes it twice\"}");
    let all = only(&src, CP12);
    assert_eq!(all.len(), 1);
    assert!(!all[0].blocking && all[0].overridden.is_some());
}

// ------------------------------------------------- determinism, no panics

#[test]
fn the_findings_are_sorted_and_repeatable() {
    let src = outprog("out := FALSE;\nout := go;\nIF go THEN RETURN; END_IF;\nRETURN;\nout := TRUE;");
    let a = lints(&src);
    assert!(a.len() >= 2);
    assert_eq!(a, lints(&src));
    let mut sorted = a.clone();
    sorted.sort();
    assert_eq!(a, sorted);
}

const FRAGMENTS: &[&str] = &[
    "x := 1;",
    "out := NOT out;",
    "IF go THEN",
    "ELSIF n > 1 THEN",
    "ELSE",
    "END_IF;",
    "CASE n OF",
    "1:",
    "2..3:",
    "END_CASE;",
    "FOR i := 0 TO n DO",
    "END_FOR;",
    "WHILE go DO",
    "END_WHILE;",
    "REPEAT",
    "UNTIL go;",
    "t(IN := go, PT := T#1s);",
    "tr(CLK := go, Q => out);",
    "RETURN;",
    "EXIT;",
    "CONTINUE;",
    "r := r + 1.5;",
    "ok := r = 2.0;",
    "n := n + 1;",
    "{lint: off HELMSMAN-L1 \"x\"}",
    "{lint: off",
    "{",
    "}",
    "(* c *)",
    "// c",
    "o[i] := 1;",
    "s.a := 2;",
    "%QX0.0 := go;",
    "i := i + 1;",
    ";",
    "END_FOR",
    "x :=",
    ")",
    "'",
    "\"",
];

fn frame(body: &str) -> String {
    format!(
        "TYPE sp : STRUCT a : INT; END_STRUCT; END_TYPE\nVAR_GLOBAL g : INT; END_VAR\nPROGRAM p\nVAR_INPUT go : BOOL; n : INT; END_VAR\nVAR_OUTPUT out : BOOL; END_VAR\nVAR i : INT; x : INT; r : REAL; ok : BOOL; t : TON; tr : R_TRIG; o : ARRAY[0..3] OF INT; s : sp; END_VAR\n{body}\nEND_PROGRAM\n"
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn random_programs_never_panic_and_are_deterministic(parts in proptest::collection::vec(0..FRAGMENTS.len(), 0..30)) {
        let body: Vec<&str> = parts.iter().map(|&i| FRAGMENTS[i]).collect();
        let src = frame(&body.join("\n"));
        let a = lints(&src);
        prop_assert_eq!(&a, &lints(&src));
        let mut sorted = a.clone();
        sorted.sort();
        prop_assert_eq!(a, sorted);
    }

    #[test]
    fn token_soup_never_panics(s in "[ -~\n\t{}()*'\"/$#%:;=<>.,\\[\\]_a-zA-Z0-9]{0,300}") {
        let _ = lints(&s);
        let _ = lints(&frame(&s));
    }

    #[test]
    fn arbitrary_bytes_as_text_never_panic(s in "\\PC{0,200}") {
        let _ = lints(&s);
        let _ = lints(&frame(&format!("{{{s}}}")));
    }
}

// ------------------------------------------------- what the repo ships

fn st_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            st_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "st") {
            out.push(p);
        }
    }
}

/// Every `.st` file under crates/ and docs/ (bench programs, fixtures, the
/// library): no blocking finding; each finding is listed with `--nocapture`.
#[test]
fn the_repos_st_files_hold_no_blocking_finding() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for d in ["crates", "docs"] {
        st_files(&root.join(d), &mut files);
    }
    assert!(files.len() > 20, "found {} files", files.len());
    let mut bad = Vec::new();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let name = f.strip_prefix(&root).unwrap().display().to_string();
        for l in lints(&src) {
            let said: String = l.msg.chars().take(60).collect();
            eprintln!("{name}:{}:{} {} {said}", l.pos.line, l.pos.col, l.id);
            if l.blocking {
                bad.push(format!("{name}:{} {}", l.pos.line, l.id));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
