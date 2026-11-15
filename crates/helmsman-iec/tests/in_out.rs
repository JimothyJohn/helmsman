//! `VAR_IN_OUT` on a function block and `VAR_OUTPUT` on a function.
//!
//! An in-out is the caller's own variable, reached by reference inside one
//! module. Everything that would reach beyond the module (the process
//! image, a program's signals) or make two names for one value is refused,
//! with a message in plain words; each refusal is pinned below.
//!
//! ruSTy agrees with the run-time behaviour on the cases in
//! `tests/oracle.rs`. The semantics of reference passing (a name for the
//! caller's variable, writes seen at once) are those the PLCopen and
//! CODESYS references describe for `VAR_IN_OUT`; docs/SOURCES.md lists them.

use std::time::Duration;

use helmsman_iec::{FaultKind, Image, ImageSizes, Program, Value, compile};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

fn build(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{src}\n{d:#?}"))
}

fn scan(p: &mut Program, ms: u64) {
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::from_millis(ms)).unwrap();
}

fn int(p: &Program, path: &str) -> i128 {
    match p.get(path) {
        Some(Value::Int(i)) => i,
        other => panic!("{path} = {other:?}"),
    }
}

fn errors(src: &str) -> String {
    match compile(src, SIZES) {
        Ok(_) => panic!("compiled: {src}"),
        Err(d) => d.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
    }
}

/// A block that adds `step` to every element of the caller's array and
/// counts its calls in its own state.
const BUMP: &str = "
FUNCTION_BLOCK bump
  VAR_INPUT step : INT; END_VAR
  VAR_IN_OUT buf : ARRAY[0..3] OF INT; END_VAR
  VAR_OUTPUT first : INT; END_VAR
  VAR i : INT; calls : INT; END_VAR
  FOR i := 0 TO 3 DO buf[i] := buf[i] + step; END_FOR;
  calls := calls + 1;
  first := buf[0];
END_FUNCTION_BLOCK
";

fn with(decls: &str, vars: &str, body: &str) -> String {
    format!("{decls}\nPROGRAM t\nVAR {vars} END_VAR\n{body}\nEND_PROGRAM\n")
}

// ---- VAR_IN_OUT: what runs ----

#[test]
fn writes_inside_the_block_reach_the_callers_array_at_once() {
    let mut p = build(&with(
        BUMP,
        "a : ARRAY[0..3] OF INT := [1, 2, 3, 4]; b : bump; seen, after : INT;",
        "b(step := 10, buf := a); seen := a[0]; after := b.first;",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "a[0]"), 11);
    assert_eq!(int(&p, "a[3]"), 14);
    assert_eq!(int(&p, "seen"), 11, "visible straight after the call");
    assert_eq!(int(&p, "after"), 11);
    scan(&mut p, 10);
    assert_eq!(int(&p, "a[3]"), 24, "the same variable, scan after scan");
}

#[test]
fn one_block_type_works_on_different_variables_per_instance() {
    let mut p = build(&with(
        BUMP,
        "x, y : ARRAY[0..3] OF INT; p, q : bump;",
        "p(step := 1, buf := x); q(step := 5, buf := y); p(step := 1, buf := x);",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "x[2]"), 2);
    assert_eq!(int(&p, "y[2]"), 5);
}

#[test]
fn an_instance_can_be_pointed_at_another_variable_on_the_next_call() {
    let mut p = build(&with(
        BUMP,
        "x, y : ARRAY[0..3] OF INT; p : bump; flip : BOOL;",
        "IF flip THEN p(step := 1, buf := y); ELSE p(step := 1, buf := x); END_IF; flip := NOT flip;",
    ));
    for k in 0..4 {
        scan(&mut p, k * 10);
    }
    assert_eq!(int(&p, "x[0]"), 2);
    assert_eq!(int(&p, "y[0]"), 2);
}

#[test]
fn structs_scalars_and_enums_go_by_reference_too() {
    let mut p = build(
        "TYPE pt : STRUCT x : INT; y : INT; END_STRUCT; END_TYPE
         TYPE color : (red, green, blue); END_TYPE
         FUNCTION_BLOCK touch
           VAR_IN_OUT p : pt; n : DINT; c : color; END_VAR
           p.x := p.y + 1; p.y := 7; n := n * 2; c := blue;
         END_FUNCTION_BLOCK
         PROGRAM t
           VAR v : pt; k : DINT := 21; col : color; b : touch; END_VAR
           b(p := v, n := k, c := col);
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "v.x"), 1);
    assert_eq!(int(&p, "v.y"), 7);
    assert_eq!(int(&p, "k"), 42);
    assert_eq!(int(&p, "col"), 2);
}

#[test]
fn an_element_or_a_member_can_be_the_argument() {
    let mut p = build(
        "TYPE rec : STRUCT id : INT; data : ARRAY[0..3] OF INT; END_STRUCT; END_TYPE
         FUNCTION_BLOCK inc
           VAR_IN_OUT n : INT; END_VAR
           n := n + 1;
         END_FUNCTION_BLOCK
         FUNCTION_BLOCK fill
           VAR_IN_OUT d : ARRAY[0..3] OF INT; END_VAR
           d[0] := 99;
         END_FUNCTION_BLOCK
         PROGRAM t
           VAR r : rec; rows : ARRAY[0..2] OF rec; i : INT := 2; a : inc; f : fill; END_VAR
           a(n := r.id);
           a(n := r.data[3]);
           a(n := rows[i].id);
           a(n := rows[i].data[i]);
           f(d := rows[1].data);
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "r.id"), 1);
    assert_eq!(int(&p, "r.data[3]"), 1);
    assert_eq!(int(&p, "rows[2].id"), 1);
    assert_eq!(int(&p, "rows[2].data[2]"), 1);
    assert_eq!(int(&p, "rows[1].data[0]"), 99);
    assert_eq!(int(&p, "rows[0].data[0]"), 0, "neighbours stay as they were");
}

#[test]
fn the_element_is_fixed_when_the_call_starts() {
    let mut p = build(
        "FUNCTION_BLOCK inc
           VAR_IN_OUT n : INT; END_VAR
           n := n + 1;
         END_FUNCTION_BLOCK
         PROGRAM t
           VAR a : ARRAY[0..2] OF INT; i : INT; w : inc; END_VAR
           FOR i := 0 TO 2 DO w(n := a[i]); END_FOR;
         END_PROGRAM",
    );
    scan(&mut p, 0);
    scan(&mut p, 10);
    for k in 0..3 {
        assert_eq!(int(&p, &format!("a[{k}]")), 2);
    }
}

#[test]
fn an_index_out_of_range_in_the_argument_faults_the_scan() {
    let mut p = build(
        "FUNCTION_BLOCK inc VAR_IN_OUT n : INT; END_VAR n := n + 1; END_FUNCTION_BLOCK
         PROGRAM t VAR a : ARRAY[0..2] OF INT; i : INT := 3; w : inc; END_VAR w(n := a[i]); END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    let f = p.scan_at(&mut img, Duration::ZERO).unwrap_err();
    assert_eq!(f.kind, FaultKind::IndexOutOfRange);
}

#[test]
fn an_in_out_can_be_forwarded_to_a_block_inside() {
    let mut p = build(
        "TYPE row : STRUCT cell : ARRAY[0..1] OF INT; END_STRUCT; END_TYPE
         FUNCTION_BLOCK inner
           VAR_IN_OUT cell : INT; END_VAR
           cell := cell + 10;
         END_FUNCTION_BLOCK
         FUNCTION_BLOCK middle
           VAR_IN_OUT pair : ARRAY[0..1] OF INT; END_VAR
           VAR a, b : inner; END_VAR
           a(cell := pair[0]);
           b(cell := pair[1]);
           b(cell := pair[1]);
         END_FUNCTION_BLOCK
         FUNCTION_BLOCK outer
           VAR_IN_OUT grid : ARRAY[0..2] OF row; END_VAR
           VAR m : middle; END_VAR
           m(pair := grid[1].cell);
         END_FUNCTION_BLOCK
         PROGRAM t
           VAR g : ARRAY[0..2] OF row; o : outer; END_VAR
           o(grid := g);
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "g[1].cell[0]"), 10);
    assert_eq!(int(&p, "g[1].cell[1]"), 20);
    assert_eq!(int(&p, "g[0].cell[0]"), 0);
}

#[test]
fn the_in_out_can_be_a_whole_struct_passed_on_as_an_input_copy() {
    let mut p = build(
        "TYPE pt : STRUCT x : INT; y : INT; END_STRUCT; END_TYPE
         FUNCTION_BLOCK sum
           VAR_INPUT q : pt; END_VAR
           VAR_OUTPUT s : INT; END_VAR
           s := q.x + q.y;
         END_FUNCTION_BLOCK
         FUNCTION_BLOCK wrap
           VAR_IN_OUT p : pt; END_VAR
           VAR_OUTPUT s : INT; END_VAR
           VAR inner : sum; copy : pt; END_VAR
           p.x := 3; p.y := 4;
           inner(q := p);
           copy := p;
           copy.x := 100;
           s := inner.s + p.x;
         END_FUNCTION_BLOCK
         PROGRAM t VAR v : pt; w : wrap; r : INT; END_VAR w(p := v); r := w.s; END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "r"), 10);
    assert_eq!(int(&p, "v.x"), 3, "assigning a copy leaves the original alone");
}

#[test]
fn a_block_without_in_out_behaves_as_before_in_a_bank() {
    let mut p = build(
        "FUNCTION_BLOCK acc VAR_IN_OUT t : INT; END_VAR t := t + 1; END_FUNCTION_BLOCK
         PROGRAM p VAR bank : ARRAY[0..2] OF acc; v : ARRAY[0..2] OF INT; i : INT; END_VAR
           FOR i := 0 TO 2 DO bank[i](t := v[i]); END_FOR;
         END_PROGRAM",
    );
    scan(&mut p, 0);
    scan(&mut p, 10);
    assert_eq!(int(&p, "v[2]"), 2);
}

// ---- VAR_OUTPUT on a FUNCTION ----

const DIVMOD: &str = "
FUNCTION divmod : DINT
  VAR_INPUT a, b : DINT; END_VAR
  VAR_OUTPUT rem : DINT; ok : BOOL; END_VAR
  IF b = 0 THEN divmod := 0; rem := a; ok := FALSE; RETURN; END_IF;
  divmod := a / b;
  rem := a MOD b;
  ok := TRUE;
END_FUNCTION
";

#[test]
fn a_function_returns_its_value_and_fills_its_outputs() {
    let mut p = build(&with(
        DIVMOD,
        "q, r, w : DINT; good : BOOL; n : LINT;",
        "q := divmod(17, 5, rem => r, ok => good); w := divmod(a := 9, b := 0, rem => n);",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "q"), 3);
    assert_eq!(int(&p, "r"), 2);
    assert_eq!(p.get("good"), Some(Value::Bool(true)));
    assert_eq!(int(&p, "w"), 0);
    assert_eq!(int(&p, "n"), 9, "DINT widens into the LINT target");
}

#[test]
fn an_output_left_unbound_is_simply_dropped() {
    let mut p = build(&with(
        DIVMOD,
        "q, r : DINT;",
        "q := divmod(17, 5); r := 77; q := q + divmod(7, 2, rem => r);",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "q"), 3 + 3);
    assert_eq!(int(&p, "r"), 1);
}

#[test]
fn a_function_statement_can_bind_outputs_and_drop_the_result() {
    let mut p = build(&with(DIVMOD, "r : DINT;", "divmod(17, 5, rem => r);"));
    scan(&mut p, 0);
    assert_eq!(int(&p, "r"), 2);
}

#[test]
fn outputs_start_at_their_initial_value_on_every_call() {
    let mut p = build(&with(
        "FUNCTION once : INT
           VAR_INPUT go : BOOL; END_VAR
           VAR_OUTPUT mark : INT := 5; END_VAR
           IF go THEN mark := 9; END_IF;
           once := 0;
         END_FUNCTION",
        "a, b, k : INT;",
        "k := once(TRUE, mark => a); k := once(FALSE, mark => b);",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "a"), 9);
    assert_eq!(int(&p, "b"), 5, "nothing is kept between calls");
}

#[test]
fn an_output_can_land_in_an_array_element_or_a_member() {
    let mut p = build(&with(
        "TYPE pair : STRUCT lo : INT; hi : INT; END_STRUCT; END_TYPE
         FUNCTION split : INT
           VAR_INPUT x : INT; END_VAR
           VAR_OUTPUT lo, hi : INT; END_VAR
           lo := x MOD 10; hi := x / 10; split := x;
         END_FUNCTION",
        "p : pair; v : ARRAY[0..2] OF INT; i : INT := 1; n : INT;",
        "n := split(47, lo => p.lo, hi => v[i]);",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "p.lo"), 7);
    assert_eq!(int(&p, "v[1]"), 4);
}

#[test]
fn nested_calls_each_fill_their_own_outputs() {
    let mut p = build(&with(
        DIVMOD,
        "q, r1, r2 : DINT;",
        "q := divmod(divmod(100, 7, rem => r1), 3, rem => r2);",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "q"), 4);
    assert_eq!(int(&p, "r1"), 2);
    assert_eq!(int(&p, "r2"), 2);
}

// ---- refusals ----

fn refused(decls: &str, vars: &str, body: &str, want: &str) {
    let e = errors(&with(decls, vars, body));
    assert!(e.contains(want), "want {want:?} in\n{e}");
}

const INC: &str = "FUNCTION_BLOCK inc VAR_IN_OUT n : INT; END_VAR n := n + 1; END_FUNCTION_BLOCK";

#[test]
fn an_in_out_must_be_given_at_every_call() {
    refused(
        INC,
        "w : inc;",
        "w();",
        "INC: VAR_IN_OUT N needs a variable at every call (N := variable)",
    );
    refused(
        BUMP,
        "w : bump;",
        "w(step := 1);",
        "BUMP: VAR_IN_OUT BUF needs a variable at every call",
    );
}

#[test]
fn an_in_out_takes_a_variable_not_an_expression_or_a_constant() {
    for (arg, want) in [
        (
            "1",
            "N is a VAR_IN_OUT of INC: pass a variable, not an expression or a constant",
        ),
        ("x + 1", "pass a variable, not an expression or a constant"),
        ("TRUE", "pass a variable, not an expression or a constant"),
        ("ABS(x)", "pass a variable, not an expression or a constant"),
        ("c", "the call may write it, but C is a CONSTANT"),
        ("x.3", "a single bit cannot be passed by reference"),
    ] {
        refused(
            INC,
            "w : inc; x : INT; END_VAR VAR CONSTANT c : INT := 5;",
            &format!("w(n := {arg});"),
            want,
        );
    }
}

#[test]
fn an_in_out_takes_exactly_its_own_type() {
    refused(
        INC,
        "w : inc; d : DINT;",
        "w(n := d);",
        "N is a VAR_IN_OUT of INC: it is INT and the argument is DINT; a VAR_IN_OUT takes exactly its own type",
    );
    refused(
        BUMP,
        "w : bump; a : ARRAY[0..4] OF INT;",
        "w(step := 1, buf := a);",
        "a VAR_IN_OUT takes exactly its own type",
    );
    refused(BUMP, "w : bump; a : INT;", "w(step := 1, buf := a);", "it is");
}

#[test]
fn an_in_out_is_not_an_output_and_not_positional() {
    refused(
        INC,
        "w : inc; x : INT;",
        "w(n => x);",
        "N is a VAR_IN_OUT of INC, not an output: pass it with :=",
    );
    refused(INC, "w : inc; x : INT;", "w(x);", "INC takes 0 input(s)");
}

#[test]
fn the_same_variable_cannot_go_to_two_in_outs_of_one_call() {
    let two = "FUNCTION_BLOCK two
      VAR_IN_OUT a, b : INT; END_VAR
      a := 1; b := 2;
    END_FUNCTION_BLOCK";
    refused(
        two,
        "w : two; x : INT;",
        "w(a := x, b := x);",
        "TWO: A and B would be two names for the same variable X; give each VAR_IN_OUT a different variable",
    );
    // Overlap through a member, an unknown index, and a whole against a part.
    let arr = "FUNCTION_BLOCK arr2
      VAR_IN_OUT a, b : INT; END_VAR
      a := 1;
    END_FUNCTION_BLOCK";
    refused(
        arr,
        "w : arr2; v : ARRAY[0..3] OF INT; i, j : INT;",
        "w(a := v[i], b := v[j]);",
        "two names for the same variable V",
    );
    refused(
        arr,
        "w : arr2; v : ARRAY[0..3] OF INT; i : INT;",
        "w(a := v[1], b := v[i]);",
        "two names for the same variable V",
    );
    refused(
        arr,
        "w : arr2; v : ARRAY[0..3] OF INT;",
        "w(a := v[2], b := v[2]);",
        "two names for the same variable V",
    );
    let whole = "TYPE s : STRUCT x : INT; END_STRUCT; END_TYPE
      FUNCTION_BLOCK wp VAR_IN_OUT a : s; b : INT; END_VAR a.x := 1; END_FUNCTION_BLOCK";
    refused(
        whole,
        "w : wp; v : s;",
        "w(a := v, b := v.x);",
        "two names for the same variable V",
    );
}

#[test]
fn different_parts_of_one_variable_are_fine() {
    let arr = "FUNCTION_BLOCK arr2
      VAR_IN_OUT a, b : INT; END_VAR
      a := 1; b := 2;
    END_FUNCTION_BLOCK
    TYPE s : STRUCT x : INT; y : INT; END_STRUCT; END_TYPE";
    let mut p = build(&with(
        arr,
        "w : arr2; v : ARRAY[0..3] OF INT; r : s; m : ARRAY[0..1] OF s;",
        "w(a := v[0], b := v[1]); w(a := r.x, b := r.y); w(a := m[0].x, b := m[1].x); w(a := m[0].x, b := m[0].y);",
    ));
    scan(&mut p, 0);
    assert_eq!(int(&p, "v[1]"), 2);
    assert_eq!(int(&p, "r.y"), 2);
    assert_eq!(int(&p, "m[1].x"), 2);
}

#[test]
fn an_instance_cannot_be_given_a_part_of_itself() {
    refused(
        "FUNCTION_BLOCK holder
           VAR_INPUT box : ARRAY[0..1] OF INT; END_VAR
           VAR_IN_OUT n : ARRAY[0..1] OF INT; END_VAR
           n[0] := 1;
         END_FUNCTION_BLOCK",
        "h : holder;",
        "h(n := h.box);",
        "HOLDER: N is part of the instance being called; give it a variable of its own",
    );
}

#[test]
fn what_other_modules_share_is_never_passed_by_reference() {
    // A located variable and a direct address live in the process image.
    refused(
        INC,
        "w : inc; lamp AT %MW0 : INT;",
        "w(n := lamp);",
        "N is a VAR_IN_OUT of INC: LAMP is located in the process image, which other modules share; copy it into a variable first",
    );
    refused(
        INC,
        "w : inc;",
        "w(n := %MW2);",
        "%MW2 is in the process image, which other modules share; copy it into a variable first",
    );
    // A program's own inputs are signals another module writes.
    let e = errors(&format!(
        "{INC}\nPROGRAM t VAR_INPUT sig : INT; END_VAR VAR w : inc; END_VAR w(n := sig); END_PROGRAM"
    ));
    assert!(
        e.contains(
            "SIG is an input of the program, a signal another module writes; copy it into a variable first"
        ),
        "{e}"
    );
}

#[test]
fn a_program_cannot_declare_an_in_out() {
    let e = errors("PROGRAM p VAR_IN_OUT a : INT; END_VAR END_PROGRAM");
    assert!(
        e.contains(
            "VAR_IN_OUT is not supported in a PROGRAM: a program's inputs and outputs are signals that other modules share"
        ),
        "{e}"
    );
}

#[test]
fn a_function_cannot_declare_an_in_out_yet() {
    let e = errors("FUNCTION f : INT VAR_IN_OUT a : INT; END_VAR f := a; END_FUNCTION PROGRAM p END_PROGRAM");
    assert!(
        e.contains("VAR_IN_OUT is not supported in a FUNCTION yet: use a FUNCTION_BLOCK to take a struct or array by reference"),
        "{e}"
    );
}

#[test]
fn in_out_declarations_have_no_initial_value_and_no_qualifiers() {
    for (decl, want) in [
        (
            "n : INT := 3;",
            "N: a VAR_IN_OUT takes no initial value: it is the caller's variable",
        ),
        (
            "n AT %MW0 : INT;",
            "N: a VAR_IN_OUT cannot be located: it is the caller's variable",
        ),
        ("n : TON;", "N: a VAR_IN_OUT cannot be a function block instance"),
    ] {
        let e = errors(&format!(
            "FUNCTION_BLOCK f VAR_IN_OUT {decl} END_VAR END_FUNCTION_BLOCK PROGRAM p END_PROGRAM"
        ));
        assert!(e.contains(want), "{decl}: {e}");
    }
    let e = errors(
        "FUNCTION_BLOCK f VAR_IN_OUT RETAIN n : INT; END_VAR END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
    );
    assert!(!e.is_empty());
    let e = errors(
        "FUNCTION_BLOCK f VAR_IN_OUT n : INT; n : INT; END_VAR END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
    );
    assert!(e.contains("N is declared twice"), "{e}");
}

#[test]
fn an_in_out_is_private_to_its_block() {
    refused(
        INC,
        "w : inc; x : INT;",
        "w(n := x); x := w.n;",
        "N is internal to INC; only inputs and outputs are visible",
    );
}

#[test]
fn an_output_of_a_function_has_its_own_rules() {
    refused(
        DIVMOD,
        "q : DINT;",
        "q := divmod(1, 1, divmod => q);",
        "DIVMOD is the result of the function: read it from the call, not with =>",
    );
    refused(
        DIVMOD,
        "q : DINT; b : BOOL;",
        "q := divmod(1, 1, rem => b);",
        "output REM is DINT, the target is BOOL",
    );
    refused(
        DIVMOD,
        "q : DINT;",
        "q := divmod(1, 1, rem => 5);",
        "expected a variable",
    );
    refused(
        DIVMOD,
        "q : DINT;",
        "q := divmod(a => q);",
        "A is not an output of DIVMOD",
    );
    refused(
        DIVMOD,
        "q : DINT;",
        "q := divmod(1, 1, nope => q);",
        "DIVMOD has no input or output NOPE",
    );
    refused(
        DIVMOD,
        "q : DINT;",
        "q := divmod(1, 1, rem => q, rem => q);",
        "REM is given twice",
    );
    refused(
        DIVMOD,
        "q : DINT;",
        "q := divmod(rem := 1);",
        "REM is not an input of DIVMOD; outputs are bound with =>",
    );
}

#[test]
fn an_output_cannot_be_bound_to_what_cannot_be_written() {
    refused(
        DIVMOD,
        "q : DINT; END_VAR VAR CONSTANT c : DINT := 1;",
        "q := divmod(1, 1, rem => c);",
        "the target is a CONSTANT",
    );
}

#[test]
fn a_function_output_cannot_land_in_the_process_input_image() {
    refused(
        DIVMOD,
        "q : DINT; inp AT %ID0 : DINT;",
        "q := divmod(1, 1, rem => inp);",
        "the target cannot be written: inputs (%I) are read-only",
    );
}

// ---- properties ----

proptest! {
    /// Against a Rust model: a bank of blocks, each pointed at one of three
    /// arrays by a random plan, adds its amount to every element it is
    /// given. Whatever the plan, every element ends as the model says, and
    /// a second program run from the same plan agrees (no hidden state).
    #[test]
    fn blocks_on_shared_arrays_match_a_model(
        plan in proptest::collection::vec((0usize..3, 0usize..2, -50i16..50), 1..12),
        scans in 1u64..4,
    ) {
        let mut body = String::new();
        let mut want = [[0i32; 3]; 3];
        for (arr, inst, by) in &plan {
            body.push_str(&format!("blk[{inst}](step := {by}, buf := a{arr});\n"));
        }
        for _ in 0..scans {
            for (arr, _, by) in &plan {
                for e in &mut want[*arr] {
                    *e = i32::from(i16::try_from(*e + i32::from(*by)).unwrap_or(0));
                }
            }
        }
        let src = format!(
            "FUNCTION_BLOCK add
               VAR_INPUT step : INT; END_VAR
               VAR_IN_OUT buf : ARRAY[0..2] OF INT; END_VAR
               VAR i : INT; END_VAR
               FOR i := 0 TO 2 DO buf[i] := buf[i] + step; END_FOR;
             END_FUNCTION_BLOCK
             PROGRAM t
               VAR a0, a1, a2 : ARRAY[0..2] OF INT; blk : ARRAY[0..1] OF add; END_VAR
               {body}
             END_PROGRAM"
        );
        let mut p = compile(&src, SIZES).map_err(|d| TestCaseError::fail(format!("{d:?}\n{src}")))?;
        let mut img = Image::new(SIZES);
        for k in 0..scans {
            p.scan_at(&mut img, Duration::from_millis(k * 10)).map_err(|f| TestCaseError::fail(f.to_string()))?;
        }
        for (a, row) in want.iter().enumerate() {
            for (e, v) in row.iter().enumerate() {
                prop_assert_eq!(int(&p, &format!("a{a}[{e}]")), i128::from(*v), "a{}[{}]\n{}", a, e, src);
            }
        }
    }

    /// A function with outputs agrees with its definition for any inputs,
    /// and the outputs land exactly where they are bound.
    #[test]
    fn function_outputs_match_their_definition(a in -1000i32..1000, b in -20i32..20) {
        let src = with(
            DIVMOD,
            "q, r : DINT; ok : BOOL; a, b : DINT;",
            &format!("a := {a}; b := {b}; q := divmod(a, b, rem => r, ok => ok);"),
        );
        let mut p = compile(&src, SIZES).map_err(|d| TestCaseError::fail(format!("{d:?}")))?;
        let mut img = Image::new(SIZES);
        p.scan_at(&mut img, Duration::ZERO).map_err(|f| TestCaseError::fail(f.to_string()))?;
        if b == 0 {
            prop_assert_eq!(int(&p, "q"), 0);
            prop_assert_eq!(int(&p, "r"), i128::from(a));
            prop_assert_eq!(p.get("ok"), Some(Value::Bool(false)));
        } else {
            prop_assert_eq!(int(&p, "q"), i128::from(a / b));
            prop_assert_eq!(int(&p, "r"), i128::from(a % b));
            prop_assert_eq!(p.get("ok"), Some(Value::Bool(true)));
        }
    }

    /// Token soup around the new syntax never panics the compiler, and
    /// whatever compiles scans without panicking.
    #[test]
    fn in_out_token_soup_never_panics(words in proptest::collection::vec(prop_oneof![
        Just("VAR_IN_OUT"), Just("VAR_OUTPUT"), Just("VAR_INPUT"), Just("END_VAR"), Just("x"), Just("a"),
        Just("b"), Just("w"), Just(":="), Just("=>"), Just(";"), Just("("), Just(")"), Just("["), Just("]"),
        Just("."), Just(","), Just(":"), Just("INT"), Just("0"), Just("1"), Just("a[x]"), Just("w.n"),
        Just("w(n := a)"), Just("w(n := a[1], m => x)"), Just("f(1, o => x)"), Just("n := n + 1;"),
    ], 0..40)) {
        let body = words.join(" ");
        for src in [
            format!(
                "FUNCTION_BLOCK blk VAR_IN_OUT n, m : INT; END_VAR VAR_OUTPUT o : INT; END_VAR {body} END_FUNCTION_BLOCK
                 PROGRAM p VAR x : INT; a : ARRAY[0..3] OF INT; w : blk; END_VAR w(n := x, m := a[1]); END_PROGRAM"
            ),
            format!(
                "FUNCTION f : INT VAR_INPUT i : INT; END_VAR VAR_OUTPUT o : INT; END_VAR {body} END_FUNCTION
                 PROGRAM p VAR x : INT; a : ARRAY[0..3] OF INT; END_VAR x := f(1, o => x); END_PROGRAM"
            ),
            format!("PROGRAM p VAR x : INT; a : ARRAY[0..3] OF INT; w : blk; END_VAR {body} END_PROGRAM"),
        ] {
            if let Ok(mut p) = compile(&src, SIZES) {
                p.set_budget(10_000);
                let mut img = Image::new(SIZES);
                let _ = p.scan_at(&mut img, Duration::from_millis(1));
            }
        }
    }
}
