//! A `CONFIGURATION`'s `TASK` lines and program instances: read past, run
//! nothing, and each one a warning that says so. The forms are the ones the
//! ST corpus writes (matiec's and iec-checker's configuration files).
//!
//! No oracle case: ruSTy is not known to take these forms.

use std::time::Duration;

use helmsman_iec::{Diagnostic, Image, ImageSizes, Program, Value, compile, compile_warned};

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

const PROGRAM: &str = "PROGRAM p VAR n : INT; END_VAR n := n + 1; END_PROGRAM\n";

const NOT_TAKEN: &str = "was not taken from this file";
const RATE: &str = "a module's scan rate is set where the module is deployed";
const ONE_PROGRAM: &str = "a module runs its one PROGRAM";

fn warned(src: &str) -> (Program, Vec<Diagnostic>) {
    let (compiled, warnings) = compile_warned(src, SIZES);
    (compiled.unwrap_or_else(|d| panic!("{src}\n{d:#?}")), warnings)
}

fn texts(w: &[Diagnostic]) -> Vec<String> {
    w.iter().map(|d| d.msg.clone()).collect()
}

fn parse_error(src: &str) -> String {
    match compile(src, SIZES) {
        Ok(_) => panic!("compiled: {src}"),
        Err(d) => d.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
    }
}

fn n_after(p: &mut Program, scans: usize) -> i128 {
    let mut img = Image::new(SIZES);
    for _ in 0..scans {
        p.scan_at(&mut img, Duration::from_millis(10)).unwrap();
    }
    match p.get("n") {
        Some(Value::Int(i)) => i,
        other => panic!("n = {other:?}"),
    }
}

/// The matiec shape, which 40 of the corpus files use.
#[test]
fn a_task_and_its_instance_are_read_past_with_two_warnings() {
    let src = format!(
        "{PROGRAM}CONFIGURATION config
  RESOURCE resource1 ON PLC
    TASK main_task(INTERVAL := T#1000ms, PRIORITY := 0);
    PROGRAM instance0 WITH main_task : p;
  END_RESOURCE
END_CONFIGURATION
"
    );
    let (mut p, w) = warned(&src);
    assert_eq!(
        texts(&w),
        [
            format!("TASK MAIN_TASK (interval T#1s) {NOT_TAKEN}: {RATE}"),
            format!("program instance INSTANCE0 of P {NOT_TAKEN}: {ONE_PROGRAM}"),
        ]
    );
    assert_eq!((w[0].pos.line, w[0].pos.col), (4, 5));
    assert_eq!((w[1].pos.line, w[1].pos.col), (5, 5));
    assert_eq!(p.warnings(), &w[..], "the program carries the same warnings");
    assert_eq!(n_after(&mut p, 3), 3, "the program runs as if there were no task");
}

/// Nothing in a task changes how the program runs: the same program with
/// and without the configuration scans to the same state, whatever the
/// task says (a 1 ms interval, the highest priority, SINGLE).
#[test]
fn nothing_in_a_task_changes_how_the_program_runs() {
    let bare = {
        let mut p = compile(PROGRAM, SIZES).unwrap();
        n_after(&mut p, 5)
    };
    for task in [
        "TASK t(INTERVAL := T#1ms, PRIORITY := 0);",
        "TASK t(INTERVAL := T#1h, PRIORITY := 99);",
        "TASK t(SINGLE := g, PRIORITY := 1);",
        "TASK t(PRIORITY := 3);",
        "TASK t;",
        "TASK t();",
        "TASK t(INTERVAL := T#2h0m0s3ms,PRIORITY := 1);",
    ] {
        let src = format!(
            "VAR_GLOBAL g : BOOL; END_VAR\n{PROGRAM}CONFIGURATION c RESOURCE r ON PLC {task} PROGRAM i WITH t : p; END_RESOURCE END_CONFIGURATION"
        );
        let (mut p, w) = warned(&src);
        assert_eq!(w.len(), 2, "{task}: {w:#?}");
        assert_eq!(n_after(&mut p, 5), bare, "{task}");
    }
}

#[test]
fn each_form_of_a_task_says_its_interval_or_none() {
    let cases = [
        (
            "TASK t(INTERVAL := T#20ms, PRIORITY := 0);",
            "TASK T (interval T#20ms) ",
        ),
        (
            "TASK t(INTERVAL := T#2h0m0s3ms,PRIORITY := 1);",
            "TASK T (interval T#2h3ms) ",
        ),
        ("TASK t(INTERVAL := T#1h30m);", "TASK T (interval T#1h30m) "),
        ("TASK t(INTERVAL := T#0s);", "TASK T (interval T#0s) "),
        // No interval to say: SINGLE is an event, PRIORITY alone has none.
        ("TASK t(SINGLE := g, PRIORITY := 0);", "TASK T "),
        ("TASK t(PRIORITY := 2);", "TASK T "),
        ("TASK t;", "TASK T "),
        // An interval that is not a time literal is not guessed at.
        ("TASK t(INTERVAL := slow);", "TASK T "),
    ];
    for (task, want) in cases {
        let src = format!(
            "VAR_GLOBAL g : BOOL; slow : TIME; END_VAR\n{PROGRAM}CONFIGURATION c {task} END_CONFIGURATION"
        );
        let (_, w) = warned(&src);
        assert_eq!(w.len(), 1, "{task}: {w:#?}");
        assert_eq!(w[0].msg, format!("{want}{NOT_TAKEN}: {RATE}"), "{task}");
    }
}

/// iec-checker's plcopen-n5, n9 and cp26 put the task straight in the
/// CONFIGURATION, with no RESOURCE.
#[test]
fn tasks_and_instances_stand_in_a_configuration_without_a_resource() {
    let src = format!(
        "{PROGRAM}CONFIGURATION config
  VAR_GLOBAL MyCalculation : REAL; END_VAR
  TASK task0(INTERVAL := T#20ms, PRIORITY := 0);
  PROGRAM instance0 WITH task0 : p;
END_CONFIGURATION
"
    );
    let (mut p, w) = warned(&src);
    assert_eq!(w.len(), 2);
    assert_eq!(n_after(&mut p, 2), 2);
    assert!(p.get("MyCalculation").is_some(), "its global is still read");
}

/// iec-checker's plcopen-cp16: two tasks, three instances (one with no
/// task, one naming a function block), in one resource.
#[test]
fn several_tasks_and_instances_are_each_warned_about() {
    let src = format!(
        "{PROGRAM}FUNCTION_BLOCK MyFB END_FUNCTION_BLOCK
CONFIGURATION config
  RESOURCE res1 ON PLC
    TASK Fast(INTERVAL := T#10ms, PRIORITY := 1);
    TASK Slow(INTERVAL := T#100ms, PRIORITY := 2);
    PROGRAM inst1 WITH Fast : p;
    PROGRAM inst2 WITH Slow : MyFB;
    PROGRAM inst3 : p;
  END_RESOURCE
END_CONFIGURATION
"
    );
    let (_, w) = warned(&src);
    let t = texts(&w);
    assert_eq!(t.len(), 5, "{t:#?}");
    assert!(t[0].starts_with("TASK FAST (interval T#10ms) "));
    assert!(t[1].starts_with("TASK SLOW (interval T#100ms) "));
    assert!(t[2].starts_with("program instance INST1 of P "));
    assert!(t[3].starts_with("program instance INST2 of MYFB "));
    assert!(t[4].starts_with("program instance INST3 of P "));
    assert!(t.iter().all(|m| m.contains(NOT_TAKEN)));
}

#[test]
fn an_instance_may_carry_connections_and_a_retain_word() {
    let src = format!(
        "VAR_GLOBAL a, b : INT; END_VAR
{PROGRAM}CONFIGURATION c RESOURCE r ON PLC
  TASK t(INTERVAL := T#5ms, PRIORITY := 0);
  PROGRAM i1 WITH t : p(x := a + 1, y => b, z := (1 + (2 * 3)));
  PROGRAM RETAIN i2 WITH t : p();
  PROGRAM NON_RETAIN i3 : p(a);
END_RESOURCE END_CONFIGURATION"
    );
    let (mut p, w) = warned(&src);
    assert_eq!(w.len(), 4, "{w:#?}");
    // The names in the connections are not looked up: nothing runs.
    assert_eq!(n_after(&mut p, 2), 2);
}

#[test]
fn an_instance_of_a_type_that_does_not_exist_gets_the_same_warning() {
    // Rule: the type of an instance is never looked up, since nothing is
    // run from the instance. It is named in the warning and that is all.
    let src = format!("{PROGRAM}CONFIGURATION c PROGRAM i : Nowhere; END_CONFIGURATION");
    let (_, w) = warned(&src);
    assert_eq!(
        texts(&w),
        [format!(
            "program instance I of NOWHERE {NOT_TAKEN}: {ONE_PROGRAM}"
        )]
    );
}

#[test]
fn two_tasks_of_one_name_are_two_warnings() {
    // Rule: names are not checked for uniqueness: nothing is registered.
    let src = format!(
        "{PROGRAM}CONFIGURATION c TASK t(INTERVAL := T#1s); TASK t(INTERVAL := T#2s); END_CONFIGURATION"
    );
    let (_, w) = warned(&src);
    assert_eq!(w.len(), 2);
    assert!(w[0].msg.starts_with("TASK T (interval T#1s) "));
    assert!(w[1].msg.starts_with("TASK T (interval T#2s) "));
}

#[test]
fn a_warning_is_beside_an_error_not_instead_of_one() {
    let src = "PROGRAM p n := ; END_PROGRAM
CONFIGURATION c TASK t(INTERVAL := T#1s); END_CONFIGURATION";
    let (compiled, w) = compile_warned(src, SIZES);
    assert!(compiled.is_err(), "the parse error stops everything");
    assert!(w.is_empty(), "a source that does not parse has no warnings");
    let src = "PROGRAM p VAR n : INT; END_VAR n := TRUE; END_PROGRAM
CONFIGURATION c TASK t(INTERVAL := T#1s); END_CONFIGURATION";
    let (compiled, w) = compile_warned(src, SIZES);
    assert!(compiled.is_err());
    assert_eq!(w.len(), 1, "a type error leaves the warning in the list: {w:#?}");
}

// ---- still refused ----

#[test]
fn a_task_outside_a_configuration_is_still_refused() {
    for src in [
        format!("TASK t(INTERVAL := T#1s); {PROGRAM}"),
        format!("{PROGRAM}TASK t(INTERVAL := T#1s);"),
    ] {
        let e = parse_error(&src);
        assert!(e.contains("TASK"), "{src}\n{e}");
    }
    // A RESOURCE outside a CONFIGURATION is refused before its TASK is read.
    let e = parse_error(&format!("{PROGRAM}RESOURCE r ON PLC TASK t; END_RESOURCE"));
    assert!(e.contains("found RESOURCE"), "{e}");
}

#[test]
fn var_access_and_var_config_stay_refused_beside_tasks() {
    let e = parse_error(&format!(
        "{PROGRAM}CONFIGURATION c TASK t; VAR_ACCESS x : p.a : INT READ_ONLY; END_VAR END_CONFIGURATION"
    ));
    assert!(
        e.contains(
            "VAR_ACCESS is not supported: modules do not share variables, signals are how modules talk"
        ),
        "{e}"
    );
    let e = parse_error(&format!(
        "{PROGRAM}CONFIGURATION c TASK t; VAR_CONFIG p.a AT %IX0.0 : BOOL; END_VAR END_CONFIGURATION"
    ));
    assert!(e.contains("VAR_CONFIG is not supported"), "{e}");
}

#[test]
fn anything_else_in_a_configuration_is_refused_in_plain_words() {
    let e = parse_error(&format!("{PROGRAM}CONFIGURATION c x := 1; END_CONFIGURATION"));
    assert!(
        e.contains("expected VAR_GLOBAL, TASK, PROGRAM or END_CONFIGURATION, found X"),
        "{e}"
    );
    let e = parse_error(&format!(
        "{PROGRAM}CONFIGURATION c RESOURCE r ON PLC FUNCTION f : INT END_FUNCTION END_RESOURCE END_CONFIGURATION"
    ));
    assert!(
        e.contains("expected VAR_GLOBAL, TASK, PROGRAM or END_RESOURCE"),
        "{e}"
    );
    // A task parameter that is none of the three.
    let e = parse_error(&format!(
        "{PROGRAM}CONFIGURATION c TASK t(SPEED := 1); END_CONFIGURATION"
    ));
    assert!(
        e.contains("expected INTERVAL, PRIORITY or SINGLE in a TASK, found SPEED"),
        "{e}"
    );
}

#[test]
fn one_configuration_and_one_resource_still() {
    let e = parse_error(&format!(
        "{PROGRAM}CONFIGURATION a TASK t; END_CONFIGURATION CONFIGURATION b TASK u; END_CONFIGURATION"
    ));
    assert!(e.contains("a module has one CONFIGURATION"), "{e}");
    let e = parse_error(&format!(
        "{PROGRAM}CONFIGURATION a RESOURCE r ON x TASK t; END_RESOURCE RESOURCE s ON x TASK u; END_RESOURCE END_CONFIGURATION"
    ));
    assert!(e.contains("a module has one RESOURCE"), "{e}");
}

// ---- hostile input ----

#[test]
fn an_unclosed_task_is_an_error_not_a_hang() {
    for tail in [
        "TASK t(INTERVAL := T#1s",
        "TASK t(INTERVAL := T#1s,",
        "TASK t(INTERVAL := T#1s)",
        "TASK t(",
        "TASK",
        "TASK t(INTERVAL",
        "TASK t(INTERVAL :=",
        "PROGRAM i WITH",
        "PROGRAM i WITH t :",
        "PROGRAM i WITH t : p(x := ",
        "PROGRAM i WITH t : p",
    ] {
        let e = parse_error(&format!("{PROGRAM}CONFIGURATION c {tail}"));
        assert!(!e.is_empty(), "{tail}");
        let e = parse_error(&format!("{PROGRAM}CONFIGURATION c {tail} END_CONFIGURATION"));
        assert!(!e.is_empty(), "{tail}");
    }
}

#[test]
fn nesting_in_a_task_or_a_connection_is_capped() {
    let deep = 10_000;
    let open = "(".repeat(deep);
    let close = ")".repeat(deep);
    for item in [
        format!("TASK t(INTERVAL := {open}1{close});"),
        format!("TASK t(PRIORITY := {open}"),
        format!("PROGRAM i : p(x := {open}1{close});"),
        format!("PROGRAM i : p(x := {});", "f(".repeat(deep)),
        format!("TASK t(SINGLE := {}1);", "-".repeat(deep)),
        format!("TASK t(SINGLE := {}a{});", "a[".repeat(deep), "]".repeat(deep)),
    ] {
        let e = parse_error(&format!("{PROGRAM}CONFIGURATION c {item} END_CONFIGURATION"));
        assert!(
            e.contains("nested deeper than 200 levels"),
            "{}",
            &e[..e.len().min(200)]
        );
    }
}
