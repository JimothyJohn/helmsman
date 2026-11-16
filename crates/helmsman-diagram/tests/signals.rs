//! The signals an output finally depends on, from the diagram alone.

mod common;

use std::fmt::Write as _;

use common::{build, prog};
use helmsman_diagram::signals;

fn of(vars: &str, body: &str, out: &str) -> Vec<String> {
    let (_, m) = build(&prog(vars, body));
    signals(&m, 0, out)
}

#[test]
fn a_straight_assignment_depends_on_its_source() {
    assert_eq!(of("a AT %IX0.0 : BOOL; y : BOOL;", "y := a;", "y"), ["A"]);
}

#[test]
fn an_and_or_chain_names_each_signal_once_in_source_order() {
    let got = of(
        "a, b, c : BOOL; y : BOOL;",
        "y := (a AND b) OR (c AND NOT a);",
        "y",
    );
    assert_eq!(got, ["A", "B", "C"]);
}

#[test]
fn constants_and_literals_are_not_signals() {
    let got = of("a : BOOL; n : INT; y : BOOL;", "y := a AND (n > 5);", "y");
    assert_eq!(got, ["A", "N"]);
    let (_, m) = build(
        "PROGRAM t\nVAR CONSTANT k : INT := 3; END_VAR\nVAR n : INT; y : BOOL; END_VAR\ny := n > k;\nEND_PROGRAM\n",
    );
    assert_eq!(signals(&m, 0, "y"), ["N"]);
}

#[test]
fn an_intermediate_variable_is_followed_to_its_inputs() {
    let got = of(
        "a, b, c : BOOL; mid, y : BOOL;",
        "mid := a AND b;\ny := mid OR c;",
        "y",
    );
    assert_eq!(got, ["A", "B", "C"]);
}

#[test]
fn an_intermediate_set_in_a_later_statement_still_counts() {
    let got = of("a, b : BOOL; mid, y : BOOL;", "y := mid;\nmid := a AND b;", "y");
    assert_eq!(got, ["A", "B"]);
}

#[test]
fn if_arms_bring_their_conditions_and_the_earlier_arms_conditions() {
    let got = of(
        "a, b, c, x, z : BOOL; y : BOOL;",
        "IF a THEN y := x;\nELSIF b THEN y := z;\nELSE y := c;\nEND_IF;",
        "y",
    );
    for s in ["A", "B", "X", "Z"] {
        assert!(got.contains(&s.to_string()), "{s} in {got:?}");
    }
    let mut once = got.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), got.len(), "each once: {got:?}");
}

#[test]
fn a_case_selector_is_a_signal() {
    let got = of(
        "sel : INT; a : BOOL; y : BOOL;",
        "CASE sel OF 1: y := a;\n2: y := FALSE;\nEND_CASE;",
        "y",
    );
    assert!(
        got.contains(&"SEL".to_string()) && got.contains(&"A".to_string()),
        "{got:?}"
    );
}

#[test]
fn a_latch_does_not_list_itself() {
    let got = of(
        "start, stop : BOOL; run : BOOL;",
        "run := (start OR run) AND NOT stop;",
        "run",
    );
    assert_eq!(got, ["START", "STOP"]);
}

#[test]
fn two_outputs_that_read_each_other_end() {
    let got = of("a : BOOL; p, q : BOOL;", "p := a OR q;\nq := p;", "p");
    assert_eq!(got, ["A"]);
}

#[test]
fn a_function_block_output_brings_the_blocks_arguments() {
    let got = of(
        "go, hold : BOOL; t : TON; y : BOOL;",
        "t(IN := go AND hold, PT := T#1s);\ny := t.Q;",
        "y",
    );
    assert_eq!(got, ["GO", "HOLD"]);
}

#[test]
fn an_output_nothing_writes_has_no_signals() {
    assert_eq!(of("a : BOOL; y : BOOL;", "a := a;", "y").len(), 0);
    assert_eq!(of("a : BOOL; y : BOOL;", "a := a;", "NOPE").len(), 0);
}

#[test]
fn a_write_inside_a_loop_does_not_list_the_loop_variable() {
    let got = of(
        "a : BOOL; y : BOOL; i : INT;",
        "FOR i := 0 TO 3 DO\n  IF a THEN y := TRUE; END_IF;\nEND_FOR;",
        "y",
    );
    assert_eq!(got, ["A"]);
}

#[test]
fn a_bad_pou_number_is_empty() {
    let (_, m) = build(&prog("a, y : BOOL;", "y := a;"));
    assert_eq!(signals(&m, 9, "y").len(), 0);
}

#[test]
fn hostile_deep_nesting_ends() {
    let mut body = String::new();
    for k in 0..40 {
        writeln!(body, "IF a{k} THEN").unwrap();
    }
    body.push_str("y := TRUE;\n");
    for _ in 0..40 {
        body.push_str("END_IF;\n");
    }
    let mut vars = String::new();
    for k in 0..40 {
        write!(vars, "a{k} : BOOL; ").unwrap();
    }
    vars.push_str("y : BOOL;");
    let got = of(&vars, &body, "y");
    assert_eq!(got.len(), 40);
}

#[test]
fn two_hundred_outputs_each_get_their_own_answer() {
    let mut vars = String::from("a, b : BOOL; ");
    let mut body = String::new();
    for k in 0..200 {
        write!(vars, "y{k} : BOOL; ").unwrap();
        writeln!(body, "y{k} := {};", if k % 2 == 0 { "a" } else { "b" }).unwrap();
    }
    let (_, m) = build(&prog(&vars, &body));
    for k in 0..200 {
        let want = if k % 2 == 0 { "A" } else { "B" };
        assert_eq!(signals(&m, 0, &format!("y{k}")), [want]);
    }
}
