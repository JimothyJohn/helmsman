//! Differential test against ruSTy (PLC-lang/rusty), an independent IEC
//! 61131-3 compiler: the same programs run through both, and every variable
//! must agree after the same number of scans.
//!
//! Runs when `HELMSMAN_RUSTY` holds the command that reads ST on stdin,
//! compiles and runs it with ruSTy, e.g.
//! `HELMSMAN_RUSTY="docker run --rm -i helmsman/rusty:1.0.8"`
//! (`make rusty-image` builds that image from docker/rusty/).
//!
//! ruSTy is only executed, never linked: it is a dev-time oracle.
//!
//! Programs are a hand-written corpus aimed at the behaviours the standard
//! leaves open, plus seeded random programs. Both stay inside what both
//! compilers accept and what is observable: no division by zero (a ruSTy
//! binary traps), no endless loops, no timers (different clocks).

use std::fmt::Write as _;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::Duration;

use helmsman_iec::{Image, ImageSizes, Value, compile};

const SIZES: ImageSizes = ImageSizes { i: 0, q: 0, m: 0 };
const SCANS: u32 = 3;

struct Case {
    /// TYPEs, FUNCTIONs and function blocks the program uses.
    decls: String,
    /// `(name, type, initial value)`.
    vars: Vec<(String, String, Option<String>)>,
    body: String,
}

fn var(name: &str, ty: &str) -> (String, String, Option<String>) {
    (name.into(), ty.into(), None)
}

fn var_init(name: &str, ty: &str, init: &str) -> (String, String, Option<String>) {
    (name.into(), ty.into(), Some(init.into()))
}

fn case(decls: &str, vars: Vec<(String, String, Option<String>)>, body: &str) -> Case {
    Case {
        decls: decls.into(),
        vars,
        body: body.into(),
    }
}

impl Case {
    fn program(&self, name: &str) -> String {
        let mut s = format!("PROGRAM {name}\nVAR\n");
        for (n, t, init) in &self.vars {
            match init {
                Some(i) => writeln!(s, "  {n} : {t} := {i};").unwrap(),
                None => writeln!(s, "  {n} : {t};").unwrap(),
            }
        }
        write!(s, "END_VAR\n{}\nEND_PROGRAM\n", self.body).unwrap();
        s
    }
}

/// Elementary types we compare (instances, arrays and structs are reached
/// through scalar copies in the program body).
fn printable(ty: &str) -> bool {
    matches!(
        ty,
        "BOOL"
            | "SINT"
            | "INT"
            | "DINT"
            | "LINT"
            | "USINT"
            | "UINT"
            | "UDINT"
            | "ULINT"
            | "BYTE"
            | "WORD"
            | "DWORD"
            | "LWORD"
            | "REAL"
            | "LREAL"
            | "DATE"
            | "TOD"
            | "DT"
    ) || is_text(ty)
}

fn is_text(ty: &str) -> bool {
    ty.starts_with("STRING")
}

/// printf conversion for a scalar type (C default argument promotion).
fn fmt(ty: &str) -> &'static str {
    match ty {
        t if is_text(t) => "%s",
        "LINT" | "DATE" | "TOD" | "DT" => "%lld",
        "ULINT" | "LWORD" => "%llu",
        "SINT" | "INT" | "DINT" => "%d",
        "REAL" | "LREAL" => "%.17g",
        _ => "%u",
    }
}

/// Every scalar variable after `SCANS` scans, by ruSTy: `(case, var) -> text`.
fn run_rusty(cmd: &str, cases: &[Case]) -> Vec<Vec<String>> {
    let mut src = String::new();
    for c in cases {
        src.push_str(&c.decls);
        src.push('\n');
    }
    for (k, c) in cases.iter().enumerate() {
        src.push_str(&c.program(&format!("case_{k}")));
    }
    src.push_str("FUNCTION main : DINT\nVAR scan : DINT; END_VAR\n");
    writeln!(src, "FOR scan := 1 TO {SCANS} DO").unwrap();
    for k in 0..cases.len() {
        writeln!(src, "  case_{k}();").unwrap();
    }
    src.push_str("END_FOR;\n");
    for (k, c) in cases.iter().enumerate() {
        for (n, t, _) in c.vars.iter().filter(|v| printable(&v.1)) {
            writeln!(src, "printf('{k} {n} {}$N', case_{k}.{n});", fmt(t)).unwrap();
        }
    }
    src.push_str("END_FUNCTION\n");

    let mut parts = cmd.split_whitespace();
    let mut child = Command::new(parts.next().expect("HELMSMAN_RUSTY is empty"))
        .args(parts)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot start HELMSMAN_RUSTY");
    child.stdin.take().unwrap().write_all(src.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "ruSTy failed ({}):\n{}\n--- source ---\n{src}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let mut values: Vec<Vec<String>> = cases.iter().map(|c| vec![String::new(); c.vars.len()]).collect();
    for line in String::from_utf8(out.stdout).unwrap().lines() {
        let mut words = line.splitn(3, ' ');
        let (case, name, value) = (
            words.next().unwrap(),
            words.next().unwrap(),
            words.next().unwrap(),
        );
        let case: usize = case.parse().unwrap();
        let slot = cases[case].vars.iter().position(|(n, _, _)| n == name).unwrap();
        values[case][slot] = value.to_string();
    }
    values
}

/// Our value, printed the way printf would.
fn ours(c: &Case) -> Result<Vec<String>, String> {
    let src = format!("{}\n{}", c.decls, c.program("case_x"));
    let mut p = compile(&src, SIZES).map_err(|d| format!("{d:?}"))?;
    let mut img = Image::new(SIZES);
    for k in 0..SCANS {
        p.scan_at(&mut img, Duration::from_millis(u64::from(k) * 10))
            .map_err(|f| f.to_string())?;
    }
    Ok(c.vars
        .iter()
        .map(|(n, t, _)| match p.get(n) {
            _ if is_text(t) => p.get_str(n).unwrap_or_default(),
            Some(Value::Bool(b)) => u8::from(b).to_string(),
            Some(Value::Int(i)) => i.to_string(),
            Some(Value::Real(r)) if t == "REAL" || t == "LREAL" => format!("{r:?}"),
            other => format!("{other:?}"),
        })
        .collect())
}

/// Same value? Integers textually, reals bit for bit (both NaN counts).
fn same(ty: &str, a: &str, b: &str) -> bool {
    if ty == "REAL" || ty == "LREAL" {
        let parse = |s: &str| {
            s.trim_start_matches('-')
                .parse::<f64>()
                .map(|v| if s.starts_with('-') { -v } else { v })
        };
        return match (parse(a), parse(b)) {
            (Ok(x), Ok(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
            _ => false,
        };
    }
    a == b
}

fn compare(cmd: &str, cases: &[Case]) {
    let theirs = run_rusty(cmd, cases);
    let mut bad = String::new();
    for (k, c) in cases.iter().enumerate() {
        let mine = ours(c)
            .unwrap_or_else(|e| panic!("we reject a program ruSTy accepts:\n{e}\n{}", c.program("case")));
        for (i, (n, t, _)) in c.vars.iter().enumerate().filter(|(_, v)| printable(&v.1)) {
            if !same(t, &mine[i], &theirs[k][i]) {
                writeln!(
                    bad,
                    "case {k} {n} : {t}: ours {} ruSTy {}\n{}{}",
                    mine[i],
                    theirs[k][i],
                    c.decls,
                    c.program("case")
                )
                .unwrap();
            }
        }
    }
    assert!(bad.is_empty(), "disagreements with ruSTy:\n{bad}");
}

fn rusty() -> Option<String> {
    let cmd = std::env::var("HELMSMAN_RUSTY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    if cmd.is_none() {
        eprintln!("HELMSMAN_RUSTY not set; ruSTy oracle skipped");
    }
    cmd
}

// ---- strings ----

/// STRING behaviour, one case per question: what an assignment cuts, what
/// a count or position outside the string does, what a conversion makes of
/// text that is no number. Every variable is compared, text as printed.
#[allow(clippy::too_many_lines)]
fn string_corpus() -> Vec<Case> {
    let t = |n: &str| var(n, "STRING[8]");
    vec![
        // Assignment cuts at the declared length; LEN sees the cut text.
        case(
            "",
            vec![t("a"), var("b", "STRING[3]"), var("c", "STRING"), var("n", "DINT"), var("m", "DINT")],
            "a := 'abcdefghij'; b := a; c := 'x'; n := LEN(a); m := LEN(b);",
        ),
        case(
            "",
            vec![var_init("a", "STRING[8]", "'hello'"), var("e", "STRING[8]"), var("n", "DINT"), var("m", "DINT")],
            "n := LEN(a); e := ''; m := LEN(e);",
        ),
        // LEFT, RIGHT, MID at every boundary of a 5-character string.
        case(
            "",
            vec![
                var_init("a", "STRING[8]", "'abcde'"),
                t("l0"), t("l1"), t("l4"), t("l5"), t("l6"),
                t("r0"), t("r1"), t("r4"), t("r5"), t("r6"),
            ],
            "l0 := LEFT(a, 0); l1 := LEFT(a, 1); l4 := LEFT(a, 4); l5 := LEFT(a, 5); l6 := LEFT(a, 6);
             r0 := RIGHT(a, 0); r1 := RIGHT(a, 1); r4 := RIGHT(a, 4); r5 := RIGHT(a, 5); r6 := RIGHT(a, 6);",
        ),
        case(
            "",
            vec![
                var_init("a", "STRING[8]", "'abcde'"),
                t("m11"), t("m15"), t("m16"), t("m31"), t("m33"), t("m35"), t("m51"), t("m55"), t("m01"), t("m60"), t("m00"),
            ],
            "m11 := MID(a, 1, 1); m15 := MID(a, 5, 1); m16 := MID(a, 6, 1); m31 := MID(a, 1, 3); m33 := MID(a, 3, 3);
             m35 := MID(a, 5, 3); m51 := MID(a, 1, 5); m55 := MID(a, 5, 5); m01 := MID(a, 1, 0); m60 := MID(a, 0, 6); m00 := MID(a, 0, 0);",
        ),
        // CONCAT, with truncation, with itself.
        case(
            "",
            vec![var_init("a", "STRING[8]", "'abc'"), var("b", "STRING[4]"), t("c"), t("d"), var("e", "STRING")],
            "c := CONCAT(a, 'def'); b := CONCAT(a, 'def'); d := CONCAT(a, a, a); a := CONCAT(a, a); e := CONCAT('x', a, 'y');",
        ),
        // FIND: position of the first match, 0 when none or when the pattern is empty.
        case(
            "",
            vec![
                var_init("a", "STRING[8]", "'abcabc'"),
                var("f1", "DINT"), var("f2", "DINT"), var("f3", "DINT"), var("f4", "DINT"), var("f5", "DINT"), var("f6", "DINT"),
            ],
            "f1 := FIND(a, 'abc'); f2 := FIND(a, 'ca'); f3 := FIND(a, 'x'); f4 := FIND(a, 'abcabcabc'); f5 := FIND(a, 'c'); f6 := FIND('', 'a');",
        ),
        // INSERT, DELETE, REPLACE.
        case(
            "",
            vec![
                var_init("a", "STRING[10]", "'abcde'"),
                t("i0"), t("i2"), t("i5"), t("d1"), t("d2"), t("d3"), t("r1"), t("r2"), t("r3"),
            ],
            "i0 := INSERT(a, 'XY', 0); i2 := INSERT(a, 'XY', 2); i5 := INSERT(a, 'XY', 5);
             d1 := DELETE(a, 1, 1); d2 := DELETE(a, 2, 2); d3 := DELETE(a, 9, 4);
             r1 := REPLACE(a, 'XY', 1, 1); r2 := REPLACE(a, 'XY', 2, 3); r3 := REPLACE(a, 'XY', 0, 5);",
        ),
        // Comparison, byte by byte, a prefix sorting first.
        case(
            "",
            vec![
                var_init("a", "STRING[8]", "'abc'"),
                var_init("b", "STRING[8]", "'abd'"),
                var_init("c", "STRING[8]", "'ab'"),
                var("eq", "BOOL"), var("ne", "BOOL"), var("lt", "BOOL"), var("gt", "BOOL"), var("le", "BOOL"), var("ge", "BOOL"),
                var("p", "BOOL"), var("q", "BOOL"), var("z", "BOOL"),
            ],
            "eq := a = b; ne := a <> b; lt := a < b; gt := a > b; le := a <= b; ge := a >= b;
             p := c < a; q := a > c; z := '' < c;",
        ),
        // Conversions.
        case(
            "",
            vec![
                var("i1", "STRING[8]"), var("i2", "STRING[8]"), var("i3", "STRING[3]"), var("b1", "STRING[8]"), var("b2", "STRING[8]"),
                var("n1", "DINT"), var("n2", "DINT"), var("n3", "DINT"),
            ],
            "i1 := INT_TO_STRING(-42); i2 := DINT_TO_STRING(123456); i3 := DINT_TO_STRING(123456);
             b1 := BOOL_TO_STRING(TRUE); b2 := BOOL_TO_STRING(FALSE);
             n1 := STRING_TO_DINT('-17'); n2 := STRING_TO_DINT('300'); n3 := STRING_TO_DINT('12');",
        ),
        // Text that is no number, and one that does not fit the type.
        case(
            "",
            vec![
                var("a", "DINT"), var("b", "DINT"), var("c", "DINT"), var("d", "DINT"), var("e", "DINT"),
                var("f", "DINT"), var("g", "DINT"), var("h", "DINT"), var("i", "SINT"), var("j", "DINT"),
                var("k", "LINT"), var("l", "UDINT"),
            ],
            "a := STRING_TO_DINT('12x'); b := STRING_TO_DINT(''); c := STRING_TO_DINT(' 7'); d := STRING_TO_DINT('+5');
             e := STRING_TO_DINT('0x10'); f := STRING_TO_DINT('1.5'); g := STRING_TO_DINT('abc'); h := STRING_TO_DINT('-');
             i := STRING_TO_SINT('300'); j := STRING_TO_DINT('99999999999'); k := STRING_TO_LINT('-9223372036854775808');
             l := STRING_TO_UDINT('-1');",
        ),
        // Fractions and exponents are cut toward zero; a dot with no digit
        // beside it, a second dot or a space inside is no number.
        case(
            "",
            vec![
                var("a", "DINT"), var("b", "DINT"), var("c", "DINT"), var("d", "DINT"), var("e", "DINT"),
                var("f", "DINT"), var("g", "DINT"), var("h", "DINT"), var("i", "DINT"), var("j", "DINT"),
                var("k", "LINT"), var("l", "LINT"), var("m", "REAL"), var("n", "REAL"), var("o", "REAL"),
            ],
            "a := STRING_TO_DINT('1.9'); b := STRING_TO_DINT('-1.9'); c := STRING_TO_DINT('1e2'); d := STRING_TO_DINT('7 ');
             e := STRING_TO_DINT('  -7'); f := STRING_TO_DINT('3000000000'); g := STRING_TO_DINT('1.'); h := STRING_TO_DINT('.5');
             i := STRING_TO_DINT('1 2'); j := STRING_TO_DINT('1.5.5');
             k := STRING_TO_LINT('9007199254740993'); l := STRING_TO_LINT('9223372036854775807');
             m := STRING_TO_REAL('2.5e-1'); n := STRING_TO_REAL('.5'); o := STRING_TO_REAL('abc');",
        ),
    ]
}

// ---- the corpus: behaviours the standard leaves to the implementation ----

#[allow(clippy::too_many_lines)]
fn corpus() -> Vec<Case> {
    vec![
        // Wrapping arithmetic at every width, and the INT*INT -> DINT trap.
        case(
            "",
            vec![
                var_init("a", "INT", "32767"),
                var_init("b", "SINT", "-128"),
                var_init("c", "UDINT", "0"),
                var_init("i", "INT", "100"),
                var("d", "DINT"),
                var_init("l", "LINT", "9223372036854775807"),
                var_init("u", "ULINT", "18446744073709551615"),
            ],
            "a := a + 1; b := -b; c := c - 1; d := i * 1000; l := l + 1; u := u + 1;",
        ),
        // Division and MOD signs.
        case(
            "",
            vec![var("q1", "DINT"), var("q2", "DINT"), var("m1", "DINT"), var("m2", "DINT")],
            // MIN / -1 is left out: x86 traps on it in native code.
            "q1 := -7 / 2; q2 := 7 / -2; m1 := -7 MOD 2; m2 := 7 MOD -2;",
        ),
        // REAL -> integer rounding and narrowing conversions.
        case(
            "",
            vec![
                var("a", "INT"),
                var("b", "INT"),
                var("c", "INT"),
                var("d", "INT"),
                var("e", "SINT"),
                var("f", "UINT"),
                var("g", "DINT"),
                var("h", "BYTE"),
            ],
            "a := REAL_TO_INT(2.5); b := REAL_TO_INT(-2.5); c := LREAL_TO_INT(0.5); d := LREAL_TO_INT(1.5);
             e := INT_TO_SINT(300); f := DINT_TO_UINT(-1); g := TRUNC(-2.7); h := INT_TO_BYTE(-1);",
        ),
        // Precedence: ** above unary minus; chains associate left.
        case(
            "",
            vec![var("r", "LREAL"), var("s", "LREAL"), var("t", "DINT"), var("u", "BOOL")],
            "r := -2.0 ** 2; s := 2.0 ** 3.0 ** 2.0; t := 100 - 10 - 1; u := 1 < 2 = TRUE;",
        ),
        // FOR: the control variable after the loop; counting down.
        case(
            "",
            vec![var("i", "INT"), var("n", "INT"), var("k", "INT"), var("s", "DINT")],
            "FOR i := 1 TO 10 DO s := s + i; END_FOR; FOR n := 10 TO 1 BY -3 DO k := k + 1; END_FOR;",
        ),
        // REAL rounds through single precision at every step.
        case(
            "",
            vec![var_init("a", "REAL", "0.1"), var("b", "REAL"), var("c", "LREAL"), var_init("d", "REAL", "16777216.0")],
            "b := a + 0.2; c := a; d := d + 1.0;",
        ),
        // Functions, nested calls, defaults on omitted inputs.
        case(
            "FUNCTION oracle_add3 : DINT
               VAR_INPUT x, y : DINT; z : DINT := 100; END_VAR
               oracle_add3 := x + y + z;
             END_FUNCTION",
            vec![var("a", "DINT"), var("b", "DINT"), var("c", "DINT")],
            "a := oracle_add3(1, 2, 3); b := oracle_add3(x := 1, y := 2); c := oracle_add3(oracle_add3(1, 1, 1), 10, 0);",
        ),
        // Function blocks keep state per instance across scans.
        case(
            "FUNCTION_BLOCK oracle_acc
               VAR_INPUT add : DINT; END_VAR
               VAR_OUTPUT total : DINT; END_VAR
               total := total + add;
             END_FUNCTION_BLOCK",
            vec![var("x", "oracle_acc"), var("y", "oracle_acc"), var("tx", "DINT"), var("ty", "DINT")],
            "x(add := 2); y(add := 5); y(); tx := x.total; ty := y.total;",
        ),
        // Arrays and structs.
        case(
            "TYPE oracle_pt : STRUCT x : INT := 3; y : INT; END_STRUCT; END_TYPE",
            vec![
                var_init("a", "ARRAY[1..5] OF INT", "[1, 2, 3]"),
                var("p", "oracle_pt"),
                var("s", "INT"),
                var("j", "INT"),
                var("px", "INT"),
            ],
            "FOR j := 1 TO 5 DO s := s + a[j]; END_FOR; a[5] := s; p.y := p.y + p.x; px := p.y;",
        ),
        // VAR_IN_OUT on a function block: an array and a scalar by
        // reference, the block's writes seen by the caller, state per
        // instance. The expected values come from ruSTy, as for every case.
        case(
            "FUNCTION_BLOCK oracle_bump
               VAR_INPUT add : DINT; END_VAR
               VAR_IN_OUT buf : ARRAY[0..3] OF DINT; n : DINT; END_VAR
               VAR_OUTPUT first : DINT; END_VAR
               VAR j : DINT; END_VAR
               FOR j := 0 TO 3 DO buf[j] := buf[j] + add; END_FOR;
               n := n * 2 + 1;
               first := buf[0];
             END_FUNCTION_BLOCK",
            vec![
                var_init("a", "ARRAY[0..3] OF DINT", "[1, 2, 3, 4]"),
                var("b", "ARRAY[0..3] OF DINT"),
                var_init("k", "DINT", "5"),
                var("p", "oracle_bump"),
                var("q", "oracle_bump"),
                var("a0", "DINT"),
                var("a3", "DINT"),
                var("b0", "DINT"),
                var("f", "DINT"),
            ],
            "p(add := 10, buf := a, n := k); q(add := 1, buf := b, n := k); p(add := 10, buf := a, n := k);
             a0 := a[0]; a3 := a[3]; b0 := b[0]; f := p.first;",
        ),
        // VAR_OUTPUT on a function, bound with =>. Every call binds both
        // outputs: ruSTy refuses a call that leaves one out, with named
        // inputs or positional ones (E032, "this POU takes 4 arguments but
        // 2 arguments were supplied"), and so does Helmsman (Nick,
        // 2026-11-15; the refusal is in tests/in_out.rs).
        case(
            "FUNCTION oracle_divmod : DINT
               VAR_INPUT a, b : DINT; END_VAR
               VAR_OUTPUT rem : DINT; ok : BOOL; END_VAR
               IF b = 0 THEN oracle_divmod := 0; rem := a; ok := FALSE; RETURN; END_IF;
               oracle_divmod := a / b;
               rem := a MOD b;
               ok := TRUE;
             END_FUNCTION",
            vec![
                var("q1", "DINT"),
                var("r1", "DINT"),
                var("g1", "BOOL"),
                var("q2", "DINT"),
                var("r2", "DINT"),
                var("g2", "BOOL"),
                var("q3", "DINT"),
                var("r3", "DINT"),
                var("g3", "BOOL"),
            ],
            "q1 := oracle_divmod(17, 5, rem => r1, ok => g1); q2 := oracle_divmod(a := 9, b := 0, rem => r2, ok => g2);
             q3 := oracle_divmod(-17, 5, rem => r3, ok => g3);",
        ),
        // VAR_GLOBAL, as ruSTy's own tests write it. The names are unique
        // across the corpus because ruSTy gets every case in one source and
        // a global is visible to all of it. Three forms, each taken from a
        // ruSTy lit test (tests/lit/single/ of PLC-lang/rusty, in the
        // fetched corpus under target/st-corpus/rusty):
        // type_promotion/bit_string/constants/constant_operands.st: a
        // CONSTANT global block, and a global initialised from it (ruSTy divides
        // the DWORD by 2 there; Helmsman has no `/` on bit strings, so AND).
        case(
            "VAR_GLOBAL CONSTANT
                 oracle_mask : DWORD := 16#FFFF_FFFF;
             END_VAR
             VAR_GLOBAL
                 oracle_half : DWORD := oracle_mask AND 16#0FFF_FFFF;
             END_VAR",
            vec![var("a", "DWORD"), var("b", "BOOL")],
            "a := oracle_half; b := oracle_mask > 1;",
        ),
        // var_external/var_external.st: a global array, reached by name in
        // a body with no declaration, and by a function that declares it
        // `VAR_EXTERNAL CONSTANT` (ruSTy's test writes through that; here
        // the function only reads, which Helmsman allows).
        case(
            "VAR_GLOBAL
                 oracle_arr : ARRAY [0..10] OF DINT;
             END_VAR
             FUNCTION oracle_total : DINT
             VAR_EXTERNAL CONSTANT
                 oracle_arr : ARRAY [0..10] OF DINT;
             END_VAR
             VAR i : DINT; END_VAR
                 FOR i := 0 TO 10 DO
                     oracle_total := oracle_total + oracle_arr[i];
                 END_FOR;
             END_FUNCTION",
            vec![var("i", "DINT"), var("t", "DINT"), var("e3", "DINT")],
            "FOR i := 0 TO 10 DO oracle_arr[i] := oracle_arr[i] + i; END_FOR; t := oracle_total(); e3 := oracle_arr[3];",
        ),
        // var_external/var_external_initializer_has_no_effect.st: a global
        // with an initial value, and a function that names it VAR_EXTERNAL
        // (ruSTy's file also gives the VAR_EXTERNAL an initial value, which
        // Helmsman refuses; this one leaves it out).
        case(
            "VAR_GLOBAL
                 oracle_i : DINT := 42;
             END_VAR
             FUNCTION oracle_twice : DINT
             VAR_EXTERNAL
                 oracle_i : DINT;
             END_VAR
                 oracle_twice := oracle_i * 2;
             END_FUNCTION",
            vec![var("a", "DINT"), var("b", "DINT")],
            "a := oracle_twice(); b := oracle_i + 1;",
        ),
        // Standard functions.
        case(
            "",
            vec![
                var("a", "INT"),
                var("b", "DINT"),
                var("c", "DINT"),
                var("d", "DINT"),
                var("e", "BYTE"),
                var("f", "WORD"),
                var("g", "DWORD"),
                var("h", "LREAL"),
                var("k", "DINT"),
            ],
            "a := ABS(INT#-5); b := MIN(DINT#7, 3); c := MAX(DINT#-1, 4); d := LIMIT(DINT#0, 150, 100);
             e := SHL(BYTE#16#81, 1); f := ROR(WORD#1, 1); g := ROL(DWORD#16#80000001, 4); h := SQRT(LREAL#16.0);
             k := SEL(TRUE, DINT#1, 2);",
        ),
        // Edge triggers and counters from the standard library.
        // BLINK_PATTERN is left out on purpose: it is a Helmsman extension,
        // not IEC 61131-3, and ruSTy has no such block to compare with. Its
        // contract is pinned against its definition in tests/pou.rs.
        case(
            "",
            vec![
                var("re", "R_TRIG"),
                var("ctu", "CTU"),
                var("clk", "BOOL"),
                var("rq", "BOOL"),
                var("cv", "INT"),
                var("cq", "BOOL"),
            ],
            "clk := NOT clk; re(CLK := clk); ctu(CU := clk, PV := 2); rq := re.Q; cv := ctu.CV; cq := ctu.Q;",
        ),
        // Bit strings and logic.
        case(
            "",
            vec![var_init("w", "WORD", "16#F0F0"), var("x", "WORD"), var("y", "WORD"), var("z", "BOOL")],
            "x := w AND 16#FF00; y := NOT w XOR 16#000F; z := (w AND 16#0010) <> 0 OR FALSE;",
        ),
    ]
}

/// ruSTy 1.0.7 compared UDINT and ULINT as signed (`ULINT_max > 1` was
/// FALSE); 1.0.8 carries the fix (PLC-lang/rusty#2015, backported in #2016).
/// Pinned so a regression on either side shows up here first, before the
/// random sweep trips over it.
#[test]
fn rusty_compares_unsigned_as_unsigned() {
    let Some(cmd) = rusty() else { return };
    let c = case(
        "",
        vec![
            var_init("big64", "ULINT", "18446744073709551615"),
            var_init("big32", "UDINT", "3398743669"),
            var("g64", "BOOL"),
            var("g32", "BOOL"),
        ],
        "g64 := big64 > ULINT#1; g32 := big32 > UDINT#1;",
    );
    assert_eq!(ours(&c).unwrap()[2..], ["1".to_string(), "1".to_string()]);
    let theirs = run_rusty(&cmd, std::slice::from_ref(&c));
    assert_eq!(
        theirs[0][2..],
        ["1".to_string(), "1".to_string()],
        "ruSTy compares UDINT/ULINT as signed again"
    );
}

/// ruSTy 1.0.8 negates a SINT or INT variable or literal in DINT (rusty#2016),
/// so `-v` on the type's minimum is +128 / +32768 there, and the value
/// shows in a division before the assignment truncates it. Ours did the
/// same only for an arithmetic result and wrapped a plain variable's
/// negation in its own width; since the TODO.md card
/// `rusty-negation-width` (Nick, 2026-11-08) both sides agree. Pinned so
/// either side moving points here.
#[test]
fn small_int_negation_is_dint_wide_on_both_sides() {
    let Some(cmd) = rusty() else { return };
    let c = case(
        "",
        vec![
            var_init("s", "SINT", "-128"),
            var_init("i", "INT", "-32768"),
            var("qs", "SINT"),
            var("qi", "INT"),
        ],
        "qs := (-s) / SINT#2; qi := (-i) / INT#2;",
    );
    let want = ["64".to_string(), "16384".to_string()];
    assert_eq!(ours(&c).unwrap()[2..], want, "ours negates SINT/INT in DINT");
    let theirs = run_rusty(&cmd, std::slice::from_ref(&c));
    assert_eq!(
        theirs[0][2..],
        want,
        "ruSTy negates SINT/INT in their own width again"
    );
}

/// Without ruSTy at hand the corpus still has to compile and run here, so a
/// case that only the oracle run would reject shows up in every `cargo test`.
#[test]
fn corpus_compiles_and_runs_here() {
    for (k, c) in corpus().iter().chain(&string_corpus()).enumerate() {
        ours(c).unwrap_or_else(|e| panic!("corpus case {k}: {e}\n{}", c.program("case")));
    }
}

/// The in-out and function-output cases, by value: what ruSTy is expected
/// to print for them is what these definitions give (a block adds to the
/// caller's array through the reference, `n` doubles and gains one).
#[test]
fn by_reference_cases_have_the_values_their_definitions_give() {
    let corpus = corpus();
    let get = |decl: &str| corpus.iter().find(|c| c.decls.contains(decl)).unwrap();
    let bump = ours(get("oracle_bump")).unwrap();
    // Three scans of: p adds 10 twice, q adds 1 to b. a = [1,2,3,4] + 60,
    // so a0 = 61, a3 = 64; b0 = 3; p.first = a[0] after its last call.
    assert_eq!(bump[5..], ["61", "64", "3", "61"]);
    // k is doubled and increased by one at each of the 9 calls:
    // (5 + 1) * 2^9 - 1.
    assert_eq!(bump[2], "3071");
    let divmod = ours(get("oracle_divmod")).unwrap();
    assert_eq!(divmod, ["3", "2", "1", "0", "9", "0", "-3", "-2", "1"]);
}

#[test]
fn corpus_agrees_with_rusty() {
    let Some(cmd) = rusty() else { return };
    compare(&cmd, &corpus());
}

#[test]
fn strings_agree_with_rusty() {
    let Some(cmd) = rusty() else { return };
    compare(&cmd, &string_corpus());
}

// ---- dates and times of day ----

/// DATE, TOD and DT, one case per question: what a literal counts, what
/// the arithmetic table gives (and where a TOD wraps), the conversions,
/// `CONCAT_*` with a date that does not exist, `DAY_OF_WEEK`. ruSTy prints
/// each as the 64-bit nanosecond count, which is what we hold.
///
/// Written from ruSTy 1.0.8's source (typesystem.rs, libs/stdlib) on a
/// machine without docker: not run against it yet.
#[allow(clippy::too_many_lines)]
fn calendar_corpus() -> Vec<Case> {
    let v = |n: &str, t: &str| var(n, t);
    vec![
        case(
            "",
            vec![
                var_init("d", "DATE", "D#2024-02-29"),
                var_init("t", "TOD", "TOD#08:30:15.5"),
                var_init("x", "DT", "DT#2024-02-29-08:30:00"),
                var_init("e", "DT", "DT#1969-12-31-23:59:59"),
                var_init("d0", "DATE", "D#1970-01-01"),
            ],
            "",
        ),
        case(
            "",
            vec![
                var_init("a", "TOD", "TOD#23:59:59.999"),
                var_init("b", "TOD", "TOD#00:00:00"),
                v("c", "TOD"),
                v("d", "TOD"),
                v("e", "TOD"),
                v("f", "TOD"),
            ],
            "c := a + T#1s; d := b - T#1ms; e := a + T#49h; f := b - T#25h;",
        ),
        case(
            "",
            vec![
                var_init("a", "DT", "DT#2023-12-31-23:59:59"),
                var_init("b", "DT", "DT#2024-02-28-12:00:00"),
                v("a1", "DT"),
                v("b1", "DT"),
                v("b2", "DT"),
                v("gap", "TIME"),
            ],
            "a1 := a + T#1s; b1 := b + T#1d; b2 := b1 - T#36h; gap := a1 - b;",
        ),
        case(
            "",
            vec![
                var_init("d1", "DATE", "D#2024-03-01"),
                var_init("d0", "DATE", "D#2023-03-01"),
                var_init("t1", "TOD", "TOD#06:00:00"),
                var_init("t0", "TOD", "TOD#18:30:00"),
                v("y", "TIME"),
                v("w", "TIME"),
            ],
            "y := d1 - d0; w := t1 - t0;",
        ),
        case(
            "",
            vec![
                var_init("x", "DT", "DT#2024-02-29-08:30:15"),
                var_init("y", "DT", "DT#1969-12-31-23:00:00"),
                v("d", "DATE"),
                v("t", "TOD"),
                v("dy", "DATE"),
                v("ty", "TOD"),
            ],
            "d := DT_TO_DATE(x); t := DT_TO_TOD(x); dy := DT_TO_DATE(y); ty := DT_TO_TOD(y);",
        ),
        case(
            "",
            vec![
                var_init("d", "DATE", "D#2024-02-29"),
                var_init("t", "TOD", "TOD#13:05:00"),
                v("x", "DT"),
                v("w", "SINT"),
                v("w0", "SINT"),
                v("c", "DATE"),
                v("ct", "TOD"),
                v("bad", "DATE"),
                v("badt", "TOD"),
            ],
            "x := CONCAT_DATE_TOD(d, t); w := DAY_OF_WEEK(d); w0 := DAY_OF_WEEK(D#1970-01-01); \
             c := CONCAT_DATE(2024, 3, 31); ct := CONCAT_TOD(23, 59, 59, 999); \
             bad := CONCAT_DATE(2023, 2, 29); badt := CONCAT_TOD(24, 0, 0, 0);",
        ),
        case(
            "",
            vec![
                var_init("a", "DT", "DT#2024-02-29-08:30:00"),
                var_init("z", "DT", "DT#2024-02-29-08:30:01"),
                v("lt", "BOOL"),
                v("ge", "BOOL"),
                v("eq", "BOOL"),
                v("ne", "BOOL"),
            ],
            "lt := a < z; ge := a >= z; eq := a = a; ne := a <> z;",
        ),
    ]
}

#[test]
fn calendar_corpus_compiles_and_runs_here() {
    for c in calendar_corpus() {
        ours(&c).unwrap_or_else(|e| panic!("{e}\n{}", c.program("case")));
    }
}

#[test]
fn calendar_agrees_with_rusty() {
    let Some(cmd) = rusty() else { return };
    compare(&cmd, &calendar_corpus());
}

// ---- seeded random programs ----

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

const INTS: [(&str, i128, i128); 8] = [
    ("SINT", -128, 127),
    ("INT", -32768, 32767),
    ("DINT", -2_147_483_648, 2_147_483_647),
    ("LINT", -9_223_372_036_854_775_808, 9_223_372_036_854_775_807),
    ("USINT", 0, 255),
    ("UINT", 0, 65535),
    ("UDINT", 0, 4_294_967_295),
    ("ULINT", 0, 18_446_744_073_709_551_615),
];

fn literal(rng: &mut Rng, lo: i128, hi: i128) -> i128 {
    // Mostly small, sometimes at the edges, sometimes anywhere.
    match rng.below(6) {
        0 => lo,
        1 => hi,
        2 => {
            let span = u128::try_from(hi - lo).unwrap() + 1;
            lo + i128::try_from(u128::from(rng.next()) % span).unwrap()
        }
        _ => (i128::from(rng.next() % 41) - 20).clamp(lo, hi),
    }
}

/// A typed literal of an integer type, e.g. `INT#-5`.
fn lit(rng: &mut Rng, ty: &str, lo: i128, hi: i128) -> String {
    format!("{ty}#{}", literal(rng, lo, hi))
}

/// A condition comparing the `ty` variable with a literal of its own type.
fn cond(rng: &mut Rng, ty: &str, lo: i128, hi: i128) -> String {
    format!("v_{} < {}", ty.to_lowercase(), lit(rng, ty, lo, hi))
}

/// A random integer expression of type `ty` over `vars` (all of the INTS).
fn int_expr(rng: &mut Rng, ty: &str, depth: u32) -> String {
    let &(_, lo, hi) = INTS.iter().find(|(t, _, _)| *t == ty).unwrap();
    let v = format!("v_{}", ty.to_lowercase());
    if depth == 0 {
        return match rng.below(3) {
            0 => lit(rng, ty, lo, hi),
            1 => v.clone(),
            _ => {
                let &(from, _, _) = rng.pick(&INTS);
                if from == ty {
                    v.clone()
                } else {
                    format!("{from}_TO_{ty}(v_{})", from.to_lowercase())
                }
            }
        };
    }
    let a = int_expr(rng, ty, depth - 1);
    match rng.below(7) {
        0 => format!("({a} + {})", int_expr(rng, ty, depth - 1)),
        1 => format!("({a} - {})", int_expr(rng, ty, depth - 1)),
        2 => format!("({a} * {})", int_expr(rng, ty, depth - 1)),
        3 | 4 => {
            // Divisors are non-zero literals: a ruSTy binary traps on /0.
            // Nor -1: MIN / -1 traps on x86.
            let mut d = literal(rng, lo, hi);
            if d == 0 || d == -1 {
                d = if hi > 6 { 7 } else { 1 };
            }
            let op = if rng.below(2) == 0 { "/" } else { "MOD" };
            format!("({a} {op} {ty}#{d})")
        }
        5 if lo < 0 => format!("(-{a})"),
        _ => format!(
            "SEL({}, {a}, {})",
            cond(rng, ty, lo, hi),
            int_expr(rng, ty, depth - 1)
        ),
    }
}

fn random_int_case(rng: &mut Rng) -> Case {
    let vars: Vec<_> = INTS
        .iter()
        .map(|&(t, lo, hi)| {
            var_init(
                &format!("v_{}", t.to_lowercase()),
                t,
                &literal(rng, lo, hi).to_string(),
            )
        })
        .collect();
    let mut body = String::new();
    for _ in 0..4 {
        let &(t, lo, hi) = rng.pick(&INTS);
        let target = format!("v_{}", t.to_lowercase());
        let depth = 1 + u32::try_from(rng.below(3)).unwrap();
        let e = int_expr(rng, t, depth);
        match rng.below(4) {
            0 => writeln!(body, "IF {} THEN {target} := {e}; ELSE {target} := {target} + {t}#1; END_IF;", cond(rng, t, lo, hi)).unwrap(),
            1 => writeln!(body, "CASE v_sint OF -128..-1: {target} := {e}; 0, 5: ; ELSE {target} := {target} - {t}#1; END_CASE;").unwrap(),
            _ => writeln!(body, "{target} := {e};").unwrap(),
        }
    }
    Case {
        decls: String::new(),
        vars,
        body,
    }
}

fn random_real_case(rng: &mut Rng) -> Case {
    let reals = ["REAL", "LREAL"];
    let lits = [
        "0.1",
        "1.5",
        "-2.25",
        "1000.0",
        "3.0E-3",
        "16777217.0",
        "0.7",
        "-0.5",
    ];
    let vars = vec![
        var_init("r", "REAL", rng.pick(&lits)),
        var_init("l", "LREAL", rng.pick(&lits)),
        var_init("i", "DINT", &literal(rng, -1000, 1000).to_string()),
    ];
    let mut body = String::new();
    for _ in 0..4 {
        let t = *rng.pick(&reals);
        let target = if t == "REAL" { "r" } else { "l" };
        let operand = |rng: &mut Rng| match rng.below(4) {
            0 => format!("{t}#{}", rng.pick(&lits)),
            1 => "r".into(),
            2 if t == "LREAL" => "DINT_TO_LREAL(i)".into(),
            _ => target.into(),
        };
        let (a, b) = (operand(rng), operand(rng));
        let op = *rng.pick(&["+", "-", "*", "/"]);
        let b = if op == "/" {
            format!("{t}#{}", rng.pick(&lits))
        } else {
            b
        };
        writeln!(body, "{target} := {a} {op} {b};").unwrap();
        if rng.below(3) == 0 {
            writeln!(body, "i := {t}_TO_DINT({target});").unwrap();
        }
    }
    Case {
        decls: String::new(),
        vars,
        body,
    }
}

#[test]
fn random_programs_agree_with_rusty() {
    let Some(cmd) = rusty() else { return };
    // Deterministic by default; HELMSMAN_RUSTY_SEED / _CASES widen a sweep.
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut rng = Rng(env("HELMSMAN_RUSTY_SEED", 0x0DDB_A11C_AFE5_EED5).max(1));
    let cases: Vec<Case> = (0..env("HELMSMAN_RUSTY_CASES", 300))
        .map(|k| {
            if k % 3 == 2 {
                random_real_case(&mut rng)
            } else {
                random_int_case(&mut rng)
            }
        })
        .collect();
    // Batches keep one ruSTy compile manageable and failures local.
    for chunk in cases.chunks(100) {
        compare(&cmd, chunk);
    }
}
