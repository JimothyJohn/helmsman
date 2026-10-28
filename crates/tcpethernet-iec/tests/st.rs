//! Structured Text through the public API: compile, scan, read the image.

use proptest::prelude::*;
use tcpethernet_iec::{FaultKind, Image, ImageSizes, Program, Value, compile};

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
        Value::Real(-4.0),
        "** binds tighter than unary minus"
    );
}

#[test]
fn implicit_widening_and_literals() {
    let (p, _) = run(
        "s : SINT := -100; u : USINT := 200; i : INT; d : DINT; r : REAL; l : LREAL; w : WORD := 16#F0F0;",
        "i := s + u; d := i * 1000; r := i + 0.5; l := d; w := w AND 16#FF00;",
    );
    assert_eq!(get(&p, "i"), Value::Int(100), "SINT + USINT is computed in INT");
    // The product is an INT (both operands are), so it wraps before the
    // assignment widens it: 100_000 mod 2^16 as signed.
    assert_eq!(get(&p, "d"), Value::Int(-31_072));
    assert_eq!(get(&p, "r"), Value::Real(100.5));
    assert_eq!(get(&p, "l"), Value::Real(-31_072.0));
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
    // 1e9 = 0x3B9A_CA00; the low 16 bits 0xCA00 as INT are -13_824.
    assert_eq!(
        get(&p, "c"),
        Value::Int(-13_824),
        "out-of-range keeps the low bits"
    );
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
        "J: a located variable takes no initial value",
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
