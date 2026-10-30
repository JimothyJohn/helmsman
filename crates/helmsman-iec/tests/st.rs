//! Structured Text through the public API: compile, scan, read the image.

use helmsman_iec::{FaultKind, Image, ImageSizes, Program, Value, compile};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

fn build(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{src}\n{d:#?}"))
}

/// Compiles `body` inside a PROGRAM with `vars` and runs one scan.
fn run(vars: &str, body: &str) -> (Program, Image) {
    let mut p = build(&format!("PROGRAM t\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM"));
    let mut img = Image::new(SIZES);
    p.scan(&mut img).unwrap();
    (p, img)
}

fn get(p: &Program, v: &str) -> Value {
    p.get(v).unwrap_or_else(|| panic!("no variable {v}"))
}

/// Compile errors, as "line:col: message".
fn errors(src: &str) -> Vec<String> {
    match compile(src, SIZES) {
        Ok(_) => panic!("compiled: {src}"),
        Err(d) => d.iter().map(ToString::to_string).collect(),
    }
}

fn prog_errors(vars: &str, body: &str) -> String {
    errors(&format!("PROGRAM t\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM")).join("\n")
}

// ---- arithmetic against Rust's own fixed-width types ----

/// `r := a OP b` on type `ty` with a and b read from the input image and r
/// written to the output image.
macro_rules! int_matches_rust {
    ($name:ident, $ty:literal, $rust:ty, $size:literal) => {
        proptest! {
            #[test]
            fn $name(a: $rust, b: $rust, op in 0usize..5) {
                let (sym, want): (&str, Option<$rust>) = match op {
                    0 => ("+", Some(a.wrapping_add(b))),
                    1 => ("-", Some(a.wrapping_sub(b))),
                    2 => ("*", Some(a.wrapping_mul(b))),
                    3 => ("/", (b != 0).then(|| a.wrapping_div(b))),
                    _ => ("MOD", (b != 0).then(|| a.wrapping_rem(b))),
                };
                let src = format!(
                    "PROGRAM t VAR a AT %I{s}0 : {ty}; b AT %I{s}1 : {ty}; r AT %Q{s}0 : {ty}; END_VAR \
r := a {sym} b; END_PROGRAM",
                    s = $size, ty = $ty,
                );
                let mut p = build(&src);
                let mut img = Image::new(SIZES);
                let n = std::mem::size_of::<$rust>();
                img.i[..n].copy_from_slice(&a.to_le_bytes());
                img.i[n..2 * n].copy_from_slice(&b.to_le_bytes());
                match want {
                    Some(w) => {
                        p.scan(&mut img).unwrap();
                        prop_assert_eq!(&img.q[..n], &w.to_le_bytes()[..], "{} {} {} {}", a, sym, b, $ty);
                    }
                    None => {
                        let f = p.scan(&mut img).unwrap_err();
                        prop_assert_eq!(f.kind, FaultKind::DivisionByZero);
                    }
                }
            }
        }
    };
}

int_matches_rust!(sint_ops, "SINT", i8, "B");
int_matches_rust!(int_ops, "INT", i16, "W");
int_matches_rust!(dint_ops, "DINT", i32, "D");
int_matches_rust!(lint_ops, "LINT", i64, "L");
int_matches_rust!(usint_ops, "USINT", u8, "B");
int_matches_rust!(uint_ops, "UINT", u16, "W");
int_matches_rust!(udint_ops, "UDINT", u32, "D");
int_matches_rust!(ulint_ops, "ULINT", u64, "L");

proptest! {
    /// REAL is IEEE single precision: each operation rounds to f32.
    #[test]
    fn real_ops_match_f32(a in any::<f32>().prop_filter("finite", |x| x.is_finite()),
                          b in any::<f32>().prop_filter("finite", |x| x.is_finite()),
                          op in 0usize..4) {
        let (sym, want) = match op {
            0 => ("+", a + b),
            1 => ("-", a - b),
            2 => ("*", a * b),
            _ => ("/", a / b),
        };
        let src = format!(
            "PROGRAM t VAR a AT %ID0 : REAL; b AT %ID1 : REAL; r AT %QD0 : REAL; END_VAR r := a {sym} b; END_PROGRAM"
        );
        let mut p = build(&src);
        let mut img = Image::new(SIZES);
        img.i[..4].copy_from_slice(&a.to_le_bytes());
        img.i[4..8].copy_from_slice(&b.to_le_bytes());
        p.scan(&mut img).unwrap();
        let got = f32::from_le_bytes(img.q[..4].try_into().unwrap());
        prop_assert!(got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()), "{a} {sym} {b}: {got} vs {want}");
    }

    /// Compiling arbitrary text never panics.
    #[test]
    fn arbitrary_text_never_panics(src in "\\PC{0,200}") {
        let _ = compile(&src, SIZES);
    }

    /// Token soup from the ST vocabulary never panics the compiler, and
    /// whatever compiles scans without panicking.
    #[test]
    fn token_soup_never_panics(words in proptest::collection::vec(prop_oneof![
        Just("x"), Just("y"), Just("b"), Just(":="), Just(";"), Just("+"), Just("-"), Just("*"), Just("/"),
        Just("MOD"), Just("**"), Just("AND"), Just("OR"), Just("NOT"), Just("="), Just("<"), Just("("), Just(")"),
        Just("IF"), Just("THEN"), Just("ELSE"), Just("END_IF"), Just("WHILE"), Just("DO"), Just("END_WHILE"),
        Just("FOR"), Just("TO"), Just("BY"), Just("END_FOR"), Just("EXIT"), Just("0"), Just("1"), Just("-1"),
        Just("127"), Just("2.5"), Just("TRUE"), Just("%QX0.0"), Just("%IW1"), Just("%MD2"), Just("INT_TO_SINT"),
        Just("CASE"), Just("OF"), Just(":"), Just(".."), Just(","), Just("END_CASE"),
    ], 0..40)) {
        let src = format!(
            "PROGRAM t VAR x, y : INT; b : BOOL; END_VAR {} END_PROGRAM",
            words.join(" ")
        );
        if let Ok(mut p) = compile(&src, SIZES) {
            p.set_budget(10_000);
            let mut img = Image::new(SIZES);
            let _ = p.scan(&mut img);
        }
    }
}

// ---- expressions ----

#[test]
fn precedence_and_integer_division() {
    let (p, _) = run(
        "a, b, c, d, e : DINT; r : LREAL;",
        "a := 10 - 3 - 2; b := -7 / 2; c := -7 MOD 2; d := 7 MOD -2; e := 2 + 3 * 4; r := -2.0 ** 2;",
    );
    assert_eq!(get(&p, "a"), Value::Int(5));
    assert_eq!(get(&p, "b"), Value::Int(-3), "division truncates toward zero");
    assert_eq!(get(&p, "c"), Value::Int(-1), "MOD takes the dividend's sign");
    assert_eq!(get(&p, "d"), Value::Int(1));
    assert_eq!(get(&p, "e"), Value::Int(14));
    assert_eq!(
        get(&p, "r"),
        Value::Real(4.0),
        "unary minus binds tighter than ** (as ruSTy)"
    );
}

#[test]
fn implicit_widening_and_literals() {
    let (p, _) = run(
        "s : SINT := -100; u : USINT := 200; i : INT; d : DINT; r : REAL; l : LREAL; w : WORD := 16#F0F0;",
        "i := s + u; d := i * 1000; r := i + 0.5; l := d; w := w AND 16#FF00;",
    );
    assert_eq!(get(&p, "i"), Value::Int(100), "SINT + USINT is computed in INT");
    // INT arithmetic is carried out DINT-wide (as ruSTy, CODESYS and
    // TwinCAT do), so the product survives the assignment to DINT.
    assert_eq!(get(&p, "d"), Value::Int(100_000));
    assert_eq!(get(&p, "r"), Value::Real(100.5));
    assert_eq!(get(&p, "l"), Value::Real(100_000.0));
    assert_eq!(get(&p, "w"), Value::Int(0xF000));
}

#[test]
fn conversions() {
    let (p, _) = run(
        "a, b, c : INT; d : SINT; e : UINT; f, g : BOOL; h : BYTE; r : REAL; t : DINT;",
        "a := REAL_TO_INT(2.5); b := REAL_TO_INT(-2.5); c := LREAL_TO_INT(1.0E9);
         d := INT_TO_SINT(300); e := DINT_TO_UINT(-1); f := INT_TO_BOOL(5); g := WORD_TO_BOOL(16#0);
         h := TO_BYTE(INT#-1); r := TO_REAL(INT#7); t := BOOL_TO_DINT(TRUE);",
    );
    assert_eq!(get(&p, "a"), Value::Int(3), "rounds half away from zero");
    assert_eq!(get(&p, "b"), Value::Int(-3));
    // REAL -> INT saturates at the range (as ruSTy) instead of wrapping.
    assert_eq!(get(&p, "c"), Value::Int(32_767), "out-of-range saturates");
    assert_eq!(get(&p, "d"), Value::Int(44));
    assert_eq!(get(&p, "e"), Value::Int(65535));
    assert_eq!(get(&p, "f"), Value::Bool(true));
    assert_eq!(get(&p, "g"), Value::Bool(false));
    assert_eq!(get(&p, "h"), Value::Int(255));
    assert_eq!(get(&p, "r"), Value::Real(7.0));
    assert_eq!(get(&p, "t"), Value::Int(1));
}

#[test]
fn logic_and_comparison() {
    let (p, _) = run(
        "a, b, c, d, e : BOOL; x : BYTE := 2#1010_0101;",
        "a := TRUE AND NOT FALSE; b := TRUE XOR TRUE; c := 3 < 4 = TRUE; d := x = 16#A5; e := (NOT x) = 16#5A;",
    );
    for (v, want) in [("a", true), ("b", false), ("c", true), ("d", true), ("e", true)] {
        assert_eq!(get(&p, v), Value::Bool(want), "{v}");
    }
}

// ---- statements ----

#[test]
fn control_flow() {
    let (p, _) = run(
        "i, n, sum, k, w, rpt : INT; c1, c2, c3 : INT; s : SINT; m : INT;",
        "FOR i := 1 TO 10 DO sum := sum + i; END_FOR;
         FOR n := 10 TO 1 BY -3 DO k := k + 1; END_FOR;
         WHILE w < 100 DO w := w + 7; IF w > 50 THEN EXIT; END_IF; END_WHILE;
         REPEAT rpt := rpt + 1; UNTIL rpt >= 3 END_REPEAT;
         FOR s := 120 TO 127 DO m := m + 1; END_FOR;
         CASE sum OF 1..54: c1 := 1; 55, 56: c1 := 2; ELSE c1 := 3; END_CASE;
         CASE k OF 1: c2 := 1; ELSE c2 := 9; END_CASE;
         CASE rpt OF 0: c3 := 1; 3: c3 := 2; END_CASE;",
    );
    assert_eq!(get(&p, "sum"), Value::Int(55));
    assert_eq!(
        get(&p, "i"),
        Value::Int(11),
        "FOR leaves the control variable one step past the end"
    );
    assert_eq!(get(&p, "k"), Value::Int(4), "10, 7, 4, 1");
    assert_eq!(get(&p, "w"), Value::Int(56));
    assert_eq!(get(&p, "rpt"), Value::Int(3));
    assert_eq!(
        get(&p, "m"),
        Value::Int(8),
        "a FOR up to the type's maximum terminates"
    );
    assert_eq!(get(&p, "c1"), Value::Int(2));
    assert_eq!(get(&p, "c2"), Value::Int(9));
    assert_eq!(get(&p, "c3"), Value::Int(2));
}

#[test]
fn continue_and_return() {
    let (p, _) = run(
        "i, odd, after : INT;",
        "FOR i := 1 TO 10 DO IF i MOD 2 = 0 THEN CONTINUE; END_IF; odd := odd + 1; END_FOR;
         RETURN;
         after := 1;",
    );
    assert_eq!(get(&p, "odd"), Value::Int(5));
    assert_eq!(get(&p, "after"), Value::Int(0));
}

#[test]
fn var_persists_temp_resets_reset_restores() {
    let mut p = build(
        "PROGRAM t VAR CONSTANT step : INT := 2; END_VAR VAR n : INT := 10; END_VAR VAR_TEMP t : INT := 5; END_VAR
         n := n + step; t := t + 1; END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    for _ in 0..3 {
        p.scan(&mut img).unwrap();
    }
    assert_eq!(p.get("n"), Some(Value::Int(16)));
    assert_eq!(
        p.get("T"),
        Some(Value::Int(6)),
        "VAR_TEMP starts from its initial value every scan"
    );
    p.reset();
    assert_eq!(p.get("n"), Some(Value::Int(10)));
}

// ---- process image ----

#[test]
fn located_variables_and_direct_addresses() {
    let mut p = build(
        "PROGRAM t VAR
           start AT %IX0.1 : BOOL; speed AT %IW1 : INT; lamp AT %QX0.3 : BOOL;
           out AT %QW1 : INT; temp AT %QD1 : REAL; big AT %QL2 : LINT; m AT %MB5 : USINT;
         END_VAR
           lamp := start;
           out := speed * 2;
           temp := 21.5;
           big := -1;
           m := m + 1;
           %QX0.0 := %IX0.0;
           %QB16 := %IB4;
         END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    img.i[0] = 0b10;
    img.i[2..4].copy_from_slice(&(-300i16).to_le_bytes());
    img.i[4] = 0xAB;
    img.q[0] = 0b1111_0000;
    p.scan(&mut img).unwrap();
    assert_eq!(img.q[0], 0b1111_1000, "bit writes leave neighbouring bits alone");
    assert_eq!(
        &img.q[2..4],
        &(-600i16).to_le_bytes(),
        "%QW1 is bytes 2..4, little-endian"
    );
    assert_eq!(&img.q[4..8], &21.5f32.to_le_bytes());
    assert_eq!(&img.q[17..24], &[0xFF; 7], "%QL2 is bytes 16..24");
    assert_eq!(img.q[16], 0xAB, "%QB16 := %IB4 runs after big := -1");
    p.scan(&mut img).unwrap();
    assert_eq!(img.m[5], 2, "%M persists in the image");
    img.i[4] = 0x12;
    p.scan(&mut img).unwrap();
    assert_eq!(img.q[16], 0x12);
    assert_eq!(p.get("lamp"), None, "located variables live in the image");
}

// ---- faults ----

#[test]
fn faults_carry_the_line_and_the_program_can_run_again() {
    let mut p = build("PROGRAM t VAR d, x : INT; END_VAR\n x := 1;\n x := 10 / d;\nEND_PROGRAM");
    let mut img = Image::new(SIZES);
    let f = p.scan(&mut img).unwrap_err();
    assert_eq!((f.kind, f.line), (FaultKind::DivisionByZero, 3));
    assert!(f.to_string().contains("line 3"));
    assert_eq!(p.scan(&mut img).unwrap_err().kind, FaultKind::DivisionByZero);

    let mut p =
        build("PROGRAM t VAR x : INT; END_VAR\n WHILE TRUE DO\n x := x + 1;\n END_WHILE;\nEND_PROGRAM");
    p.set_budget(1000);
    let f = p.scan(&mut img).unwrap_err();
    assert_eq!(f.kind, FaultKind::Watchdog);
    assert!((2..=3).contains(&f.line), "{f}");

    let mut p = build("PROGRAM t VAR i : INT; END_VAR FOR i := 1 TO 5 BY 0 DO ; END_FOR; END_PROGRAM");
    p.set_budget(1000);
    assert_eq!(
        p.scan(&mut img).unwrap_err().kind,
        FaultKind::Watchdog,
        "BY 0 never ends"
    );
}

// ---- compile errors ----

#[test]
fn type_errors_explain_the_fix() {
    let e = prog_errors("a : SINT; b : INT;", "a := b;");
    assert!(
        e.contains("expected SINT, found INT; convert explicitly with INT_TO_SINT"),
        "{e}"
    );
    let e = prog_errors("a : SINT;", "a := 200;");
    assert!(e.contains("200 does not fit SINT (-128..127)"), "{e}");
    let e = prog_errors("a : INT; w : WORD;", "a := a + w;");
    assert!(e.contains("no implicit conversion"), "{e}");
    let e = prog_errors("w : WORD;", "w := w + 1;");
    assert!(e.contains("+ needs a numeric type, found WORD"), "{e}");
    let e = prog_errors("a : INT;", "a := a AND 1;");
    assert!(e.contains("AND needs BOOL or a bit string"), "{e}");
    let e = prog_errors("r : REAL; i : INT;", "r := i ** 2;");
    assert!(e.contains("** needs a REAL or LREAL base"), "{e}");
    let e = prog_errors("u : UINT;", "u := -u;");
    assert!(e.contains("unary - needs a signed"), "{e}");
    let e = prog_errors("i : INT;", "IF i THEN i := 1; END_IF;");
    assert!(e.contains("expected BOOL, found INT"), "{e}");
    let e = prog_errors("r : REAL;", "CASE r OF 1: ; END_CASE;");
    assert!(e.contains("CASE selector must be an integer"), "{e}");
    let e = prog_errors("i : SINT;", "CASE i OF 1..5: ; 5: ; 300: ; END_CASE;");
    assert!(e.contains("overlaps") && e.contains("outside SINT"), "{e}");
    let e = prog_errors("x : INT;", "x := FOO(1);");
    assert!(e.contains("unknown function FOO"), "{e}");
}

#[test]
fn declaration_and_statement_errors() {
    let e = errors(
        "PROGRAM t
         VAR CONSTANT k : INT; END_VAR
         VAR a : INT; a : INT; z : FLOAT; i AT %IX0.0 : INT; q AT %QW99 : WORD; j AT %IW0 : INT := 1; END_VAR
         VAR_TEMP tt AT %MB0 : BYTE; END_VAR
         END_PROGRAM",
    )
    .join("\n");
    for want in [
        "CONSTANT K needs an initial value",
        "A is declared twice",
        "unknown type FLOAT",
        "%IX0.0 is 1 bits wide, INT is 16",
        "outside the 32-byte Q area",
        "J: an input (%I) cannot take an initial value",
        "TT: VAR_TEMP cannot be located",
    ] {
        assert!(e.contains(want), "missing {want:?} in\n{e}");
    }

    let e = errors(
        "PROGRAM t VAR CONSTANT k : INT := 1; END_VAR VAR i : INT; x AT %IX0.0 : BOOL; r : REAL; END_VAR
         k := 2; x := TRUE; %IW0 := 1; EXIT; y := 1; FOR r := 1 TO 2 DO ; END_FOR;
         END_PROGRAM",
    )
    .join("\n");
    for want in [
        "K is a CONSTANT",
        "inputs (%I) are read-only",
        "EXIT and CONTINUE are only allowed inside a loop",
        "undeclared variable Y",
        "FOR variable R must be a non-CONSTANT, non-located integer",
    ] {
        assert!(e.contains(want), "missing {want:?} in\n{e}");
    }
    assert_eq!(e.lines().filter(|l| l.contains("read-only")).count(), 2);
}

/// IEC 61131-3 `boolean_literal` is `[BOOL#] (1 | 0) | TRUE | FALSE`;
/// the `OpenPLC` Editor's `Random_Generator_Pragma` example writes
/// `M : BOOL := 0` and `IF M = 0 AND IN = 1`.
#[test]
fn zero_and_one_are_bool_literals() {
    let (p, _) = run(
        "a : BOOL := 1; b : BOOL := 0; c : BOOL; d : BOOL := 1;",
        "IF b = 0 AND a = 1 THEN c := 1; END_IF; d := 0;",
    );
    assert_eq!(get(&p, "a"), Value::Bool(true));
    assert_eq!(get(&p, "b"), Value::Bool(false));
    assert_eq!(get(&p, "c"), Value::Bool(true));
    assert_eq!(get(&p, "d"), Value::Bool(false));
    let e = prog_errors("b : BOOL := 2;", "");
    assert!(e.contains("integer literal 2 cannot be a BOOL"), "{e}");
    let e = prog_errors("b : BOOL;", "b := -1;");
    assert!(e.contains("cannot be a BOOL"), "{e}");
}

/// Features real editor exports use that this runtime lacks are named as
/// unsupported, not reported as syntax errors.
#[test]
fn unsupported_features_say_so() {
    for (vars, body, want) in [
        ("x : INT;", "x := 'abc';", "STRING literals are not supported yet"),
        (
            "x : INT;",
            "x := \"abc\";",
            "STRING literals are not supported yet",
        ),
        (
            "x : INT;",
            "x := DT#2013-02-23-22:35:46;",
            "DT literals are not supported yet",
        ),
        (
            "x : INT;",
            "x := TOD#12:00:00;",
            "TOD literals are not supported yet",
        ),
        (
            "x : INT;",
            "x := D#2020-01-01;",
            "D literals are not supported yet",
        ),
        (
            "x : INT;",
            "x := DATE_AND_TIME#2020-01-01-00:00:00;",
            "DATE_AND_TIME literals are not supported yet",
        ),
        ("d : DT;", "", "unknown type DT (DT is not supported yet)"),
        ("d : TOD;", "", "unknown type TOD (TOD is not supported yet)"),
        ("d : DATE;", "", "unknown type DATE (DATE is not supported yet)"),
        (
            "w AT %QW0.1.8192.0 : WORD;",
            "",
            "%QW0.1.8192.0: multi-level addresses (as Beremiz extensions assign) are not supported",
        ),
        (
            "x AT %QX0.4.2 : BOOL;",
            "",
            "%QX0.4.2: multi-level addresses (as Beremiz extensions assign) are not supported",
        ),
    ] {
        let e = prog_errors(vars, body);
        assert!(e.contains(want), "{vars} {body}: want {want:?}, got\n{e}");
    }
}

/// IEC 61131-3 lets a located variable take an initial value; the
/// `OpenPLC` Editor's `Traffic_Light_FBD` example starts with
/// `First_cycle AT %QX0.3 : BOOL := TRUE`. It is written to the image
/// before the first scan, and again after a reset, never in between.
#[test]
fn located_initial_values_are_written_at_a_cold_start() {
    let mut p = build(
        "PROGRAM t VAR first AT %QX0.3 : BOOL := TRUE; w AT %QW2 : INT := -2; m AT %MB1 : BYTE := 16#5A;
         seen : BOOL; END_VAR
         seen := first; first := FALSE;
         END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    p.scan(&mut img).unwrap();
    assert_eq!(
        get(&p, "seen"),
        Value::Bool(true),
        "first scan sees the initial value"
    );
    assert_eq!(img.q[0] & 0b1000, 0, "the body cleared it");
    assert_eq!(&img.q[4..6], &(-2i16).to_le_bytes());
    assert_eq!(img.m[1], 0x5A);
    p.scan(&mut img).unwrap();
    assert_eq!(
        get(&p, "seen"),
        Value::Bool(false),
        "not rewritten on a warm scan"
    );
    p.reset();
    p.scan(&mut img).unwrap();
    assert_eq!(get(&p, "seen"), Value::Bool(true), "rewritten after a reset");
    let e = prog_errors("i AT %IX0.0 : BOOL := TRUE;", "");
    assert!(e.contains("I: an input (%I) cannot take an initial value"), "{e}");
    let e = prog_errors("q AT %QB0 : BYTE := 300;", "");
    assert!(e.contains("300 does not fit BYTE"), "{e}");
}

/// Hostile nesting is refused with a message instead of overflowing the
/// stack (which aborts the process: no panic to catch).
#[test]
fn deep_nesting_is_refused_not_a_stack_overflow() {
    let n = 100_000;
    for (vars, body) in [
        ("a : BOOL;", format!("a := {}a{};", "(".repeat(n), ")".repeat(n))),
        ("a : BOOL;", format!("a := a{};", " AND a".repeat(n))),
        ("a : INT;", format!("a := 2{};", " ** 2".repeat(n))),
        ("a : BOOL;", format!("a := {}a;", "NOT ".repeat(n))),
        ("a : INT;", format!("a := {}a;", "- ".repeat(n))),
        ("a : INT;", format!("a := {}a;", "+ ".repeat(n))),
        ("a : INT;", format!("a := a{};", ".x".repeat(n))),
        ("a : INT;", format!("a := a{};", "[0]".repeat(n))),
        (
            "a : BOOL;",
            format!("{}a := TRUE;{}", "IF a THEN ".repeat(n), " END_IF;".repeat(n)),
        ),
        (
            &format!("a : ARRAY[0..1] OF INT := {}1{};", "[".repeat(n), "]".repeat(n)),
            String::new(),
        ),
    ] {
        let e = prog_errors(vars, &body);
        assert!(
            e.contains("nested deeper than"),
            "{}: {e}",
            &body[..body.len().min(40)]
        );
    }
}

/// Just inside the limit, the deepest shape of each kind compiles and runs
/// on a thread with the default 2 MiB test stack (a debug build uses the
/// most stack per level); a few levels more is refused.
#[test]
fn nesting_up_to_the_limit_runs() {
    /// What is nested, how deep, and a body nesting it that deep.
    type Shape = (&'static str, usize, fn(usize) -> String);
    let shapes: [Shape; 5] = [
        ("parentheses", 64, |k| {
            format!("b := {}a{};", "(".repeat(k), ")".repeat(k))
        }),
        ("(NOT", 48, |k| {
            format!("b := {}a{};", "(NOT ".repeat(k), ")".repeat(k))
        }),
        ("NOT", 190, |k| format!("b := {}a;", "NOT ".repeat(k))),
        ("+", 190, |k| format!("i := 0{};", " + 1".repeat(k))),
        ("IF", 64, |k| {
            format!("{}b := a;{}", "IF a THEN ".repeat(k), " END_IF;".repeat(k))
        }),
    ];
    for (what, k, body) in shapes {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let vars = "a : BOOL := TRUE; b : BOOL; i : INT;";
                let (p, _) = run(vars, &body(k));
                if what == "+" {
                    assert_eq!(get(&p, "i"), Value::Int(i128::try_from(k).unwrap()));
                } else {
                    assert_eq!(get(&p, "b"), Value::Bool(true), "{k} {what}");
                }
                let e = prog_errors(vars, &body(k + 8));
                assert!(e.contains("nested deeper than"), "{} {what}: {e}", k + 8);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}

/// A variable whose declaration failed is reported once, at the
/// declaration, not again at every use.
#[test]
fn a_failed_declaration_is_reported_once() {
    let e = errors(
        "PROGRAM t VAR f : NOSUCH; w AT %QW0.1.2 : WORD; x : BOOL; END_VAR
         f(IN := TRUE); x := f.Q; w := 1; f.IN := x; FOR f := 1 TO 2 DO ; END_FOR;
         END_PROGRAM",
    );
    assert_eq!(e.len(), 2, "{e:#?}");
    assert!(e[0].contains("unknown type NOSUCH"), "{e:#?}");
    assert!(e[1].contains("multi-level"), "{e:#?}");
}

#[test]
fn errors_are_sorted_and_positioned() {
    let e = errors("PROGRAM t VAR a : INT; END_VAR\n a := b;\n  a := c;\nEND_PROGRAM");
    assert_eq!(
        e,
        vec!["2:7: undeclared variable B", "3:8: undeclared variable C"]
    );
    let e = errors("PROGRAM t\n x := ;\nEND_PROGRAM");
    assert_eq!(e, vec!["2:7: expected an expression, found ;"]);
}

#[test]
fn initial_values_use_only_literals_and_constants() {
    let (p, _) = run(
        "a : INT := 3 * 4 + 1; r : REAL := 1.5E2; b : BOOL := TRUE; w : WORD := 16#ffff;",
        "",
    );
    assert_eq!(get(&p, "a"), Value::Int(13));
    assert_eq!(get(&p, "r"), Value::Real(150.0));
    assert_eq!(get(&p, "b"), Value::Bool(true));
    assert_eq!(get(&p, "w"), Value::Int(0xFFFF));
    let e = prog_errors("a : INT; b : INT := a;", "");
    assert!(e.contains("only literals and CONSTANTs"), "{e}");
    let e = prog_errors("a : INT := 1 / 0;", "");
    assert!(e.contains("division by zero in an initial value"), "{e}");
}
