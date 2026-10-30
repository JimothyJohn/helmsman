//! The trace tap: what a traced scan reports, and that tracing never
//! changes what a program does.

use std::fmt::Write as _;
use std::time::Duration;

use helmsman_iec::trace::{Shape, StmtId, Trace};
use helmsman_iec::{FaultKind, Image, ImageSizes, Program, Value, compile};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

fn build(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{src}\n{d:#?}"))
}

/// One traced scan with `%IB0 = sel` and the clock at `ms`.
fn traced(p: &mut Program, sel: u8, ms: u64) -> Trace {
    let mut img = Image::new(SIZES);
    img.i[0] = sel;
    let (r, t) = p.scan_traced(&mut img, Duration::from_millis(ms));
    assert_eq!(r.err(), t.fault());
    t
}

/// The user statement starting on the first source line containing
/// `marker` (the leftmost, if several start there).
fn at(src: &str, t: &Trace, marker: &str) -> StmtId {
    let line = src
        .lines()
        .position(|l| l.contains(marker))
        .unwrap_or_else(|| panic!("no line has {marker:?}"));
    let line = u32::try_from(line + 1).unwrap();
    t.map()
        .at_line(line)
        .min_by_key(|&id| t.map().stmt(id).unwrap().col)
        .unwrap_or_else(|| panic!("no statement on line {line} ({marker:?})"))
}

fn int(t: &Trace, path: &str) -> i128 {
    match t.get(path) {
        Some(Value::Int(i)) => i,
        other => panic!("{path} = {other:?}"),
    }
}

fn boolean(t: &Trace, path: &str) -> bool {
    match t.get(path) {
        Some(Value::Bool(b)) => b,
        other => panic!("{path} = {other:?}"),
    }
}

const IF_SRC: &str = "PROGRAM t
VAR sel AT %IB0 : USINT; r : INT; n : INT; END_VAR
IF sel = 1 THEN
  r := 10;
ELSIF sel = 2 THEN
  r := 20;
ELSIF sel = 3 THEN
  r := 30;
ELSE
  r := 99;
END_IF;
IF sel > 100 THEN
  n := n + 1;
END_IF;
END_PROGRAM";

#[test]
fn if_elsif_else_reports_the_arm_taken_and_what_ran() {
    let mut p = build(IF_SRC);
    for (sel, arm, want) in [(1u8, 0usize, 10), (2, 1, 20), (3, 2, 30), (7, 3, 99)] {
        let t = traced(&mut p, sel, 0);
        let iff = at(IF_SRC, &t, "IF sel = 1");
        assert_eq!(t.map().stmt(iff).unwrap().shape, Shape::If { arms: 3 });
        assert_eq!(t.hits(iff), 1);
        assert_eq!(t.arm(iff), Some(arm), "sel {sel}");
        let mut counts = [0u32; 4];
        counts[arm] = 1;
        assert_eq!(t.arm_hits(iff), counts.as_slice());
        for (k, marker) in ["r := 10", "r := 20", "r := 30", "r := 99"].iter().enumerate() {
            let s = at(IF_SRC, &t, marker);
            assert_eq!(t.ran(s), k == arm, "sel {sel}: {marker}");
            assert_eq!(t.map().stmt(s).unwrap().parent, Some((iff, k)));
        }
        assert_eq!(int(&t, "r"), want);
        // An IF without ELSE whose condition fails takes "no arm".
        let second = at(IF_SRC, &t, "IF sel > 100");
        assert_eq!(t.arm(second), Some(1));
        assert!(!t.ran(at(IF_SRC, &t, "n := n + 1")));
    }
}

const CASE_SRC: &str = "PROGRAM t
VAR sel AT %IB0 : USINT; r : INT; END_VAR
CASE sel OF
  0: r := 0;
  1, 2: r := 12;
  5..9: r := 59;
ELSE
  r := -1;
END_CASE;
END_PROGRAM";

#[test]
fn case_reports_the_arm_taken() {
    let mut p = build(CASE_SRC);
    for (sel, arm, want) in [(0u8, 0usize, 0), (2, 1, 12), (7, 2, 59), (9, 2, 59), (4, 3, -1)] {
        let t = traced(&mut p, sel, 0);
        let case = at(CASE_SRC, &t, "CASE sel");
        assert_eq!(t.map().stmt(case).unwrap().shape, Shape::Case { arms: 3 });
        assert_eq!(t.arm(case), Some(arm), "sel {sel}");
        assert_eq!(int(&t, "r"), want);
        for (k, marker) in ["r := 0;", "r := 12", "r := 59", "r := -1"].iter().enumerate() {
            assert_eq!(t.ran(at(CASE_SRC, &t, marker)), k == arm, "sel {sel}: {marker}");
        }
    }
}

const LOOP_SRC: &str = "PROGRAM t
VAR sel AT %IB0 : USINT; i, j, k, sum : INT; END_VAR
sum := 0;
FOR i := 1 TO 10 DO
  IF i > sel THEN
    EXIT;
  END_IF;
  sum := sum + i;
END_FOR;
j := 0;
WHILE j < 20 DO
  j := j + 1;
  IF j = 4 THEN EXIT; END_IF;
END_WHILE;
k := 0;
REPEAT
  k := k + 2;
UNTIL k >= 6
END_REPEAT;
END_PROGRAM";

#[test]
fn loops_count_passes_and_exit_shows_where_they_stopped() {
    let mut p = build(LOOP_SRC);
    let t = traced(&mut p, 3, 0);
    let for_ = at(LOOP_SRC, &t, "FOR i");
    assert_eq!(t.map().stmt(for_).unwrap().shape, Shape::For);
    // Passes 1..3 add, pass 4 exits before adding.
    assert_eq!(t.passes(for_), Some(4));
    assert_eq!(t.hits(at(LOOP_SRC, &t, "IF i > sel")), 4);
    assert_eq!(t.hits(at(LOOP_SRC, &t, "EXIT;")), 1);
    assert_eq!(t.hits(at(LOOP_SRC, &t, "sum := sum + i")), 3);
    assert_eq!(int(&t, "sum"), 6);
    assert_eq!(t.arm_hits(at(LOOP_SRC, &t, "IF i > sel")), &[1, 3]);

    let while_ = at(LOOP_SRC, &t, "WHILE j");
    assert_eq!(t.passes(while_), Some(4));
    assert_eq!(t.hits(at(LOOP_SRC, &t, "j := j + 1")), 4);
    let exit_if = at(LOOP_SRC, &t, "IF j = 4");
    assert_eq!(t.arm_hits(exit_if), &[1, 3]);
    assert_eq!(t.arm(exit_if), Some(0), "the last pass took the EXIT");

    let repeat = at(LOOP_SRC, &t, "REPEAT");
    assert_eq!(t.passes(repeat), Some(3));
    assert_eq!(int(&t, "k"), 6);
    // Not loops: no passes.
    assert_eq!(t.passes(exit_if), None);
    assert_eq!(t.arm(while_), None);
    assert_eq!(t.arm_hits(while_), &[] as &[u32]);

    // No EXIT taken: the FOR runs all ten passes.
    let t = traced(&mut p, 50, 0);
    assert_eq!(t.passes(at(LOOP_SRC, &t, "FOR i")), Some(10));
    assert!(!t.ran(at(LOOP_SRC, &t, "EXIT;")));
    assert_eq!(int(&t, "sum"), 55);
}

const FB_SRC: &str = "FUNCTION_BLOCK acc
VAR_INPUT x : INT; END_VAR
VAR_OUTPUT total : INT; END_VAR
IF x > 0 THEN
  total := total + x;
END_IF;
END_FUNCTION_BLOCK
FUNCTION twice : INT
VAR_INPUT v : INT; END_VAR
twice := v * 2;
END_FUNCTION
PROGRAM t
VAR sel AT %IB0 : USINT; a, b : acc; r : INT; END_VAR
a(x := 5);
IF sel = 1 THEN
  b(x := twice(3));
END_IF;
r := a.total + b.total;
END_PROGRAM";

#[test]
fn fb_and_function_bodies_count_every_call_and_instances_hold_their_values() {
    let mut p = build(FB_SRC);
    let t = traced(&mut p, 0, 0);
    let call_a = at(FB_SRC, &t, "a(x := 5)");
    assert_eq!(t.map().stmt(call_a).unwrap().shape, Shape::Call);
    assert!(t.ran(call_a));
    assert!(!t.ran(at(FB_SRC, &t, "b(x := twice(3))")));
    let body = at(FB_SRC, &t, "total := total + x");
    assert_eq!(t.map().stmt(body).unwrap().pou, "ACC");
    assert_eq!(t.hits(body), 1, "one call, from a");
    assert!(!t.ran(at(FB_SRC, &t, "twice := v * 2")));
    assert_eq!(int(&t, "a.total"), 5);
    assert_eq!(int(&t, "b.total"), 0);
    assert_eq!(int(&t, "a.x"), 5, "instance inputs are visible");

    let t = traced(&mut p, 1, 0);
    assert_eq!(t.hits(body), 2, "a and b both ran the shared body");
    assert_eq!(
        t.hits(at(FB_SRC, &t, "twice := v * 2")),
        1,
        "a function called in an expression"
    );
    assert_eq!(int(&t, "a.total"), 10);
    assert_eq!(int(&t, "b.total"), 6);
    assert_eq!(int(&t, "r"), 16);
    // The program's statements, in source order.
    let lines: Vec<u32> = t
        .map()
        .in_pou("t")
        .map(|id| t.map().stmt(id).unwrap().line)
        .collect();
    assert_eq!(lines, [14, 15, 16, 18]);
}

const NESTED_SRC: &str = "FUNCTION_BLOCK edge_count
VAR_INPUT in : BOOL; END_VAR
VAR_OUTPUT n : INT; END_VAR
VAR rise : R_TRIG; END_VAR
rise(CLK := in);
IF rise.Q THEN
  n := n + 1;
END_IF;
END_FUNCTION_BLOCK
FUNCTION_BLOCK station
VAR_INPUT go : BOOL; END_VAR
VAR_OUTPUT busy : BOOL; END_VAR
VAR counter : edge_count; hold : TON; END_VAR
counter(in := go);
hold(IN := go, PT := T#100ms);
busy := go AND NOT hold.Q;
END_FUNCTION_BLOCK
PROGRAM t
VAR go AT %IX0.0 : BOOL; s : station; lamp AT %QX0.0 : BOOL; END_VAR
s(go := go);
lamp := s.busy;
END_PROGRAM";

#[test]
fn nested_fbs_expose_every_member_and_library_statements_are_marked() {
    let mut p = build(NESTED_SRC);
    let t = traced(&mut p, 1, 0);
    assert_eq!(int(&t, "s.counter.n"), 1);
    assert!(boolean(&t, "s.counter.rise.Q"));
    assert!(boolean(&t, "S.HOLD.IN"), "paths are case-insensitive");
    assert!(boolean(&t, "s.busy"));
    assert!(boolean(&t, "lamp"), "located variables read the image");
    assert_eq!(t.image().q[0] & 1, 1);
    assert!(t.ran(at(NESTED_SRC, &t, "n := n + 1")));
    // TON and R_TRIG bodies ran, and are flagged as the library's.
    let lib: Vec<_> = t
        .map()
        .stmts()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.system && (s.pou == "TON" || s.pou == "R_TRIG"))
        .map(|(i, _)| StmtId(u32::try_from(i).unwrap()))
        .collect();
    assert!(lib.iter().any(|&id| t.ran(id)));
    assert!(t.map().at_line(1).all(|id| !t.map().stmt(id).unwrap().system));

    // Held for 150 ms: the timer is done, the edge counted once.
    let t = traced(&mut p, 1, 150);
    assert!(boolean(&t, "s.hold.Q"));
    assert!(!boolean(&t, "lamp"));
    assert_eq!(int(&t, "s.counter.n"), 1);
    assert!(!t.ran(at(NESTED_SRC, &t, "n := n + 1")));
    assert_eq!(t.now(), Duration::from_millis(150));

    // Every member is listed, with types.
    let vars: Vec<&str> = t.map().vars().map(|(p, _)| p).collect();
    for want in ["GO", "S.GO", "S.COUNTER.RISE.CLK", "S.HOLD.ET", "LAMP"] {
        assert!(vars.contains(&want), "{want} in {vars:?}");
    }
    assert_eq!(t.values().count(), vars.len());
}

const FAULT_SRC: &str = "FUNCTION_BLOCK divider
VAR_INPUT d : INT; END_VAR
VAR_OUTPUT q : INT; END_VAR
q := 100 / d;
END_FUNCTION_BLOCK
PROGRAM t
VAR sel AT %IB0 : USINT; f : divider; before, after : INT; a : ARRAY[0..3] OF INT; i : INT; END_VAR
before := before + 1;
IF sel = 1 THEN
  f(d := 0);
ELSIF sel = 2 THEN
  FOR i := 0 TO 5 DO
    a[i] := i;
  END_FOR;
ELSIF sel = 3 THEN
  WHILE TRUE DO
    i := i + 1;
  END_WHILE;
END_IF;
after := 1;
END_PROGRAM";

#[test]
fn a_fault_ends_the_trace_at_the_statement_that_faulted() {
    // Division by zero inside a function block body.
    let mut p = build(FAULT_SRC);
    let t = traced(&mut p, 1, 0);
    assert_eq!(t.fault().map(|f| f.kind), Some(FaultKind::DivisionByZero));
    let culprit = at(FAULT_SRC, &t, "q := 100 / d");
    assert_eq!(t.faulted_at(), Some(culprit));
    assert!(t.ran(culprit));
    assert_eq!(t.arm(at(FAULT_SRC, &t, "IF sel = 1")), Some(0));
    assert!(!t.ran(at(FAULT_SRC, &t, "after := 1")), "nothing past the fault");
    assert_eq!(int(&t, "before"), 1);
    assert_eq!(int(&t, "after"), 0);

    // Index out of range on the fifth pass of a FOR.
    let mut p = build(FAULT_SRC);
    let t = traced(&mut p, 2, 0);
    assert_eq!(t.fault().map(|f| f.kind), Some(FaultKind::IndexOutOfRange));
    let store = at(FAULT_SRC, &t, "a[i] := i");
    assert_eq!(t.faulted_at(), Some(store));
    assert_eq!(t.hits(store), 5);
    assert_eq!(t.passes(at(FAULT_SRC, &t, "FOR i")), Some(5));
    assert_eq!(int(&t, "a[3]"), 3);
    assert_eq!(int(&t, "i"), 4);
    assert!(!t.ran(at(FAULT_SRC, &t, "after := 1")));

    // The watchdog in an endless WHILE.
    let mut p = build(FAULT_SRC);
    p.set_budget(1_000);
    let t = traced(&mut p, 3, 0);
    assert_eq!(t.fault().map(|f| f.kind), Some(FaultKind::Watchdog));
    let w = at(FAULT_SRC, &t, "WHILE TRUE");
    let body = at(FAULT_SRC, &t, "i := i + 1");
    assert!(t.faulted_at() == Some(w) || t.faulted_at() == Some(body));
    assert!(t.passes(w).unwrap() > 400);
    assert!(!t.ran(at(FAULT_SRC, &t, "after := 1")));

    // A clean scan has no fault.
    let mut p = build(FAULT_SRC);
    let t = traced(&mut p, 0, 0);
    assert_eq!(t.fault(), None);
    assert_eq!(t.faulted_at(), None);
    assert!(t.ran(at(FAULT_SRC, &t, "after := 1")));
}

#[test]
fn values_cover_arrays_structs_enums_and_the_map_is_shared() {
    let src = "TYPE
  Mode : (Idle, Run, Stop);
  Point : STRUCT x : INT; y : REAL; END_STRUCT;
END_TYPE
PROGRAM t
VAR grid : ARRAY[1..2, 0..1] OF INT; p : Point; m : Mode; pts : ARRAY[0..1] OF Point; END_VAR
grid[2, 1] := 21;
p.y := 1.5;
m := Stop;
pts[1].x := 7;
END_PROGRAM";
    let mut p = build(src);
    let t = traced(&mut p, 0, 0);
    assert_eq!(int(&t, "grid[2,1]"), 21);
    assert_eq!(int(&t, "grid[ 2, 1 ]"), 21);
    assert_eq!(int(&t, "grid[1,0]"), 0);
    assert_eq!(t.get("p.y"), Some(Value::Real(1.5)));
    assert_eq!(int(&t, "m"), 2);
    assert_eq!(int(&t, "pts[1].x"), 7);
    assert_eq!(t.get("grid[3,0]"), None);
    assert_eq!(t.get("nope"), None);
    let vars: Vec<&str> = t.map().vars().map(|(p, _)| p).collect();
    assert_eq!(&vars[..4], ["GRID[1,0]", "GRID[1,1]", "GRID[2,0]", "GRID[2,1]"]);
    // Each scan's trace is its own; the map is built once.
    let t2 = traced(&mut p, 0, 10);
    assert!(std::ptr::eq(t.map(), t2.map()));
    assert!(std::sync::Arc::ptr_eq(&p.trace_map(), &p.clone().trace_map()));
}

// ---- tracing never changes what a program does ----

/// A small random ST program over three INTs, two BOOLs and an input
/// byte, using every statement kind. Divisions may fault and loops may
/// run out of budget: both runs must then fault alike.
fn stmt() -> impl Strategy<Value = String> {
    let var = prop_oneof![Just("a"), Just("b"), Just("c")];
    let expr = (var.clone(), 0..5u8, -9i32..10, var.clone()).prop_map(|(v, op, k, w)| match op {
        0 => format!("{v} + {k}"),
        1 => format!("{v} * {k} - {w}"),
        2 => format!("{v} / ({w} MOD 4)"),
        3 => format!("BYTE_TO_INT(inp) - {k}"),
        _ => format!("{v} MOD 7"),
    });
    let cond = (var.clone(), 0..4u8, -5i32..6).prop_map(|(v, op, k)| match op {
        0 => format!("{v} > {k}"),
        1 => format!("{v} = {k}"),
        2 => "x".to_string(),
        _ => format!("y AND {v} < {k}"),
    });
    let leaf = prop_oneof![
        (var.clone(), expr.clone()).prop_map(|(v, e)| format!("{v} := {e};")),
        cond.clone().prop_map(|c| format!("x := {c};")),
        Just("y := NOT y;".to_string()),
        Just("f(in := x);".to_string()),
    ];
    leaf.prop_recursive(3, 24, 4, move |inner| {
        let block = proptest::collection::vec(inner, 1..4).prop_map(|v| v.join("\n"));
        prop_oneof![
            (cond.clone(), block.clone(), block.clone())
                .prop_map(|(c, t, e)| format!("IF {c} THEN\n{t}\nELSE\n{e}\nEND_IF;")),
            (var.clone(), block.clone(), block.clone()).prop_map(|(v, x, y)| format!(
                "CASE {v} OF\n0, 1: {x}\n2..5: {y}\nELSE\nc := c + 1;\nEND_CASE;"
            )),
            (0..4i32, block.clone()).prop_map(|(n, b)| format!(
                "FOR i := 0 TO {n} DO\n{b}\nIF i = a THEN EXIT; END_IF;\nEND_FOR;"
            )),
            (block.clone(), cond.clone()).prop_map(|(b, c)| format!(
                "n := 0;\nWHILE n < 3 AND NOT ({c}) DO\nn := n + 1;\n{b}\nEND_WHILE;"
            )),
            (block, cond.clone()).prop_map(|(b, c)| format!("REPEAT\n{b}\nUNTIL {c}\nEND_REPEAT;")),
        ]
    })
}

fn program(body: &[String]) -> String {
    let mut s = String::from(
        "FUNCTION_BLOCK pulse
VAR_INPUT in : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
VAR r : R_TRIG; t : TOF; END_VAR
r(CLK := in);
t(IN := r.Q, PT := T#20ms);
q := t.Q;
END_FUNCTION_BLOCK
PROGRAM t
VAR inp AT %IB0 : BYTE; a, b, c, i, n : INT; x, y : BOOL; f : pulse; o AT %QW0 : INT; END_VAR
",
    );
    for st in body {
        writeln!(s, "{st}").unwrap();
    }
    s.push_str("o := a + b + c;\nEND_PROGRAM\n");
    s
}

/// Every scalar of the program, printed (so NaN compares equal to NaN).
fn state(p: &Program, img: &Image) -> Vec<String> {
    let mut out: Vec<String> = p
        .trace_map()
        .vars()
        .map(|(path, _)| format!("{path}={:?}", p.get(path)))
        .collect();
    out.push(format!("{img:?}"));
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Tracing any subset of scans leaves every variable, the image and
    /// every scan result exactly as the untraced run does.
    #[test]
    fn tracing_never_changes_results(
        body in proptest::collection::vec(stmt(), 1..6),
        inputs in proptest::collection::vec(any::<u8>(), 1..12),
        mask in any::<u16>(),
    ) {
        let src = program(&body);
        let Ok(plain) = compile(&src, SIZES) else {
            return Err(TestCaseError::fail(format!("does not compile:\n{src}")));
        };
        let (mut plain, mut traced) = (plain.clone(), plain);
        plain.set_budget(5_000);
        traced.set_budget(5_000);
        let (mut ia, mut ib) = (Image::new(SIZES), Image::new(SIZES));
        for (n, &inp) in inputs.iter().enumerate() {
            ia.i[0] = inp;
            ib.i[0] = inp;
            let now = Duration::from_millis(u64::try_from(n).unwrap() * 7);
            let ra = plain.scan_at(&mut ia, now);
            let rb = if mask >> (n % 16) & 1 == 1 {
                let (r, t) = traced.scan_traced(&mut ib, now);
                prop_assert_eq!(r.err(), t.fault());
                prop_assert_eq!(t.image(), &ib);
                // Located variables live in the image (compared above).
                for (path, v) in t.values() {
                    if let Some(got) = traced.get(path) {
                        prop_assert_eq!(format!("{v:?}"), format!("{got:?}"), "{}", path);
                    }
                }
                r
            } else {
                traced.scan_at(&mut ib, now)
            };
            prop_assert_eq!(ra, rb, "scan {}\n{}", n, src);
            prop_assert_eq!(state(&plain, &ia), state(&traced, &ib), "scan {}\n{}", n, src);
            if ra.is_err() {
                break;
            }
        }
    }
}

/// The library doc's blink program, traced on every scan, matches the
/// untraced run.
#[test]
fn blink_traced_every_scan_matches_untraced() {
    let src = "
PROGRAM blink
VAR pulse : TON; on : BOOL; END_VAR
  pulse(IN := NOT pulse.Q, PT := T#500ms);
  IF pulse.Q THEN on := NOT on; END_IF;
  %QX0.0 := on;
END_PROGRAM";
    let mut a = build(src);
    let mut b = build(src);
    let (mut ia, mut ib) = (Image::new(SIZES), Image::new(SIZES));
    let mut toggles = 0;
    for ms in (0..=1100).step_by(10) {
        let now = Duration::from_millis(ms);
        a.scan_at(&mut ia, now).unwrap();
        let (r, t) = b.scan_traced(&mut ib, now);
        r.unwrap();
        let iff = at(src, &t, "IF pulse.Q");
        if t.arm(iff) == Some(0) {
            toggles += 1;
        }
        assert_eq!(state(&a, &ia), state(&b, &ib), "at {ms} ms");
    }
    assert_eq!(toggles, 2, "fired at 500 ms and 1020 ms");
}
