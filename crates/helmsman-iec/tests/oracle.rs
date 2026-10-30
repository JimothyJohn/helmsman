//! Differential test against ruSTy (PLC-lang/rusty), an independent IEC
//! 61131-3 compiler: the same programs run through both, and every variable
//! must agree after the same number of scans.
//!
//! Runs when `HELMSMAN_RUSTY` holds the command that reads ST on stdin,
//! compiles and runs it with ruSTy, e.g.
//! `HELMSMAN_RUSTY="docker run --rm -i helmsman/rusty:1.0.7"`
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
    )
}

/// printf conversion for a scalar type (C default argument promotion).
fn fmt(ty: &str) -> &'static str {
    match ty {
        "LINT" => "%lld",
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

/// ruSTy 1.0.7 compares UDINT and ULINT as signed: `ULINT_max > 1` is
/// FALSE there. Ours is right; this pins both facts so a ruSTy fix (and the
/// generator workaround in `cond`) can be noticed and dropped.
#[test]
fn rusty_unsigned_compare_bug() {
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
        ["0".to_string(), "0".to_string()],
        "ruSTy fixed its unsigned compare: drop the workaround"
    );
}

#[test]
fn corpus_agrees_with_rusty() {
    let Some(cmd) = rusty() else { return };
    compare(&cmd, &corpus());
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

/// A condition involving a `ty` value. ruSTy 1.0.7 compares UDINT and ULINT
/// as signed (see `rusty_unsigned_compare_bug`), so conditions on those
/// types test a signed variable instead.
fn cond(rng: &mut Rng, ty: &str, lo: i128, hi: i128) -> String {
    if ty == "UDINT" || ty == "ULINT" {
        format!("v_dint < {}", lit(rng, "DINT", -2_147_483_648, 2_147_483_647))
    } else {
        format!("v_{} < {}", ty.to_lowercase(), lit(rng, ty, lo, hi))
    }
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
