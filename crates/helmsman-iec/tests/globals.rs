//! `VAR_GLOBAL` inside one module.
//!
//! A module's globals are declared before its first POU or in its
//! `CONFIGURATION` / `RESOURCE`, and every POU of the module sees them
//! (`VAR_EXTERNAL` names one, optionally read-only). A global is never a
//! port and nothing outside the module can name it: every way a second
//! module could reach one is refused here, with the message that says
//! signals are how modules talk.
//!
//! Which forms ruSTy also accepts is pinned by the cases in
//! `tests/oracle.rs`; everything else in this file (the CONFIGURATION
//! syntax, `VAR_EXTERNAL CONSTANT` as a read-only view, CONSTANT and located
//! globals, the refusals) has no oracle, since ruSTy takes none of it or
//! takes it differently (it lets a `VAR_EXTERNAL CONSTANT` be written).

use std::time::Duration;

use helmsman_iec::{FaultKind, Image, ImageSizes, Program, SetError, Value, compile};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

fn build(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{src}\n{d:#?}"))
}

fn scan(p: &mut Program, img: &mut Image, ms: u64) {
    p.scan_at(img, Duration::from_millis(ms)).unwrap();
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

fn refused(src: &str, want: &str) {
    let e = errors(src);
    assert!(e.contains(want), "wanted {want:?} in:\n{e}\nfor\n{src}");
}

const SIGNALS: &str = "signals are how modules talk";

// ---- what runs ----

#[test]
fn every_pou_of_the_module_sees_a_global() {
    let mut p = build(
        "
VAR_GLOBAL
  total : DINT := 5;
END_VAR
FUNCTION_BLOCK acc
  VAR_INPUT add : DINT; END_VAR
  VAR_EXTERNAL total : DINT; END_VAR
  total := total + add;
END_FUNCTION_BLOCK
FUNCTION peek : DINT
  peek := total * 2;
END_FUNCTION
PROGRAM p
  VAR a : acc; seen, twice : DINT; END_VAR
  a(add := 3);
  seen := total;
  twice := peek();
END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    scan(&mut p, &mut img, 0);
    assert_eq!(int(&p, "total"), 8);
    assert_eq!(int(&p, "seen"), 8);
    assert_eq!(int(&p, "twice"), 16);
    scan(&mut p, &mut img, 10);
    assert_eq!(int(&p, "total"), 11, "the global keeps its value between scans");
    p.reset();
    assert_eq!(int(&p, "total"), 5, "a cold restart restores the initial value");
}

#[test]
fn a_block_may_use_a_global_without_declaring_it_external() {
    let mut p = build(
        "VAR_GLOBAL n : INT; END_VAR
         FUNCTION_BLOCK b n := n + 1; END_FUNCTION_BLOCK
         PROGRAM p VAR x : b; END_VAR x(); x(); END_PROGRAM",
    );
    scan(&mut p, &mut Image::new(SIZES), 0);
    assert_eq!(int(&p, "n"), 2);
}

#[test]
fn two_instances_of_a_block_share_the_one_global() {
    let mut p = build(
        "VAR_GLOBAL n : INT; END_VAR
         FUNCTION_BLOCK b VAR_OUTPUT seen : INT; END_VAR n := n + 1; seen := n; END_FUNCTION_BLOCK
         PROGRAM p VAR x, y : b; a, c : INT; END_VAR x(); y(); a := x.seen; c := y.seen; END_PROGRAM",
    );
    scan(&mut p, &mut Image::new(SIZES), 0);
    assert_eq!((int(&p, "a"), int(&p, "c"), int(&p, "n")), (1, 2, 2));
}

#[test]
fn structs_arrays_and_block_instances_can_be_global() {
    let mut p = build(
        "TYPE pt : STRUCT x : INT := 3; y : INT; END_STRUCT; END_TYPE
         VAR_GLOBAL
           pos : pt;
           tab : ARRAY[1..3] OF INT := [10, 20, 30];
           delay : TON;
         END_VAR
         FUNCTION_BLOCK mover
           VAR_EXTERNAL pos : pt; tab : ARRAY[1..3] OF INT; END_VAR
           VAR i : INT; END_VAR
           pos.y := pos.x + 1;
           FOR i := 1 TO 3 DO tab[i] := tab[i] + 1; END_FOR;
         END_FUNCTION_BLOCK
         PROGRAM p
           VAR m : mover; px, py, t3 : INT; END_VAR
           m();
           delay(IN := TRUE, PT := T#50ms);
           px := pos.x; py := pos.y; t3 := tab[3];
         END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    scan(&mut p, &mut img, 0);
    assert_eq!((int(&p, "px"), int(&p, "py"), int(&p, "t3")), (3, 4, 31));
    assert_eq!(p.get("delay.Q"), Some(Value::Bool(false)));
    scan(&mut p, &mut img, 60);
    assert_eq!(p.get("delay.Q"), Some(Value::Bool(true)));
}

#[test]
fn a_global_block_instance_can_be_called_from_another_block() {
    let mut p = build(
        "VAR_GLOBAL c : CTU; END_VAR
         FUNCTION_BLOCK clicker c(CU := TRUE, R := FALSE, PV := 100); END_FUNCTION_BLOCK
         PROGRAM p VAR k : clicker; END_VAR k(); END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    scan(&mut p, &mut img, 0);
    // CU holds TRUE: only the edge counts.
    scan(&mut p, &mut img, 10);
    assert_eq!(int(&p, "c.CV"), 1);
}

#[test]
fn a_constant_global_folds_into_initial_values_labels_and_expressions() {
    let mut p = build(
        "VAR_GLOBAL CONSTANT
           IDLE : INT := 0;
           RUN : INT := 2;
           LIMIT : DINT := 40;
         END_VAR
         FUNCTION classify : INT
           VAR_INPUT s : INT; END_VAR
           CASE s OF
             IDLE: classify := 100;
             RUN: classify := 200;
           ELSE classify := -1;
           END_CASE;
         END_FUNCTION
         FUNCTION_BLOCK blk
           VAR_EXTERNAL CONSTANT LIMIT : DINT; END_VAR
           VAR cap : DINT := LIMIT; END_VAR
           VAR_OUTPUT room : DINT; END_VAR
           room := cap + LIMIT;
         END_FUNCTION_BLOCK
         PROGRAM p
           VAR b : blk; a, c, d, room : INT; e : DINT := LIMIT; END_VAR
           b(); room := DINT_TO_INT(b.room);
           a := classify(IDLE); c := classify(RUN); d := classify(7);
         END_PROGRAM",
    );
    scan(&mut p, &mut Image::new(SIZES), 0);
    assert_eq!((int(&p, "a"), int(&p, "c"), int(&p, "d")), (100, 200, -1));
    assert_eq!((int(&p, "room"), int(&p, "e")), (80, 40));
}

#[test]
fn a_global_can_be_a_for_variable() {
    let mut p = build(
        "VAR_GLOBAL i : INT; END_VAR
         FUNCTION_BLOCK sum VAR_OUTPUT s : INT; END_VAR FOR i := 1 TO 4 DO s := s + i; END_FOR; END_FUNCTION_BLOCK
         PROGRAM p VAR b : sum; r : INT; END_VAR b(); r := b.s; END_PROGRAM",
    );
    scan(&mut p, &mut Image::new(SIZES), 0);
    assert_eq!((int(&p, "r"), int(&p, "i")), (10, 5));
}

#[test]
fn a_located_global_lives_in_the_image_like_a_located_variable() {
    let mut p = build(
        "VAR_GLOBAL lamp AT %QX0.3 : BOOL := TRUE; sw AT %IX0.0 : BOOL; END_VAR
         FUNCTION_BLOCK follow VAR_EXTERNAL sw : BOOL; lamp : BOOL; END_VAR lamp := lamp AND sw; END_FUNCTION_BLOCK
         PROGRAM p VAR f : follow; END_VAR f(); END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    scan(&mut p, &mut img, 0);
    assert_eq!(img.q[0] & 0b1000, 0, "sw is off: the block cleared the lamp");
    img.i[0] = 1;
    scan(&mut p, &mut img, 10);
    assert_eq!(img.q[0] & 0b1000, 0, "the lamp stays off: nothing sets it");
    // The same rule as a located variable: no slot, so no `get`.
    assert_eq!(p.get("lamp"), None);
}

#[test]
fn a_retain_global_is_kept_like_a_retain_variable() {
    let src = "VAR_GLOBAL RETAIN hits : DINT; END_VAR VAR_GLOBAL fresh : DINT; END_VAR
               FUNCTION_BLOCK b hits := hits + 1; fresh := fresh + 1; END_FUNCTION_BLOCK
               PROGRAM p VAR x : b; END_VAR x(); END_PROGRAM";
    let mut p = build(src);
    assert!(p.has_retain());
    let mut img = Image::new(SIZES);
    scan(&mut p, &mut img, 0);
    scan(&mut p, &mut img, 10);
    let saved = p.retain_state();
    let mut q = build(src);
    q.restore_retain(&saved);
    assert_eq!((int(&q, "hits"), int(&q, "fresh")), (2, 0));
}

#[test]
fn a_global_is_a_program_variable_for_the_host_but_never_a_port() {
    let mut p = build(
        "VAR_GLOBAL g : INT := 4; END_VAR
         PROGRAM p VAR_INPUT go : BOOL; END_VAR VAR_OUTPUT done : BOOL; END_VAR done := go AND g = 4; END_PROGRAM",
    );
    let names: Vec<String> = p.ports().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["GO", "DONE"], "a global is not a signal");
    assert_eq!(p.set_input("g", Value::Int(1)), Err(SetError::NoSuchInput));
    assert_eq!(p.set_input("G", Value::Int(1)), Err(SetError::NoSuchInput));
}

#[test]
fn two_modules_do_not_share_their_globals() {
    let a = "VAR_GLOBAL n : INT; END_VAR PROGRAM a n := n + 1; END_PROGRAM";
    let b = "VAR_GLOBAL n : INT; END_VAR PROGRAM b n := n + 10; END_PROGRAM";
    let (mut pa, mut pb) = (build(a), build(b));
    let mut img = Image::new(SIZES);
    scan(&mut pa, &mut img, 0);
    scan(&mut pb, &mut img, 0);
    scan(&mut pb, &mut img, 10);
    assert_eq!((int(&pa, "n"), int(&pb, "n")), (1, 20));
}

#[test]
fn a_fault_inside_a_block_leaves_the_global_as_it_found_it() {
    let mut p = build(
        "VAR_GLOBAL d, n : INT; END_VAR
         FUNCTION_BLOCK b n := 7; n := 1 / d; END_FUNCTION_BLOCK
         PROGRAM p VAR x : b; END_VAR x(); END_PROGRAM",
    );
    let f = p.scan_at(&mut Image::new(SIZES), Duration::ZERO).unwrap_err();
    assert_eq!(f.kind, FaultKind::DivisionByZero);
    assert_eq!(int(&p, "n"), 7);
}

// ---- where a global is declared ----

const CONFIG: &str = "
CONFIGURATION plant
  VAR_GLOBAL cfg_count : INT := 2; END_VAR
  RESOURCE cpu ON PLC
    VAR_GLOBAL RETAIN res_total : DINT; END_VAR
    VAR_GLOBAL CONSTANT res_step : DINT := 5; END_VAR
  END_RESOURCE
END_CONFIGURATION
PROGRAM p VAR_EXTERNAL res_total : DINT; END_VAR res_total := res_total + res_step; cfg_count := cfg_count + 1; END_PROGRAM";

#[test]
fn globals_in_a_configuration_and_its_resource_are_one_namespace() {
    let mut p = build(CONFIG);
    let mut img = Image::new(SIZES);
    scan(&mut p, &mut img, 0);
    scan(&mut p, &mut img, 10);
    assert_eq!((int(&p, "res_total"), int(&p, "cfg_count")), (10, 4));
    assert!(p.has_retain());
}

#[test]
fn a_bare_top_level_block_and_a_configuration_can_both_declare_globals() {
    let src = format!("VAR_GLOBAL a : INT := 1; END_VAR\n{CONFIG}\nVAR_GLOBAL b : INT := 2; END_VAR");
    let p = build(&src);
    assert_eq!((int(&p, "a"), int(&p, "b"), int(&p, "cfg_count")), (1, 2, 2));
}

#[test]
fn the_rest_of_a_configuration_is_refused_in_plain_words() {
    let task = "TASK t(INTERVAL := T#20ms, PRIORITY := 0);";
    refused(
        &format!(
            "CONFIGURATION c RESOURCE r ON PLC {task} END_RESOURCE END_CONFIGURATION PROGRAM p END_PROGRAM"
        ),
        "TASK is not supported: a module's scan rate is set where the module is deployed, not in its source",
    );
    refused(
        "CONFIGURATION c RESOURCE r ON PLC PROGRAM i : p; END_RESOURCE END_CONFIGURATION PROGRAM p END_PROGRAM",
        "a PROGRAM instance in a RESOURCE is not supported: a module runs its one PROGRAM",
    );
    refused(
        "CONFIGURATION c VAR_ACCESS x : p.a : INT READ_ONLY; END_VAR END_CONFIGURATION PROGRAM p END_PROGRAM",
        "VAR_ACCESS is not supported: modules do not share variables, signals are how modules talk",
    );
    refused(
        "CONFIGURATION c VAR_CONFIG p.a AT %IX0.0 : BOOL; END_VAR END_CONFIGURATION PROGRAM p END_PROGRAM",
        "VAR_CONFIG is not supported",
    );
    refused(
        "CONFIGURATION c RESOURCE r ON PLC x := 1; END_RESOURCE END_CONFIGURATION PROGRAM p END_PROGRAM",
        "only global variables are read from a RESOURCE",
    );
    refused(
        "CONFIGURATION c VAR_GLOBAL a : INT; END_VAR",
        "this CONFIGURATION is never closed",
    );
}

#[test]
fn a_module_has_one_configuration_and_one_resource() {
    refused(
        "CONFIGURATION a END_CONFIGURATION CONFIGURATION b END_CONFIGURATION PROGRAM p END_PROGRAM",
        "a module has one CONFIGURATION",
    );
    refused(
        "CONFIGURATION a RESOURCE r ON x END_RESOURCE RESOURCE s ON x END_RESOURCE END_CONFIGURATION PROGRAM p END_PROGRAM",
        "a module has one RESOURCE",
    );
}

#[test]
fn var_global_inside_a_pou_is_refused_and_says_where_it_goes() {
    for pou in [
        "PROGRAM p VAR_GLOBAL a : INT; END_VAR END_PROGRAM",
        "FUNCTION_BLOCK b VAR_GLOBAL a : INT; END_VAR END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
    ] {
        refused(
            pou,
            "VAR_GLOBAL goes before the first PROGRAM, or in a CONFIGURATION, not inside a POU",
        );
    }
}

#[test]
fn names_that_were_variables_stay_variables() {
    // Not reserved: only a unit that starts with CONFIGURATION is one.
    let mut p = build(
        "PROGRAM p VAR task, resource, configuration, on : INT; END_VAR
         task := 1; resource := 2; configuration := task + resource; on := configuration; END_PROGRAM",
    );
    scan(&mut p, &mut Image::new(SIZES), 0);
    assert_eq!(int(&p, "on"), 3);
}

// ---- how a POU sees one ----

#[test]
fn var_external_must_match_a_global_of_this_module() {
    refused(
        "PROGRAM p VAR_EXTERNAL speed : INT; END_VAR END_PROGRAM",
        &format!(
            "VAR_EXTERNAL SPEED: this module has no VAR_GLOBAL SPEED. A global belongs to one module; {SIGNALS}"
        ),
    );
    refused(
        "FUNCTION_BLOCK b VAR_EXTERNAL speed : INT; END_VAR END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
        &format!("this module has no VAR_GLOBAL SPEED. A global belongs to one module; {SIGNALS}"),
    );
    // A local of the program is not a global.
    refused(
        "PROGRAM p VAR a : INT; END_VAR VAR_EXTERNAL a : INT; END_VAR END_PROGRAM",
        "this module has no VAR_GLOBAL A",
    );
    // Another module's variable by a qualified name.
    refused(
        "VAR_GLOBAL a : INT; END_VAR PROGRAM p VAR x : INT; END_VAR x := other.a; END_PROGRAM",
        "undeclared variable OTHER",
    );
}

#[test]
fn var_external_takes_exactly_the_globals_type() {
    refused(
        "VAR_GLOBAL a : INT; END_VAR PROGRAM p VAR_EXTERNAL a : DINT; END_VAR END_PROGRAM",
        "VAR_EXTERNAL A is DINT here and INT in its VAR_GLOBAL; it takes exactly that type",
    );
    refused(
        "VAR_GLOBAL a : ARRAY[0..2] OF INT; END_VAR PROGRAM p VAR_EXTERNAL a : ARRAY[0..3] OF INT; END_VAR END_PROGRAM",
        "VAR_EXTERNAL A is ARRAY",
    );
}

#[test]
fn var_external_takes_no_initial_value_and_no_qualifiers() {
    let g = "VAR_GLOBAL a : INT; END_VAR PROGRAM p";
    refused(
        &format!("{g} VAR_EXTERNAL a : INT := 3; END_VAR END_PROGRAM"),
        "A: a VAR_EXTERNAL takes no initial value: the VAR_GLOBAL gives it",
    );
    refused(
        &format!("{g} VAR_EXTERNAL a AT %MW0 : INT; END_VAR END_PROGRAM"),
        "A: a VAR_EXTERNAL cannot be located: the VAR_GLOBAL decides",
    );
    refused(
        &format!("{g} VAR_EXTERNAL RETAIN a : INT; END_VAR END_PROGRAM"),
        "A: a VAR_EXTERNAL cannot be RETAIN: the VAR_GLOBAL decides",
    );
    refused(
        &format!("{g} VAR_EXTERNAL a : INT; a : INT; END_VAR END_PROGRAM"),
        "A is declared twice",
    );
}

#[test]
fn a_constant_global_is_read_only_and_must_be_declared_so() {
    refused(
        "VAR_GLOBAL CONSTANT k : INT := 1; END_VAR PROGRAM p k := 2; END_PROGRAM",
        "K is a CONSTANT",
    );
    refused(
        "VAR_GLOBAL CONSTANT k : INT := 1; END_VAR FUNCTION_BLOCK b k := 2; END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
        "K is a CONSTANT",
    );
    refused(
        "VAR_GLOBAL CONSTANT k : INT := 1; END_VAR PROGRAM p VAR_EXTERNAL k : INT; END_VAR END_PROGRAM",
        "K is a CONSTANT global: declare it VAR_EXTERNAL CONSTANT",
    );
    refused(
        "VAR_GLOBAL CONSTANT k : INT; END_VAR PROGRAM p END_PROGRAM",
        "CONSTANT K needs an initial value",
    );
}

#[test]
fn var_external_constant_lets_a_pou_read_but_not_write() {
    let src = |body: &str| {
        format!(
            "VAR_GLOBAL n : INT; END_VAR
             FUNCTION_BLOCK b VAR_EXTERNAL CONSTANT n : INT; END_VAR VAR x : INT; END_VAR {body} END_FUNCTION_BLOCK
             PROGRAM p VAR m : b; END_VAR n := 1; m(); END_PROGRAM"
        )
    };
    build(&src("x := n;"));
    let want = "N is declared VAR_EXTERNAL CONSTANT in this POU, which can only read it";
    refused(&src("n := 1;"), want);
    refused(&src("FOR n := 1 TO 2 DO x := 1; END_FOR;"), want);
    refused(&src("n := n + 1;"), want);
    // The program (the module's owner of the slot) can narrow itself too.
    refused(
        "VAR_GLOBAL n : INT; END_VAR PROGRAM p VAR_EXTERNAL CONSTANT n : INT; END_VAR n := 1; END_PROGRAM",
        want,
    );
}

#[test]
fn a_local_cannot_take_a_globals_name() {
    let hide = "is a global variable of this module; a variable of the same name here would hide it";
    refused(
        "VAR_GLOBAL a : INT; END_VAR PROGRAM p VAR a : INT; END_VAR END_PROGRAM",
        hide,
    );
    refused(
        "VAR_GLOBAL a : INT; END_VAR FUNCTION_BLOCK b VAR_INPUT a : INT; END_VAR END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
        hide,
    );
    refused(
        "VAR_GLOBAL a : INT; END_VAR FUNCTION f : INT VAR a : INT; END_VAR END_FUNCTION PROGRAM p END_PROGRAM",
        hide,
    );
    refused(
        "VAR_GLOBAL f : INT; END_VAR FUNCTION f : INT f := 1; END_FUNCTION PROGRAM p END_PROGRAM",
        "a global variable of this module has the name of this function",
    );
    refused(
        "VAR_GLOBAL a : INT; a : INT; END_VAR PROGRAM p END_PROGRAM",
        "A is declared twice",
    );
}

#[test]
fn the_standard_blocks_are_not_disturbed_by_a_global_of_their_name() {
    // TON has its own IN, PT, Q and ET; a module's globals do not reach it.
    let mut p = build(
        "VAR_GLOBAL IN, Q, ET, PT : BOOL; END_VAR
         PROGRAM p VAR t : TON; ok : BOOL; END_VAR t(IN := TRUE, PT := T#10ms); ok := t.Q; Q := TRUE; END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    scan(&mut p, &mut img, 0);
    scan(&mut p, &mut img, 20);
    assert_eq!(p.get("ok"), Some(Value::Bool(true)));
}

#[test]
fn a_global_cannot_go_by_reference_because_the_callee_could_reach_it_twice() {
    let src = "VAR_GLOBAL buf : ARRAY[0..1] OF INT; END_VAR
               FUNCTION_BLOCK inc VAR_IN_OUT a : ARRAY[0..1] OF INT; END_VAR a[0] := a[0] + 1; buf[0] := buf[0] + 1; END_FUNCTION_BLOCK
               PROGRAM p VAR w : inc; own : ARRAY[0..1] OF INT; END_VAR w(a := buf); END_PROGRAM";
    refused(
        src,
        "A is a VAR_IN_OUT of INC: BUF is a global of this module, which the callee can also reach by its own name; copy it into a variable first",
    );
    // A local copy is the way, and element of a global likewise.
    refused(
        &src.replace("w(a := buf)", "w(a := buf[0])")
            .replace("ARRAY[0..1] OF INT; END_VAR a[0]", "INT; END_VAR a"),
        "is a global of this module",
    );
}

#[test]
fn a_misspelt_global_is_suggested() {
    refused(
        "VAR_GLOBAL conveyor_speed : INT; END_VAR FUNCTION_BLOCK b conveyor_sped := 1; END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
        "did you mean CONVEYOR_SPEED",
    );
}

#[test]
fn a_failed_global_is_reported_once_not_at_every_use() {
    let e = errors(
        "VAR_GLOBAL bad : NOSUCH; END_VAR FUNCTION_BLOCK b bad := 1; END_FUNCTION_BLOCK PROGRAM p VAR x : b; END_VAR x(); bad := 2; END_PROGRAM",
    );
    assert_eq!(e.lines().count(), 1, "{e}");
    assert!(e.contains("unknown type NOSUCH"), "{e}");
}

#[test]
fn globals_are_there_even_when_the_module_has_two_programs() {
    // The error is the second PROGRAM; the global in the block is not a second one.
    let e = errors(
        "VAR_GLOBAL g : INT; END_VAR FUNCTION_BLOCK b g := 1; END_FUNCTION_BLOCK PROGRAM p VAR x : b; END_VAR END_PROGRAM PROGRAM q END_PROGRAM",
    );
    assert_eq!(e.lines().count(), 1, "{e}");
    assert!(e.contains("only one PROGRAM is supported"), "{e}");
}

// ---- adversarial input ----

#[test]
fn hostile_declarations_are_errors_not_panics() {
    for src in [
        "VAR_GLOBAL",
        "VAR_GLOBAL END_VAR",
        "VAR_GLOBAL a : INT",
        "VAR_GLOBAL AT %IX0.0 : BOOL; END_VAR",
        "VAR_GLOBAL a, b AT %IX0.0 : BOOL; END_VAR PROGRAM p END_PROGRAM",
        "VAR_GLOBAL a : INT := b; b : INT := 1; END_VAR PROGRAM p END_PROGRAM",
        "VAR_GLOBAL a : INT := a; END_VAR PROGRAM p END_PROGRAM",
        "VAR_GLOBAL a : ARRAY[0..99999999] OF LINT; END_VAR PROGRAM p END_PROGRAM",
        "VAR_GLOBAL x : x2; END_VAR FUNCTION_BLOCK x2 VAR y : x2; END_VAR END_FUNCTION_BLOCK PROGRAM p END_PROGRAM",
        "VAR_GLOBAL a : INT; END_VAR VAR_GLOBAL a : INT; END_VAR",
        "CONFIGURATION",
        "CONFIGURATION c RESOURCE",
        "CONFIGURATION c RESOURCE r ON",
        "CONFIGURATION c RESOURCE r ON PLC",
        "CONFIGURATION 5 END_CONFIGURATION",
        "VAR_GLOBAL_X a : INT; END_VAR",
        "VAR_EXTERNAL a : INT; END_VAR",
        "FUNCTION f : INT VAR_EXTERNAL CONSTANT x : INT; END_VAR END_FUNCTION",
    ] {
        assert!(compile(src, SIZES).is_err(), "{src}");
    }
}

#[test]
fn a_global_cannot_be_located_twice_or_wrongly() {
    refused(
        "VAR_GLOBAL a AT %IX0.0 : INT; END_VAR PROGRAM p END_PROGRAM",
        "bits wide",
    );
    refused(
        "VAR_GLOBAL a AT %IX0.0 : BOOL := TRUE; END_VAR PROGRAM p END_PROGRAM",
        "an input (%I) cannot take an initial value",
    );
    refused(
        "VAR_GLOBAL CONSTANT a AT %QX0.0 : BOOL := TRUE; END_VAR PROGRAM p END_PROGRAM",
        "a CONSTANT cannot be located",
    );
}

// A located global is the same image bit for every POU, and an in-out of it
// is refused as any located variable is (tests/in_out.rs).

// ---- a global behaves as the same variable moved into the program ----

#[derive(Debug, Clone)]
enum Op {
    /// `g[k] := g[k] + v` in the program.
    Add(usize, i16),
    /// The same through a function block.
    BlockAdd(usize, i16),
    /// `g[k] := peek(j)` (a function reading a global) in the program.
    Copy(usize, usize),
    /// `IF g[k] > v THEN g[k] := 0` inside the block.
    BlockReset(usize, i16),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..3, -50i16..50).prop_map(|(k, v)| Op::Add(k, v)),
        (0usize..3, -50i16..50).prop_map(|(k, v)| Op::BlockAdd(k, v)),
        (0usize..3, 0usize..3).prop_map(|(k, j)| Op::Copy(k, j)),
        (0usize..3, -50i16..50).prop_map(|(k, v)| Op::BlockReset(k, v)),
    ]
}

fn lit(v: i16) -> String {
    if v < 0 { format!("({v})") } else { v.to_string() }
}

/// The module with `g0..g2` global, and the same logic with them as
/// the program's own variables (the block takes them by input and output).
fn programs(ops: &[Op]) -> (String, String) {
    let mut global = String::from(
        "VAR_GLOBAL g0, g1, g2 : INT; END_VAR
FUNCTION peek : INT VAR_INPUT j : INT; END_VAR
  CASE j OF 0: peek := g0; 1: peek := g1; ELSE peek := g2; END_CASE;
END_FUNCTION
FUNCTION_BLOCK blk
  VAR_INPUT k, v : INT; reset : BOOL; END_VAR
  IF reset THEN
    CASE k OF 0: IF g0 > v THEN g0 := 0; END_IF; 1: IF g1 > v THEN g1 := 0; END_IF; ELSE IF g2 > v THEN g2 := 0; END_IF; END_CASE;
  ELSE
    CASE k OF 0: g0 := g0 + v; 1: g1 := g1 + v; ELSE g2 := g2 + v; END_CASE;
  END_IF;
END_FUNCTION_BLOCK
PROGRAM p
VAR b : blk; END_VAR
",
    );
    let mut local = String::from(
        "PROGRAM p
VAR g0, g1, g2 : INT; END_VAR
",
    );
    for o in ops {
        let (g, l) = match o {
            Op::Add(k, v) => (
                format!("g{k} := g{k} + {};\n", lit(*v)),
                format!("g{k} := g{k} + {};\n", lit(*v)),
            ),
            Op::BlockAdd(k, v) => (
                format!("b(k := {k}, v := {}, reset := FALSE);\n", lit(*v)),
                format!("g{k} := g{k} + {};\n", lit(*v)),
            ),
            Op::Copy(k, j) => (format!("g{k} := peek({j});\n"), format!("g{k} := g{j};\n")),
            Op::BlockReset(k, v) => (
                format!("b(k := {k}, v := {}, reset := TRUE);\n", lit(*v)),
                format!("IF g{k} > {} THEN g{k} := 0; END_IF;\n", lit(*v)),
            ),
        };
        global.push_str(&g);
        local.push_str(&l);
    }
    global.push_str("END_PROGRAM\n");
    local.push_str("END_PROGRAM\n");
    (global, local)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn a_global_acts_as_the_same_variable_in_the_program(
        ops in proptest::collection::vec(op(), 0..10),
        scans in 1u64..4,
    ) {
        let (g, l) = programs(&ops);
        let (mut pg, mut pl) = (build(&g), build(&l));
        let (mut ig, mut il) = (Image::new(SIZES), Image::new(SIZES));
        for ms in 0..scans {
            let a = pg.scan_at(&mut ig, Duration::from_millis(ms * 10));
            let b = pl.scan_at(&mut il, Duration::from_millis(ms * 10));
            prop_assert_eq!(a.is_ok(), b.is_ok());
            for n in ["g0", "g1", "g2"] {
                prop_assert_eq!(pg.get(n), pl.get(n), "{} after scan {}\n{}", n, ms, g);
            }
        }
    }

    #[test]
    fn global_token_soup_never_panics(words in proptest::collection::vec(prop_oneof![
        Just("VAR_GLOBAL"), Just("VAR_EXTERNAL"), Just("END_VAR"), Just("CONSTANT"), Just("RETAIN"),
        Just("CONFIGURATION"), Just("END_CONFIGURATION"), Just("RESOURCE"), Just("END_RESOURCE"),
        Just("ON"), Just("TASK"), Just("PROGRAM"), Just("END_PROGRAM"), Just("FUNCTION_BLOCK"),
        Just("END_FUNCTION_BLOCK"), Just("a"), Just("b"), Just(":"), Just(";"), Just(":="), Just("INT"),
        Just("AT"), Just("%QX0.0"), Just("BOOL"), Just("1"), Just("("), Just(")"),
    ], 0..40)) {
        let src = words.join(" ");
        let _ = compile(&src, SIZES);
    }
}
