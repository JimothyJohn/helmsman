//! Functions, function blocks, user types, TIME, the standard functions and
//! the standard function blocks.

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

fn boolean(p: &Program, path: &str) -> bool {
    match p.get(path) {
        Some(Value::Bool(b)) => b,
        other => panic!("{path} = {other:?}"),
    }
}

fn errors(src: &str) -> String {
    match compile(src, SIZES) {
        Ok(_) => panic!("compiled: {src}"),
        Err(d) => d.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
    }
}

const MS: i128 = 1_000_000;

// ---- functions ----

#[test]
fn functions_positional_named_nested_and_defaults() {
    let mut p = build(
        "FUNCTION add3 : DINT
           VAR_INPUT a, b : DINT; c : DINT := 100; END_VAR
           VAR tmp : DINT; END_VAR
           tmp := a + b;
           add3 := tmp + c;
         END_FUNCTION
         FUNCTION clamp_pos : INT
           VAR_INPUT x : INT; END_VAR
           IF x < 0 THEN clamp_pos := 0; RETURN; END_IF;
           clamp_pos := x;
         END_FUNCTION
         PROGRAM t
           VAR r1, r2, r3, r4 : DINT; c1, c2 : INT; END_VAR
           r1 := add3(1, 2, 3);
           r2 := add3(a := 1, b := 2);
           r3 := add3(add3(1, 1, 1), 10, 0);
           r4 := add3(c := 0, b := 5, a := 5);
           c1 := clamp_pos(-7);
           c2 := clamp_pos(7);
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "r1"), 6);
    assert_eq!(
        int(&p, "r2"),
        103,
        "an input left out takes its initial value on every call"
    );
    assert_eq!(int(&p, "r3"), 13, "nested calls of the same function");
    assert_eq!(int(&p, "r4"), 10);
    assert_eq!(int(&p, "c1"), 0);
    assert_eq!(int(&p, "c2"), 7);
    scan(&mut p, 10);
    assert_eq!(int(&p, "r2"), 103, "function locals do not persist between calls");
}

#[test]
fn function_errors() {
    let e = errors(
        "FUNCTION f : INT VAR_INPUT a : INT; END_VAR f := a; END_FUNCTION
         FUNCTION g : INT VAR_OUTPUT o : INT; END_VAR g := 1; END_FUNCTION
         PROGRAM t VAR x : INT; s : SINT; END_VAR
           x := f(1, 2);
           x := f(b := 1);
           x := f(a := 1, a := 2);
           s := f(1);
           f(1);
           x := f;
         END_PROGRAM",
    );
    for want in [
        "VAR_OUTPUT is not supported in a FUNCTION yet",
        "F takes 1 input(s)",
        "F has no input or output B",
        "A is given twice",
        "expected SINT, found INT",
        "undeclared variable F (a function is called with parentheses)",
    ] {
        assert!(e.contains(want), "missing {want:?} in\n{e}");
    }
}

#[test]
fn recursion_is_rejected() {
    let e = errors(
        "FUNCTION a : INT VAR_INPUT x : INT; END_VAR a := b(x); END_FUNCTION
         FUNCTION b : INT VAR_INPUT x : INT; END_VAR b := a(x); END_FUNCTION
         FUNCTION_BLOCK fb VAR_INPUT x : INT; END_VAR VAR y : INT; END_VAR y := c(x); END_FUNCTION_BLOCK
         FUNCTION c : INT VAR_INPUT x : INT; END_VAR VAR i : fb; END_VAR i(x := x); c := 0; END_FUNCTION
         PROGRAM t END_PROGRAM",
    );
    assert!(e.contains("recursion is not allowed: A -> B -> A"), "{e}");
    assert!(
        e.contains("recursion is not allowed: C -> FB -> C") || e.contains("FB -> C -> FB"),
        "{e}"
    );
}

// ---- function blocks ----

#[test]
fn function_blocks_keep_state_per_instance() {
    let mut p = build(
        "FUNCTION_BLOCK acc
           VAR_INPUT add : DINT; reset : BOOL; END_VAR
           VAR_OUTPUT total : DINT; calls : INT; END_VAR
           VAR_TEMP scratch : DINT := 5; END_VAR
           scratch := scratch + 1;
           IF reset THEN total := 0; ELSE total := total + add; END_IF;
           calls := calls + 1;
         END_FUNCTION_BLOCK
         PROGRAM t
           VAR a, b : acc; bank : ARRAY[0..2] OF acc; i : INT; got : DINT; big : LINT; END_VAR
           a(add := 2);
           b(add := 10);
           a();
           b.add := 1;
           b(Total => got);
           a(add := 3, total => big);
           FOR i := 0 TO 2 DO bank[i](add := i); END_FOR;
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "a.total"), 7, "a(): inputs keep their last value");
    assert_eq!(int(&p, "a.calls"), 3);
    assert_eq!(int(&p, "b.total"), 11);
    assert_eq!(int(&p, "got"), 11, "=> copies an output after the call");
    assert_eq!(int(&p, "big"), 7, "=> widens DINT to LINT");
    assert_eq!(int(&p, "a.scratch"), 6, "VAR_TEMP is reset on every call");
    for k in 0..3 {
        assert_eq!(int(&p, &format!("bank[{k}].total")), i128::from(k));
    }
    scan(&mut p, 10);
    assert_eq!(int(&p, "a.total"), 7 + 7);
    assert_eq!(int(&p, "bank[2].total"), 4);
}

#[test]
fn function_block_errors() {
    let e = errors(
        "FUNCTION_BLOCK fb
           VAR_INPUT i : INT; END_VAR VAR_OUTPUT o : INT; END_VAR VAR hidden : INT; END_VAR
           VAR_IN_OUT r : INT; END_VAR
           o := i;
         END_FUNCTION_BLOCK
         FUNCTION_BLOCK loop1 VAR inner : loop2; END_VAR END_FUNCTION_BLOCK
         FUNCTION_BLOCK loop2 VAR inner : loop1; END_VAR END_FUNCTION_BLOCK
         FUNCTION_BLOCK selfish VAR me : selfish; END_VAR END_FUNCTION_BLOCK
         PROGRAM t VAR x : fb; v : INT; END_VAR
           x.o := 1;
           v := x.hidden;
           v := x(i := 1);
           fb(i := 1);
           x(o := 1);
           x(i => v);
           x(o => 5);
         END_PROGRAM",
    );
    for want in [
        "VAR_IN_OUT is not supported in a FUNCTION_BLOCK yet",
        "contains an instance of itself",
        "the target is an output of the function block",
        "HIDDEN is internal to FB",
        "X is a variable; a function block call is a statement",
        "FB is a function block type; declare an instance and call that",
        "O is not an input of FB; outputs are bound with =>",
        "I is not an output of FB",
        "expected a variable",
    ] {
        assert!(e.contains(want), "missing {want:?} in\n{e}");
    }
}

// ---- user types ----

#[test]
fn structs_arrays_and_enums() {
    let mut p = build(
        "TYPE
           Color : (Red, Amber := 5, Green);
           Point : STRUCT x : INT := 1; y : REAL; END_STRUCT;
           Path : ARRAY[1..3] OF Point;
           Count : DINT := 42;
         END_TYPE
         FUNCTION next : Color VAR_INPUT c : Color; END_VAR
           CASE c OF Red: next := Green; Green: next := Amber; ELSE next := Red; END_CASE;
         END_FUNCTION
         PROGRAM t
           VAR
             p : Point; q : Point := (y := 2.5);
             path : Path;
             grid : ARRAY[1..2, 0..2] OF INT := [1, 2, 3, 2(9)];
             light : Color := Amber;
             n : Count;
             i, j, sum : INT;
             same, is_green : BOOL;
           END_VAR
           p.x := p.x + 1;
           path[2] := q;
           path[3].x := 30;
           FOR i := 1 TO 2 DO FOR j := 0 TO 2 DO sum := sum + grid[i, j]; END_FOR; END_FOR;
           same := path[2].y = q.y;
           light := next(light);
           is_green := light = Color#Green OR light = Green;
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "p.x"), 2);
    assert_eq!(p.get("path[2].y"), Some(Value::Real(2.5)));
    assert_eq!(int(&p, "path[2].x"), 1);
    assert_eq!(int(&p, "path[3].x"), 30);
    assert_eq!(
        int(&p, "path[1].x"),
        1,
        "unlisted elements keep the type's initial value"
    );
    assert_eq!(int(&p, "grid[2,2]"), 0, "[1, 2, 3, 2(9)] fills five of six");
    assert_eq!(int(&p, "sum"), 1 + 2 + 3 + 9 + 9);
    assert!(boolean(&p, "same"));
    assert_eq!(int(&p, "light"), 0, "Amber -> Red");
    assert_eq!(int(&p, "n"), 42, "alias initial value");
    scan(&mut p, 10);
    assert_eq!(int(&p, "light"), 6, "Red -> Green (Amber = 5, Green = 6)");
    assert!(boolean(&p, "is_green"));
}

#[test]
fn runtime_index_out_of_range_faults() {
    let mut p =
        build("PROGRAM t VAR a : ARRAY[0..3] OF INT; i : INT := 4; END_VAR\n a[i] := 1;\nEND_PROGRAM");
    let mut img = Image::new(SIZES);
    let f = p.scan_at(&mut img, Duration::ZERO).unwrap_err();
    assert_eq!((f.kind, f.line), (FaultKind::IndexOutOfRange, 2));
    let e = errors(
        "PROGRAM t VAR a : ARRAY[0..3] OF INT; END_VAR a[4] := 1; a[1, 2] := 0; a[1.5] := 0; END_PROGRAM",
    );
    assert!(e.contains("index 4 is outside 0..3"), "{e}");
    assert!(e.contains("this array takes 1 index(es), got 2"), "{e}");
    assert!(e.contains("an array index must be an integer"), "{e}");
}

#[test]
fn type_errors() {
    let e = errors(
        "TYPE
           C : (A, B); D : (B, E);
           S : STRUCT x : INT; x : INT; END_STRUCT;
           Loop : STRUCT inner : Loop; END_STRUCT;
           Bad : STRUCT f : TON; END_STRUCT;
         END_TYPE
         PROGRAM t VAR c : C; d : D; i : INT; s : STRING; p : ARRAY[0..1] OF INT := [1, 2, 3]; END_VAR
           c := 1;
           c := d;
           i := c;
           IF c = d THEN END_IF;
           IF c < A THEN END_IF;
           CASE c OF 1: ; END_CASE;
           c := B;
         END_PROGRAM",
    );
    for want in [
        "field X appears twice",
        "LOOP contains itself",
        "a STRUCT cannot hold a function block instance",
        "unknown type STRING (STRING is not supported yet)",
        "3 initial values for 2 elements",
        "a number cannot be a C",
        "expected C, found D",
        "expected INT, found C",
        "= cannot compare C and D",
        "< cannot compare C and C",
        "an enumeration CASE takes value names as labels",
        "B is a value of several enumerations; write Type#B",
    ] {
        assert!(e.contains(want), "missing {want:?} in\n{e}");
    }
}

// ---- TIME ----

#[test]
fn time_arithmetic_and_conversions() {
    let mut p = build(
        "PROGRAM t VAR a, b, c, d, e, n : TIME; ms : DINT; r : LREAL; lt : BOOL; END_VAR
           a := T#1h30m + T#15s;
           b := a - T#1h;
           c := T#250ms * 4;
           d := T#1s / 3;
           e := 2.5 * T#2s;
           n := -T#5s;
           ms := TIME_TO_DINT(T#1.5s);
           r := TIME_TO_LREAL(T#1ms) ;
           lt := T#59s < T#1m;
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "a"), (90 * 60 + 15) * 1000 * MS);
    assert_eq!(int(&p, "b"), (30 * 60 + 15) * 1000 * MS);
    assert_eq!(int(&p, "c"), 1000 * MS);
    assert_eq!(int(&p, "d"), 333_333_333, "TIME / n rounds to the nanosecond");
    assert_eq!(int(&p, "e"), 5000 * MS);
    assert_eq!(int(&p, "n"), -5000 * MS);
    assert_eq!(int(&p, "ms"), 1500, "TIME converts to numbers in milliseconds");
    assert_eq!(p.get("r"), Some(Value::Real(1.0)));
    assert!(boolean(&p, "lt"));

    let e = errors(
        "PROGRAM t VAR a : TIME; i : DINT; END_VAR a := a + 1; a := a * a; i := a; a := T#1s / 0; END_PROGRAM",
    );
    assert!(e.contains("integer literal 1 cannot be a TIME"), "{e}");
    assert!(e.contains("* needs a TIME and a number"), "{e}");
    assert!(e.contains("expected DINT, found TIME"), "{e}");
    let mut p = build("PROGRAM t VAR a : TIME; z : LREAL; END_VAR\n a := T#1s / z;\nEND_PROGRAM");
    let mut img = Image::new(SIZES);
    assert_eq!(
        p.scan_at(&mut img, Duration::ZERO).unwrap_err().kind,
        FaultKind::DivisionByZero
    );
}

// ---- partial bit access ----

#[test]
fn bit_access() {
    let mut p = build(
        "PROGRAM t VAR w : WORD := 16#00F0; i : INT := -1; b0, b4, top : BOOL; o AT %QB0 : BYTE; END_VAR
           b0 := w.0; b4 := w.4;
           w.0 := TRUE; w.4 := FALSE; w.15 := TRUE;
           i.15 := FALSE;
           top := i.14;
           o.7 := TRUE; o.1 := TRUE;
         END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::ZERO).unwrap();
    assert!(!boolean(&p, "b0"));
    assert!(boolean(&p, "b4"));
    assert_eq!(int(&p, "w"), 0x80E1);
    assert_eq!(int(&p, "i"), 0x7FFF, "clearing the sign bit of INT -1");
    assert!(boolean(&p, "top"));
    assert_eq!(img.q[0], 0x82, "bit writes to a located BYTE");
    let e = errors(
        "PROGRAM t VAR w : WORD; r : REAL; b : BOOL; END_VAR b := w.16; b := r.0; b := b.0; END_PROGRAM",
    );
    assert!(e.contains("WORD has bits 0..15"), "{e}");
    assert!(
        e.contains("bit access needs an integer or bit string, found REAL"),
        "{e}"
    );
    assert!(e.contains("found BOOL"), "{e}");
}

// ---- standard functions ----

#[test]
fn standard_functions() {
    let mut p = build(
        "PROGRAM t
           VAR a, b, c, d, e, k : INT; r, s, u : LREAL; x : BYTE; y : WORD; tr : DINT; ok : BOOL; m : REAL; END_VAR
           a := ABS(INT#-5);
           b := MIN(7, 3, 9);
           c := MAX(a, b, -1);
           d := LIMIT(0, 150, 100);
           e := SEL(TRUE, 1, 2);
           k := MUX(2, 10, 20, 30);
           r := SQRT(16.0);
           s := EXPT(2.0, 10);
           u := ATAN(1.0) * 4.0;
           x := SHL(BYTE#2#1000_0001, 1);
           y := ROR(WORD#16#0001, 1);
           tr := TRUNC(-2.7);
           m := MOVE(REAL#1.5);
           ok := LN(EXP(1.0)) = 1.0 AND LOG(1000.0) = 3.0;
         END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(int(&p, "a"), 5);
    assert_eq!(int(&p, "b"), 3);
    assert_eq!(int(&p, "c"), 5);
    assert_eq!(int(&p, "d"), 100);
    assert_eq!(int(&p, "e"), 2, "SEL(G, IN0, IN1) picks IN1 when G");
    assert_eq!(int(&p, "k"), 30);
    assert_eq!(p.get("r"), Some(Value::Real(4.0)));
    assert_eq!(p.get("s"), Some(Value::Real(1024.0)));
    assert_eq!(p.get("u"), Some(Value::Real(std::f64::consts::PI)));
    assert_eq!(int(&p, "x"), 2);
    assert_eq!(int(&p, "y"), 0x8000);
    assert_eq!(int(&p, "tr"), -2);
    assert_eq!(p.get("m"), Some(Value::Real(1.5)));
    assert!(boolean(&p, "ok"));

    let mut p = build("PROGRAM t VAR k : INT := 3; v : INT; END_VAR\n v := MUX(k, 1, 2);\nEND_PROGRAM");
    let mut img = Image::new(SIZES);
    assert_eq!(
        p.scan_at(&mut img, Duration::ZERO).unwrap_err().kind,
        FaultKind::IndexOutOfRange
    );

    let e = errors(
        "PROGRAM t VAR i : INT; r : REAL; b : BOOL; END_VAR
           r := SQRT(i); i := SHL(i, 1); i := MIN(1); i := LIMIT(1, 2); i := ABS(b); i := NOPE(1);
           i := ABS(x := 1);
         END_PROGRAM",
    );
    for want in [
        "SQRT needs REAL or LREAL, found INT",
        "SHL needs a bit string",
        "MIN takes 2 argument(s), got 1",
        "LIMIT takes 3 argument(s), got 2",
        "ABS needs a number, found BOOL",
        "unknown function NOPE",
        "ABS takes positional arguments only",
    ] {
        assert!(e.contains(want), "missing {want:?} in\n{e}");
    }
}

#[test]
fn the_library_clock_is_not_user_callable() {
    let e = errors("PROGRAM t VAR x : TIME; END_VAR x := __NOW(); END_PROGRAM");
    assert!(e.contains("'__' is not allowed in names"), "{e}");
}

proptest! {
    /// Shifts and rotates on every bit-string width match Rust's.
    #[test]
    fn shifts_and_rotates_match_rust(x: u64, n in 0u32..80, which in 0usize..4, width in 0usize..4) {
        let (ty, bits) = [("BYTE", 8u32), ("WORD", 16), ("DWORD", 32), ("LWORD", 64)][width];
        let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        let x = x & mask;
        let (f, want) = match which {
            0 => ("SHL", if n >= bits { 0 } else { (x << n) & mask }),
            1 => ("SHR", if n >= bits { 0 } else { x >> n }),
            2 => ("ROL", match bits {
                8 => u64::from(u8::try_from(x).unwrap().rotate_left(n)),
                16 => u64::from(u16::try_from(x).unwrap().rotate_left(n)),
                32 => u64::from(u32::try_from(x).unwrap().rotate_left(n)),
                _ => x.rotate_left(n),
            }),
            _ => ("ROR", match bits {
                8 => u64::from(u8::try_from(x).unwrap().rotate_right(n)),
                16 => u64::from(u16::try_from(x).unwrap().rotate_right(n)),
                32 => u64::from(u32::try_from(x).unwrap().rotate_right(n)),
                _ => x.rotate_right(n),
            }),
        };
        let src = format!("PROGRAM t VAR x : {ty} := 16#{x:X}; r : {ty}; END_VAR r := {f}(x, {n}); END_PROGRAM");
        let mut p = build(&src);
        scan(&mut p, 0);
        prop_assert_eq!(int(&p, "r"), i128::from(want), "{}", src);
    }

    /// TON against its definition: Q exactly when IN has been TRUE for at
    /// least PT without a break (measured from the first scan that saw it),
    /// ET = min(elapsed, PT) while IN, 0 otherwise.
    #[test]
    fn ton_matches_its_definition(steps in proptest::collection::vec((any::<bool>(), 0u64..40), 1..120), pt in 0u64..200) {
        let mut p = build(&format!(
            "PROGRAM t VAR t : TON; go AT %IX0.0 : BOOL; END_VAR t(IN := go, PT := T#{pt}ms); END_PROGRAM"
        ));
        let mut img = Image::new(SIZES);
        let (mut now, mut since) = (0u64, None::<u64>);
        for (inp, dt) in steps {
            now += dt;
            img.i[0] = u8::from(inp);
            p.scan_at(&mut img, Duration::from_millis(now)).unwrap();
            since = if inp { since.or(Some(now)) } else { None };
            let elapsed = since.map(|s| now - s);
            let q = elapsed.is_some_and(|e| e >= pt);
            let et = elapsed.map_or(0, |e| e.min(pt));
            prop_assert_eq!(boolean(&p, "t.Q"), q, "now {} in {}", now, inp);
            prop_assert_eq!(int(&p, "t.ET"), i128::from(et) * MS, "now {}", now);
        }
    }

    /// TOF: Q while IN, and for PT after IN falls (measured from the first
    /// scan that saw it low).
    #[test]
    fn tof_matches_its_definition(steps in proptest::collection::vec((any::<bool>(), 0u64..40), 1..120), pt in 1u64..200) {
        let mut p = build(&format!(
            "PROGRAM t VAR t : TOF; go AT %IX0.0 : BOOL; END_VAR t(IN := go, PT := T#{pt}ms); END_PROGRAM"
        ));
        let mut img = Image::new(SIZES);
        let (mut now, mut fell, mut ever_on) = (0u64, None::<u64>, false);
        for (inp, dt) in steps {
            now += dt;
            img.i[0] = u8::from(inp);
            p.scan_at(&mut img, Duration::from_millis(now)).unwrap();
            if inp {
                ever_on = true;
                fell = None;
            } else if ever_on {
                fell = fell.or(Some(now));
            }
            let q = inp || fell.is_some_and(|f| now - f < pt);
            prop_assert_eq!(boolean(&p, "t.Q"), q, "now {} in {}", now, inp);
        }
    }

    /// CTU counts rising edges of CU since the last reset, saturating at
    /// 32767; Q = CV >= PV.
    #[test]
    fn ctu_counts_rising_edges(steps in proptest::collection::vec((any::<bool>(), any::<bool>()), 1..200), pv in -5i16..50) {
        let mut p = build(&format!(
            "PROGRAM t VAR c : CTU; cu AT %IX0.0 : BOOL; r AT %IX0.1 : BOOL; END_VAR c(CU := cu, R := r, PV := {pv}); END_PROGRAM"
        ));
        let mut img = Image::new(SIZES);
        let (mut prev, mut cv) = (false, 0i128);
        for (k, (cu, r)) in steps.into_iter().enumerate() {
            img.i[0] = u8::from(cu) | (u8::from(r) << 1);
            p.scan_at(&mut img, Duration::from_millis(u64::try_from(k).unwrap())).unwrap();
            if r {
                cv = 0;
            } else if cu && !prev {
                cv = (cv + 1).min(32767);
            }
            prev = cu;
            prop_assert_eq!(int(&p, "c.CV"), cv);
            prop_assert_eq!(boolean(&p, "c.Q"), cv >= i128::from(pv));
        }
    }
}

// ---- standard function blocks, scripted ----

#[test]
fn tp_pulse_ignores_retrigger() {
    let mut p =
        build("PROGRAM t VAR t : TP; go AT %IX0.0 : BOOL; END_VAR t(IN := go, PT := T#100ms); END_PROGRAM");
    let mut img = Image::new(SIZES);
    let mut at = |ms: u64, inp: bool, p: &mut Program| {
        img.i[0] = u8::from(inp);
        p.scan_at(&mut img, Duration::from_millis(ms)).unwrap();
        (boolean(p, "t.Q"), int(p, "t.ET") / MS)
    };
    assert_eq!(at(0, true, &mut p), (true, 0));
    assert_eq!(
        at(50, false, &mut p),
        (true, 50),
        "the pulse runs on after IN falls"
    );
    assert_eq!(
        at(60, true, &mut p),
        (true, 60),
        "a new edge during the pulse is ignored"
    );
    assert_eq!(
        at(120, true, &mut p),
        (false, 100),
        "ET holds PT while IN stays high"
    );
    assert_eq!(at(130, false, &mut p), (false, 0));
    assert_eq!(
        at(140, true, &mut p),
        (true, 0),
        "a fresh edge starts a new pulse"
    );
}

#[test]
fn edges_latches_and_down_counters() {
    let mut p = build(
        "PROGRAM t
           VAR
             x AT %IX0.0 : BOOL; y AT %IX0.1 : BOOL;
             re : R_TRIG; fe : F_TRIG; sr : SR; rs : RS; cd : CTD; ud : CTUD;
           END_VAR
           re(CLK := x); fe(CLK := x);
           sr(S1 := x, R := y); rs(S := x, R1 := y);
           cd(CD := x, LD := y, PV := 2);
           ud(CU := x, CD := y, PV := 2);
         END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    let mut step = |bits: u8, p: &mut Program| {
        img.i[0] = bits;
        p.scan_at(&mut img, Duration::ZERO).unwrap();
    };
    step(0, &mut p);
    assert!(
        !boolean(&p, "re.Q") && !boolean(&p, "fe.Q"),
        "no edge on the first scan"
    );
    step(1, &mut p);
    assert!(boolean(&p, "re.Q") && !boolean(&p, "fe.Q"));
    assert!(boolean(&p, "sr.Q1") && boolean(&p, "rs.Q1"));
    step(1, &mut p);
    assert!(!boolean(&p, "re.Q"), "R_TRIG is one scan long");
    step(3, &mut p);
    assert!(boolean(&p, "sr.Q1"), "SR: set dominates");
    assert!(!boolean(&p, "rs.Q1"), "RS: reset dominates");
    assert_eq!(int(&p, "cd.CV"), 2, "CTD loaded with PV");
    step(0, &mut p);
    assert!(boolean(&p, "fe.Q"));
    step(1, &mut p);
    step(0, &mut p);
    step(1, &mut p);
    assert_eq!(int(&p, "cd.CV"), 0);
    assert!(boolean(&p, "cd.Q"));
    assert_eq!(
        int(&p, "ud.CV"),
        2,
        "CTUD: up three times (one with LD-less CD edge), down once"
    );
    assert!(boolean(&p, "ud.QU"));
}

// ---- BLINK_PATTERN (a Helmsman extension, not IEC 61131-3) ----

const OFF: i32 = 0;
const STEADY: i32 = 1;
const SLOW: i32 = 2;
const FAST: i32 = 3;
const DOUBLE: i32 = 4;
const NS_PER_MS: u64 = 1_000_000;

/// A program around one `BLINK_PATTERN`, driven by the image like a
/// module's inputs: EN is %IX0.0, MODE is %IB1 - 2 (so a byte reaches the
/// modes below 0 too), PERIOD is `period` (ST, may read `alt` at %IX0.1).
struct Light {
    p: Program,
    img: Image,
}

impl Light {
    fn new(period: &str) -> Self {
        Self {
            p: build(&format!(
                "PROGRAM t
                   VAR b : BLINK_PATTERN; en AT %IX0.0 : BOOL; alt AT %IX0.1 : BOOL; m AT %IB1 : BYTE; END_VAR
                   b(EN := en, MODE := BYTE_TO_INT(m) - 2, PERIOD := {period});
                 END_PROGRAM"
            )),
            img: Image::new(SIZES),
        }
    }

    /// One scan at `ns` on the scan clock with these inputs; Q after it.
    fn at_ns(&mut self, ns: u64, en: bool, mode: i32) -> bool {
        self.img.i[0] = (self.img.i[0] & !1) | u8::from(en);
        self.img.i[1] = u8::try_from(mode + 2).unwrap();
        self.p.scan_at(&mut self.img, Duration::from_nanos(ns)).unwrap();
        boolean(&self.p, "b.Q")
    }

    fn at(&mut self, ms: u64, en: bool, mode: i32) -> bool {
        self.at_ns(ms * NS_PER_MS, en, mode)
    }
}

/// A PERIOD of `ms` milliseconds as an ST literal (negative too).
fn period_ms(ms: i64) -> String {
    format!("T#{ms}ms")
}

/// `BLINK_PATTERN` by its definition, for a fixed PERIOD of `period` ns:
/// Q at `now`, with `run` the pattern's start and mode while it blinks.
fn blink_def(en: bool, mode: i32, period: i64, now: i64, run: &mut Option<(i64, i32)>) -> bool {
    if !en || !(STEADY..=DOUBLE).contains(&mode) {
        *run = None;
        return false;
    }
    if mode == STEADY {
        *run = None;
        return true;
    }
    let cycle = if mode == FAST { period / 4 } else { period };
    let step = if mode == DOUBLE { cycle / 8 } else { cycle / 2 };
    if step <= 0 {
        *run = None;
        return true;
    }
    if run.is_none_or(|(_, m)| m != mode) {
        *run = Some((now, mode));
    }
    let phase = (now - run.unwrap().0) % cycle;
    if mode == DOUBLE {
        phase < step || (2 * step..3 * step).contains(&phase)
    } else {
        phase < step
    }
}

proptest! {
    /// Against its definition, scan by scan: EN, MODE (in range and out)
    /// and the gaps between scans at random, PERIOD from negative through
    /// zero to longer than most gaps.
    #[test]
    fn blink_pattern_matches_its_definition(
        steps in proptest::collection::vec((prop::bool::weighted(0.85), -2i32..7, 0u64..700), 1..150),
        period in -100i64..1200,
    ) {
        let mut l = Light::new(&period_ms(period));
        let (mut now, mut run) = (0u64, None);
        for (en, mode, dt) in steps {
            now += dt;
            let q = l.at(now, en, mode);
            let want = blink_def(en, mode, period * 1_000_000, i64::try_from(now * NS_PER_MS).unwrap(), &mut run);
            prop_assert_eq!(q, want, "now {}ms en {} mode {} period {}ms", now, en, mode, period);
        }
    }

    /// Any PERIOD to the nanosecond, any gap: the block never faults (no
    /// division by zero, no runaway), EN FALSE is always off, steady always
    /// on, and a blinking mode without a usable PERIOD is steady on.
    #[test]
    fn blink_pattern_never_faults_and_keeps_its_safe_rules(
        steps in proptest::collection::vec((any::<bool>(), -2i32..7, 0u64..(1 << 50)), 1..60),
        period in prop_oneof![-(1i64 << 60)..(1i64 << 60), -10i64..10],
    ) {
        let mut l = Light::new(&format!("T#{period}ns"));
        let mut now = 0u64;
        for (en, mode, dt) in steps {
            now += dt;
            let q = l.at_ns(now, en, mode);
            if !en || !(STEADY..=DOUBLE).contains(&mode) {
                prop_assert!(!q);
            } else if mode == STEADY || (period <= 0) {
                prop_assert!(q);
            }
        }
    }
}

#[test]
fn blink_pattern_edges_and_duty_cycles() {
    // PERIOD 1 s, scanned every millisecond for three cycles: where each
    // pattern switches, and its share of on time.
    let ons = |mode: i32| {
        let mut l = Light::new("T#1s");
        (0..3000).filter(|&ms| l.at(ms, true, mode)).collect::<Vec<_>>()
    };
    let windows = |ons: &[u64]| {
        let mut w: Vec<(u64, u64)> = Vec::new();
        for &ms in ons {
            match w.last_mut() {
                Some((_, end)) if *end == ms => *end = ms + 1,
                _ => w.push((ms, ms + 1)),
            }
        }
        w
    };
    let slow = ons(SLOW);
    assert_eq!(slow.len(), 1500, "50 % duty");
    assert_eq!(windows(&slow), [(0, 500), (1000, 1500), (2000, 2500)]);
    let fast = ons(FAST);
    assert_eq!(fast.len(), 1500, "50 % duty, four times as often");
    assert_eq!(windows(&fast).len(), 12);
    assert_eq!(windows(&fast)[..3], [(0, 125), (250, 375), (500, 625)]);
    let double = ons(DOUBLE);
    assert_eq!(double.len(), 750, "two eighths of each period");
    assert_eq!(
        windows(&double),
        [
            (0, 125),
            (250, 375),
            (1000, 1125),
            (1250, 1375),
            (2000, 2125),
            (2250, 2375)
        ]
    );
    assert_eq!(ons(STEADY).len(), 3000);
    assert_eq!(ons(OFF).len(), 0);
    for bad in [-2, 5, 6] {
        assert_eq!(ons(bad).len(), 0, "MODE {bad} is off");
    }

    // Just either side of an edge, to the nanosecond.
    let mut l = Light::new("T#1s");
    assert!(l.at_ns(0, true, SLOW));
    assert!(l.at_ns(500 * NS_PER_MS - 1, true, SLOW));
    assert!(!l.at_ns(500 * NS_PER_MS, true, SLOW));
    assert!(!l.at_ns(1000 * NS_PER_MS - 1, true, SLOW));
    assert!(l.at_ns(1000 * NS_PER_MS, true, SLOW));
}

#[test]
fn blink_pattern_restarts_on_a_mode_change_or_a_rising_en() {
    let mut l = Light::new("T#1s");
    assert!(l.at(0, true, SLOW));
    assert!(l.at(300, true, SLOW));
    // Mid-cycle, still on: fast starts over, on at once, off 125 ms later.
    assert!(l.at(300, true, FAST));
    assert!(l.at(424, true, FAST));
    assert!(!l.at(425, true, FAST));
    assert!(l.at(550, true, FAST));
    // In fast's off half: slow starts over on, for its full half period.
    assert!(!l.at(680, true, FAST));
    assert!(l.at(680, true, SLOW));
    assert!(l.at(1179, true, SLOW));
    assert!(!l.at(1180, true, SLOW));
    // Double flash from slow's off half.
    assert!(l.at(1300, true, DOUBLE));
    assert!(!l.at(1430, true, DOUBLE));
    assert!(l.at(1560, true, DOUBLE));
    assert!(!l.at(1700, true, DOUBLE));
    // Steady and back: the pattern starts over.
    assert!(l.at(1800, true, STEADY));
    assert!(l.at(1900, true, DOUBLE));
    assert!(!l.at(2030, true, DOUBLE));

    // EN FALSE forces Q off in every mode; EN back starts the pattern over.
    for mode in [STEADY, SLOW, FAST, DOUBLE] {
        assert!(!l.at(3000, false, mode), "mode {mode}");
    }
    assert!(!l.at(3700, false, SLOW));
    assert!(l.at(3700, true, SLOW), "on at once, not where the old cycle was");
    assert!(l.at(4199, true, SLOW));
    assert!(!l.at(4200, true, SLOW));
}

#[test]
fn blink_pattern_with_no_usable_period_is_steady_on() {
    // PERIOD 0 or negative: every blinking mode is steady on, scan after
    // scan, and the off modes stay off.
    for period in ["T#0s", "T#-1s", "T#-1ns"] {
        let mut l = Light::new(period);
        for ms in [0, 1, 7, 500, 10_000] {
            for mode in [SLOW, FAST, DOUBLE, STEADY] {
                assert!(l.at(ms, true, mode), "{period} mode {mode} at {ms}");
            }
            assert!(!l.at(ms, true, OFF), "{period}");
            assert!(!l.at(ms, false, SLOW), "{period}");
        }
    }
    // Too short to divide into its steps is the same: 3 ns has no eighth (it rounds to 0).
    let mut l = Light::new("T#3ns");
    for ns in [0, 1, 3, 5, 100] {
        assert!(l.at_ns(ns, true, DOUBLE), "{ns}");
    }
}

#[test]
fn blink_pattern_keeps_its_phase_across_gaps_and_period_changes() {
    // A long scan gap lands where the pattern would be, even a hundred
    // days of 3 ms cycles later (a quotient past what a DINT holds).
    let mut l = Light::new("T#3ms");
    assert!(l.at(0, true, SLOW));
    let days = 100 * 24 * 3_600_000;
    assert_eq!(days % 3, 0);
    assert!(days / 3 > u64::try_from(i32::MAX).unwrap());
    assert!(l.at(days + 1, true, SLOW), "1 ms into a cycle: on");
    assert!(!l.at(days + 2, true, SLOW), "2 ms in: off");
    assert!(l.at(days + 3, true, SLOW));
    let mut l = Light::new("T#1s");
    assert!(l.at(0, true, DOUBLE));
    assert!(
        l.at(10_260, true, DOUBLE),
        "10 cycles and 260 ms: the second flash"
    );
    assert!(!l.at(10_400, true, DOUBLE));

    // A PERIOD change keeps the phase, within the new cycle.
    let mut l = Light::new("SEL(alt, T#1s, T#400ms)");
    assert!(l.at(0, true, SLOW));
    assert!(!l.at(580, true, SLOW), "1 s: off from 500 ms");
    l.img.i[0] |= 2;
    assert!(
        l.at(590, true, SLOW),
        "400 ms now: 590 ms of phase is 190 ms into a cycle, on"
    );
    assert!(!l.at(610, true, SLOW), "210 ms in: off");
    assert!(l.at(800, true, SLOW), "a new 400 ms cycle");
    assert!(!l.at(1000, true, SLOW));
}

#[test]
fn standard_names_are_reserved() {
    let e = errors("FUNCTION_BLOCK TON END_FUNCTION_BLOCK PROGRAM t END_PROGRAM");
    assert!(e.contains("TON is already defined (a standard block)"), "{e}");
    let e = errors("FUNCTION_BLOCK blink_pattern END_FUNCTION_BLOCK PROGRAM t END_PROGRAM");
    assert!(e.contains("already defined (a standard block)"), "{e}");
    // The stack-light block is not called BLINK so a module named blink
    // (the playground's first example is one) still compiles.
    build("PROGRAM blink VAR blink_pattern_1 : BLINK_PATTERN; END_VAR END_PROGRAM");
    build("FUNCTION_BLOCK blink END_FUNCTION_BLOCK PROGRAM t VAR b : blink; END_VAR b(); END_PROGRAM");
    let e = errors("FUNCTION_BLOCK a END_FUNCTION_BLOCK");
    assert!(e.contains("no PROGRAM found"), "{e}");
    let e = errors("PROGRAM a END_PROGRAM PROGRAM b END_PROGRAM");
    assert!(e.contains("only one PROGRAM is supported"), "{e}");
}

#[test]
fn get_paths() {
    let mut p = build(
        "TYPE P : STRUCT v : ARRAY[0..1] OF INT; END_STRUCT; END_TYPE
         PROGRAM t VAR a : ARRAY[0..1] OF P; t : TON; END_VAR a[1].v[1] := 7; END_PROGRAM",
    );
    scan(&mut p, 0);
    assert_eq!(p.get("a[1].v[1]"), Some(Value::Int(7)));
    assert_eq!(p.get("A[1].V[0]"), Some(Value::Int(0)));
    assert_eq!(
        p.get("t.running"),
        Some(Value::Bool(false)),
        "FB internals are visible to get()"
    );
    for bad in [
        "a[2].v[0]",
        "a",
        "a[1]",
        "nope",
        "t.nope",
        "a[1].v",
        "a[x]",
        "a[1",
    ] {
        assert_eq!(p.get(bad), None, "{bad}");
    }
}

proptest! {
    /// Token soup over functions, FBs, arrays, members, TIME and TYPE never
    /// panics the compiler, and whatever compiles scans without panicking.
    #[test]
    fn unit_token_soup_never_panics(words in proptest::collection::vec(prop_oneof![
        Just("x"), Just("a"), Just("t"), Just("f"), Just(":="), Just("=>"), Just(";"), Just("+"), Just("*"),
        Just("("), Just(")"), Just("["), Just("]"), Just("."), Just(","), Just(".."), Just(":"), Just("0"),
        Just("1"), Just("3"), Just("-1"), Just("T#5ms"), Just("TRUE"), Just("t.Q"), Just("t.ET"), Just("IN"),
        Just("PT"), Just("a[x]"), Just("x.3"), Just("IF"), Just("THEN"), Just("END_IF"), Just("FOR"),
        Just("TO"), Just("DO"), Just("END_FOR"), Just("FUNCTION"), Just("END_FUNCTION"),
        Just("FUNCTION_BLOCK"), Just("END_FUNCTION_BLOCK"), Just("VAR_INPUT"), Just("VAR"), Just("END_VAR"),
        Just("TYPE"), Just("END_TYPE"), Just("STRUCT"), Just("END_STRUCT"), Just("ARRAY"), Just("OF"),
        Just("INT"), Just("TON"), Just("MUX"), Just("LIMIT"), Just("Color#Red"), Just("CASE"), Just("END_CASE"),
    ], 0..50)) {
        let body = words.join(" ");
        for src in [
            format!("PROGRAM p VAR x : INT; a : ARRAY[0..3] OF INT; t : TON; END_VAR {body} END_PROGRAM"),
            format!("{body} PROGRAM p END_PROGRAM"),
        ] {
            if let Ok(mut p) = compile(&src, SIZES) {
                p.set_budget(10_000);
                let mut img = Image::new(SIZES);
                let _ = p.scan_at(&mut img, Duration::from_millis(1));
            }
        }
    }
}
