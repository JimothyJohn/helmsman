//! `STRING[n]`: literals, storage, the standard functions, conversions.
//!
//! The behaviours that are the standard's own (LEFT, RIGHT, MID, CONCAT,
//! INSERT, DELETE, REPLACE, FIND on the examples of its function table, a
//! literal's `$` escapes) are pinned by the cases below and, where docker
//! runs, by `tests/oracle.rs`. The choices the standard leaves open (a
//! count below 1, a position past the end, a text that is no number,
//! truncation on assignment) are Helmsman's own and are pinned here; see
//! docs/SOURCES.md, "STRING".

use std::cell::RefCell;
use std::cmp::Ordering;

use helmsman_iec::{FaultKind, Image, ImageSizes, Program, SetError, Value, compile};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

fn build(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{src}\n{d:#?}"))
}

/// Compiles `body` in a PROGRAM with `vars` and runs one scan.
fn run(vars: &str, body: &str) -> Program {
    let mut p = build(&format!("PROGRAM t\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM"));
    let mut img = Image::new(SIZES);
    p.scan(&mut img).unwrap_or_else(|f| panic!("{f}\n{vars}\n{body}"));
    p
}

fn s(p: &Program, path: &str) -> String {
    p.get_str(path).unwrap_or_else(|| panic!("no string {path}"))
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

fn prog_errors(vars: &str, body: &str) -> String {
    errors(&format!("PROGRAM t\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM"))
}

// ---- literals and the lexer ----

#[test]
fn escapes_resolve_to_one_byte_each() {
    let p = run("a : STRING[40];", "a := 'a$$b$'c$Nd$41$7e$t$L$P$R$l$n$p$r$T';");
    assert_eq!(s(&p, "a"), "a$b'c\ndA~\t\n\x0c\r\n\n\x0c\r\t");
}

#[test]
fn hex_escapes_reach_every_byte_as_latin1() {
    let p = run("a : STRING[4];", "a := '$00$7F$80$fF';");
    assert_eq!(s(&p, "a"), "\u{0}\u{7f}\u{80}\u{ff}");
    let p = run("a : STRING[4]; n : INT;", "a := '$00x'; n := LEN(a);");
    assert_eq!(int(&p, "n"), 2, "a NUL byte is a character, not the end");
}

#[test]
fn bad_literals_are_diagnostics_at_the_quote_or_the_escape() {
    for (body, want) in [
        ("a := 'abc", "unterminated STRING literal"),
        ("a := 'ab$';", "unterminated STRING literal"),
        ("a := 'ab$", "bad $ escape"),
        ("a := '$G';", "bad $ escape"),
        ("a := '$4';", "bad $ escape: $HH takes two hex digits"),
        ("a := '$4'", "bad $ escape"),
        ("a := '$\"';", "bad $ escape"),
        ("a := 'caf\u{e9}';", "single-byte characters"),
        ("a := '\u{1f600}';", "single-byte characters"),
        ("a := 'a\tb';", "control character"),
        ("a := 'a\rb';", "unterminated STRING literal"),
        ("a := 'a\nb';", "unterminated STRING literal"),
        ("a := \"wide\";", "WSTRING literals"),
    ] {
        let e = prog_errors("a : STRING[9];", body);
        assert!(e.contains(want), "{body:?}: {e}");
    }
}

#[test]
fn a_literal_may_be_255_characters_and_not_256() {
    let ok = "x".repeat(255);
    let p = run("a : STRING[255]; n : INT;", &format!("a := '{ok}'; n := LEN(a);"));
    assert_eq!(int(&p, "n"), 255);
    let too = "x".repeat(256);
    let e = prog_errors("a : STRING[255];", &format!("a := '{too}';"));
    assert!(e.contains("at most 255 characters"), "{e}");
    // An escape counts as one character.
    let esc = "$41".repeat(255);
    run("a : STRING[255];", &format!("a := '{esc}';"));
    let esc = "$41".repeat(256);
    assert!(prog_errors("a : STRING[255];", &format!("a := '{esc}';")).contains("at most 255"));
}

#[test]
fn a_quote_inside_a_comment_is_not_a_literal() {
    run("a : STRING[4];", "(* it's fine *) a := 'ab'; // don't\n");
}

// ---- declarations ----

#[test]
fn capacities_are_one_to_255() {
    for ok in ["STRING[1]", "STRING[255]", "STRING(10)", "STRING"] {
        run(&format!("a : {ok};"), "");
    }
    for bad in ["STRING[0]", "STRING[256]", "STRING[-3]", "STRING[100000]"] {
        let e = prog_errors(&format!("a : {bad};"), "");
        assert!(e.contains("a STRING holds 1 to 255 characters"), "{bad}: {e}");
    }
    assert!(prog_errors("a : STRING[;", "").contains("expected"));
    assert!(prog_errors("a : STRING[5;", "").contains("expected ]"));
}

#[test]
fn a_bare_string_holds_80_and_wstring_is_refused() {
    let long = "y".repeat(100);
    let p = run(
        "a : STRING; b : STRING[100]; n : INT;",
        &format!("b := '{long}'; a := b; n := LEN(a);"),
    );
    assert_eq!(int(&p, "n"), 80);
    let e = prog_errors("a : WSTRING;", "");
    assert!(e.contains("WSTRING is not supported"), "{e}");
    let e = prog_errors("a : WSTRING[10];", "");
    assert!(e.contains("expected"), "{e}");
}

#[test]
fn initial_values_fit_or_are_refused() {
    let p = run(
        "a : STRING[5] := 'abc'; b : STRING[3] := ''; c : STRING[2] := 'ab';",
        "",
    );
    assert_eq!(
        (s(&p, "a"), s(&p, "b"), s(&p, "c")),
        ("abc".into(), String::new(), "ab".into())
    );
    let e = prog_errors("a : STRING[2] := 'abc';", "");
    assert!(e.contains("has 3 characters, STRING[2] holds 2"), "{e}");
    let e = prog_errors("a : STRING[5] := 7;", "");
    assert!(e.contains("must be a literal"), "{e}");
    let e = prog_errors("a : STRING[5]; b : STRING[5] := a;", "");
    assert!(e.contains("must be a literal"), "{e}");
}

#[test]
fn constants_structs_arrays_and_aliases_hold_strings() {
    let p = run(
        "TAG : STRING[3] := 'xyz'; c : STRING[8];",
        "c := CONCAT(TAG, '!');",
    );
    assert_eq!(s(&p, "c"), "xyz!");
    let src = "
TYPE Name : STRING[6] := 'anon'; Rec : STRUCT n : Name; k : STRING[3] := 'ok'; i : INT := 5; END_STRUCT; END_TYPE
PROGRAM t
VAR r : Rec; q : Rec; a : ARRAY[1..3] OF STRING[6]; m : ARRAY[0..1, 0..1] OF STRING[2] := ['a', 'b', 'c', 'd']; i : INT := 2; END_VAR
  r.n := 'robot-arm';
  q := r;
  a[2] := r.n;
  a[i + 1] := 'z';
END_PROGRAM";
    let mut p = build(src);
    p.scan(&mut Image::new(SIZES)).unwrap();
    assert_eq!(s(&p, "r.n"), "robot-");
    assert_eq!(s(&p, "q.n"), "robot-", "a struct copy copies its strings");
    assert_eq!((s(&p, "q.k"), int(&p, "q.i")), ("ok".into(), 5));
    assert_eq!(
        (s(&p, "a[1]"), s(&p, "a[2]"), s(&p, "a[3]")),
        ("".into(), "robot-".into(), "z".into())
    );
    assert_eq!(s(&p, "m[1,0]"), "c");
    assert_eq!(s(&p, "m[0, 1]"), "b");
}

// ---- assignment and truncation ----

#[test]
fn assignment_cuts_at_the_declared_length() {
    let p = run(
        "s5 : STRING[5]; s1 : STRING[1]; big : STRING[20] := 'abcdefghij';",
        "s5 := 'abcdefgh'; s1 := big; big := 'tiny'; ",
    );
    assert_eq!(
        (s(&p, "s5"), s(&p, "s1"), s(&p, "big")),
        ("abcde".into(), "a".into(), "tiny".into())
    );
    let p = run(
        "s5 : STRING[5]; big : STRING[20] := 'abcdefghij';",
        "s5 := big; big := 'xy'; s5 := big;",
    );
    assert_eq!(s(&p, "s5"), "xy", "a shorter value replaces, never merges");
}

#[test]
fn a_string_assigned_over_a_longer_one_leaves_nothing_behind() {
    let mut p = build(
        "PROGRAM t VAR a : STRING[20]; b : STRING[20]; k : INT; END_VAR
        IF k = 0 THEN a := 'a long string here'; ELSE a := 'x'; END_IF; b := a; k := k + 1; END_PROGRAM",
    );
    let mut img = Image::new(SIZES);
    p.scan(&mut img).unwrap();
    p.scan(&mut img).unwrap();
    assert_eq!(s(&p, "b"), "x");
    assert_eq!(int(&p, "k"), 2);
}

#[test]
fn a_string_is_not_a_number_nor_the_other_way_round() {
    for (vars, body, want) in [
        (
            "a : STRING[4]; i : INT;",
            "i := a;",
            "expected INT, found STRING[4]",
        ),
        (
            "a : STRING[4]; i : INT;",
            "a := i;",
            "expected STRING[4], found INT",
        ),
        ("a : STRING[4];", "a := 5;", "a number cannot be a STRING[4]"),
        ("a : STRING[4];", "a := TRUE;", "expected STRING[4], found BOOL"),
        (
            "a : STRING[4];",
            "IF a THEN a := ''; END_IF;",
            "expected BOOL, found STRING[4]",
        ),
        (
            "a : STRING[4]; i : INT;",
            "i := a + 1;",
            "cannot take a STRING and a number",
        ),
        ("a : STRING[4];", "a := a + 'x';", "join strings with CONCAT"),
        ("a : STRING[4];", "a := a * a;", "does not work on STRINGs"),
        ("a : STRING[4];", "a := -a;", "unary - needs"),
        ("a : STRING[4];", "a := NOT a;", "NOT needs"),
        (
            "a : STRING[4];",
            "CASE a OF 1: ; END_CASE;",
            "CASE selector must be",
        ),
        (
            "a : STRING[4]; x : ARRAY[0..3] OF INT;",
            "x[a] := 1;",
            "an array index must be an integer",
        ),
        ("a : STRING[4]; i : INT;", "i := a[1];", "is not an array"),
        ("a : STRING[4]; i : INT;", "i := a.3;", "bit access needs"),
        (
            "a : STRING[4]; i : INT;",
            "FOR a := 1 TO 3 DO END_FOR;",
            "FOR variable A must be",
        ),
        ("a : STRING[4];", "a := 1.5;", "cannot be a STRING[4]"),
        ("a : STRING[4];", "a := SEL(TRUE, a, a);", "cannot take"),
    ] {
        let e = prog_errors(vars, body);
        assert!(e.contains(want), "{body}: {e}");
    }
}

#[test]
fn a_string_cannot_be_located_or_a_loop_variable() {
    let e = prog_errors("a AT %MB0 : STRING[4];", "");
    assert!(e.contains("only elementary variables can be located"), "{e}");
}

// ---- comparisons ----

#[test]
fn comparison_is_by_byte_with_a_prefix_first() {
    let cases: [(&str, &str); 9] = [
        ("", ""),
        ("", "a"),
        ("a", "a"),
        ("a", "b"),
        ("ab", "a"),
        ("abc", "abd"),
        ("Z", "a"),
        ("$80", "a"),
        ("a$00", "a"),
    ];
    for (x, y) in cases {
        let p = run(
            "a : STRING[9]; b : STRING[9]; eq, ne, lt, gt, le, ge : BOOL;",
            &format!(
                "a := '{x}'; b := '{y}'; eq := a = b; ne := a <> b; lt := a < b; gt := a > b; le := a <= b; ge := a >= b;"
            ),
        );
        let (xb, yb) = (unescape(x), unescape(y));
        let o = xb.cmp(&yb);
        let b = |n: &str| p.get(n) == Some(Value::Bool(true));
        assert_eq!(
            (b("eq"), b("ne"), b("lt"), b("gt"), b("le"), b("ge")),
            (
                o == Ordering::Equal,
                o != Ordering::Equal,
                o == Ordering::Less,
                o == Ordering::Greater,
                o != Ordering::Greater,
                o != Ordering::Less
            ),
            "{x:?} vs {y:?}"
        );
    }
}

fn unescape(t: &str) -> Vec<u8> {
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            out.push(u8::from_str_radix(&t[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

#[test]
fn strings_of_different_capacity_compare_by_text() {
    let p = run(
        "a : STRING[3] := 'abc'; b : STRING[200] := 'abc'; c : STRING[200] := 'abcd'; e, f, g : BOOL;",
        "e := a = b; f := a < c; g := 'abc' = a;",
    );
    assert_eq!(
        (p.get("e"), p.get("f"), p.get("g")),
        (
            Some(Value::Bool(true)),
            Some(Value::Bool(true)),
            Some(Value::Bool(true))
        )
    );
}

// ---- the standard functions, on the standard's own examples ----

#[test]
fn the_standards_function_examples() {
    let p = run(
        "l, r, m, c, i, d, rp : STRING[20]; f, n : INT;",
        "l := LEFT('ASTR', 3); r := RIGHT('ASTR', 3); m := MID('ASTR', 2, 2);
         c := CONCAT('AB', 'CD'); i := INSERT('ABC', 'XY', 2); d := DELETE('ABCDE', 2, 2);
         rp := REPLACE('ABCDE', 'XY', 2, 3); f := FIND('ABCBC', 'BC'); n := LEN('ASTR');",
    );
    for (v, want) in [
        ("l", "AST"),
        ("r", "STR"),
        ("m", "ST"),
        ("c", "ABCD"),
        ("i", "ABXYC"),
        ("d", "ADE"),
        ("rp", "ABXYE"),
    ] {
        assert_eq!(s(&p, v), want, "{v}");
    }
    assert_eq!((int(&p, "f"), int(&p, "n")), (2, 4));
}

#[test]
fn concat_takes_two_or_more_and_nests() {
    let p = run(
        "a : STRING[30]; b : STRING[4];",
        "a := CONCAT('a', 'b', 'c', 'd', 'e', CONCAT('f', 'g')); b := CONCAT('1234', '5');",
    );
    assert_eq!((s(&p, "a"), s(&p, "b")), ("abcdefg".into(), "1234".into()));
    assert!(prog_errors("a : STRING[4];", "a := CONCAT('x');").contains("at least 2"));
    assert!(prog_errors("a : STRING[4];", "a := CONCAT('x', 1);").contains("must be a STRING"));
}

#[test]
fn calls_check_their_arguments() {
    for (body, want) in [
        ("a := LEFT(a);", "takes 2 argument(s), got 1"),
        ("a := LEFT(a, 1, 2);", "takes 2 argument(s), got 3"),
        ("a := LEFT(1, 1);", "argument 1 must be a STRING, found a number"),
        ("a := LEFT(i, 1);", "argument 1 must be a STRING, found INT"),
        ("a := LEFT(a, 'x');", "argument 2 must be an integer"),
        ("a := LEFT(a, 1.5);", "argument 2 must be an integer"),
        ("a := LEFT(a, TRUE);", "argument 2 must be an integer"),
        ("a := LEFT(IN := a, L := 1);", "takes positional arguments only"),
        ("a := LEN();", "needs arguments"),
        ("i := LEN(a, a);", "takes 1 argument(s), got 2"),
        ("a := FIND(a, a);", "expected STRING[6], found INT"),
        ("a := REPLACE(a, a, 1);", "takes 4 argument(s), got 3"),
        ("LEN(a);", "result of LEN is unused"),
    ] {
        let e = prog_errors("a : STRING[6]; i : INT;", body);
        assert!(e.contains(want), "{body}: {e}");
    }
}

#[test]
fn integer_arguments_may_be_any_integer_type_or_expression() {
    let p = run(
        "a : STRING[9] := 'abcdef'; b : USINT := 2; c : LINT := 3; r1, r2, r3 : STRING[9];",
        "r1 := LEFT(a, b); r2 := MID(a, c, b + 1); r3 := RIGHT(a, 1 + 1);",
    );
    assert_eq!(
        (s(&p, "r1"), s(&p, "r2"), s(&p, "r3")),
        ("ab".into(), "cde".into(), "ef".into())
    );
}

#[test]
fn the_boundaries_of_every_function_at_a_fixed_capacity() {
    // STRING[4]: lengths 0, 1, 3, 4 and (by truncation) 5 are all 4 or
    // fewer; counts and positions around them.
    for len in 0..=4usize {
        let text: String = "abcd".chars().take(len).collect();
        for count in [-1i64, 0, 1, 3, 4, 5, 100, i64::MAX, i64::MIN] {
            for pos in [-1i64, 0, 1, 3, 4, 5, 100, i64::MAX, i64::MIN] {
                let mut p = build(
                    "PROGRAM t VAR_INPUT l : LINT; q : LINT; END_VAR
                     VAR a : STRING[4]; r1, r2, r3, r4, r5 : STRING[8]; END_VAR
                     r1 := LEFT(a, l); r2 := RIGHT(a, l); r3 := MID(a, l, q);
                     r4 := DELETE(a, l, q); r5 := INSERT(a, 'XY', q); END_PROGRAM",
                );
                p.set_str("a", &text).unwrap();
                p.set_input("l", Value::Int(count.into())).unwrap();
                p.set_input("q", Value::Int(pos.into())).unwrap();
                p.scan(&mut Image::new(SIZES)).unwrap();
                let t = text.as_bytes();
                assert_eq!(
                    s(&p, "r1").as_bytes(),
                    model_left(t, count),
                    "LEFT {text:?} {count}"
                );
                assert_eq!(
                    s(&p, "r2").as_bytes(),
                    model_right(t, count),
                    "RIGHT {text:?} {count}"
                );
                assert_eq!(
                    s(&p, "r3").as_bytes(),
                    model_mid(t, count, pos),
                    "MID {text:?} {count} {pos}"
                );
                assert_eq!(
                    s(&p, "r4").as_bytes(),
                    model_delete(t, count, pos),
                    "DELETE {text:?} {count} {pos}"
                );
                assert_eq!(
                    s(&p, "r5").as_bytes(),
                    model_insert(t, b"XY", pos, 8),
                    "INSERT {text:?} {pos}"
                );
            }
        }
    }
}

// ---- properties against a model written for the test ----

fn model_left(a: &[u8], l: i64) -> Vec<u8> {
    a.iter()
        .take(usize::try_from(l.max(0)).unwrap_or(usize::MAX))
        .copied()
        .collect()
}

fn model_right(a: &[u8], l: i64) -> Vec<u8> {
    let l = usize::try_from(l.max(0)).unwrap_or(usize::MAX);
    let skip = a.len().saturating_sub(l);
    a.iter().skip(skip).copied().collect()
}

/// `l` characters from 1-based `p`; nothing if `p` is outside 1..=len.
fn model_mid(a: &[u8], l: i64, p: i64) -> Vec<u8> {
    if p < 1 || usize::try_from(p).map_or(true, |p| p > a.len()) {
        return Vec::new();
    }
    let start = usize::try_from(p).unwrap() - 1;
    a.iter()
        .skip(start)
        .take(usize::try_from(l.max(0)).unwrap_or(usize::MAX))
        .copied()
        .collect()
}

/// Unchanged unless `p` is in 1..=len and `l` is at least 1.
fn model_delete(a: &[u8], l: i64, p: i64) -> Vec<u8> {
    if p < 1 || l < 1 || usize::try_from(p).map_or(true, |p| p > a.len()) {
        return a.to_vec();
    }
    let start = usize::try_from(p).unwrap() - 1;
    let n = usize::try_from(l).unwrap_or(usize::MAX);
    a.iter()
        .enumerate()
        .filter(|&(i, _)| i < start || i - start >= n)
        .map(|(_, &c)| c)
        .collect()
}

/// `ins` after position `p`: first for `p` of 0 or less, last past the end;
/// the result cut at `cap`.
fn model_insert(a: &[u8], ins: &[u8], p: i64, cap: usize) -> Vec<u8> {
    let at = usize::try_from(p.max(0)).unwrap_or(usize::MAX).min(a.len());
    let mut out = a[..at].to_vec();
    out.extend_from_slice(ins);
    out.extend_from_slice(&a[at..]);
    out.truncate(cap);
    out
}

/// Unchanged unless `p` is in 1..=len+1; `l` characters from `p` are
/// replaced (none for `l` below 1).
fn model_replace(a: &[u8], ins: &[u8], l: i64, p: i64, cap: usize) -> Vec<u8> {
    if p < 1 || usize::try_from(p).map_or(true, |p| p > a.len() + 1) {
        return a[..a.len().min(cap)].to_vec();
    }
    let start = usize::try_from(p).unwrap() - 1;
    let n = usize::try_from(l.max(0)).unwrap_or(usize::MAX);
    let mut out = a[..start].to_vec();
    out.extend_from_slice(ins);
    out.extend(a.iter().skip(start).skip(n.min(a.len())).copied());
    out.truncate(cap);
    out
}

fn model_find(a: &[u8], sub: &[u8]) -> i128 {
    if sub.is_empty() {
        return 0;
    }
    (0..a.len())
        .find(|&i| a.len() - i >= sub.len() && &a[i..i + sub.len()] == sub)
        .map_or(0, |i| i as i128 + 1)
}

fn model_cmp(a: &[u8], b: &[u8]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        if x != y {
            return x.cmp(y);
        }
    }
    a.len().cmp(&b.len())
}

fn count() -> impl Strategy<Value = i64> {
    prop_oneof![-3i64..60, Just(i64::MAX), Just(i64::MIN), any::<i64>()]
}

#[test]
fn every_function_agrees_with_the_model() {
    const CAP: usize = 40;
    let p = RefCell::new(build(&format!(
        "PROGRAM t VAR_INPUT l : LINT; q : LINT; END_VAR
         VAR a : STRING[{CAP}]; b : STRING[{CAP}]; b3 : STRING[3];
           lf, rt, md, cat, ins, del, rep : STRING[{CAP}];
           rep3 : STRING[3]; ln, fnd : INT; eq, lt, le, ne, gt, ge : BOOL; END_VAR
         lf := LEFT(a, l); rt := RIGHT(a, l); md := MID(a, l, q); cat := CONCAT(a, b);
         ins := INSERT(a, b, q); del := DELETE(a, l, q); rep := REPLACE(a, b, l, q);
         rep3 := REPLACE(a, b, l, q); ln := LEN(a); fnd := FIND(a, b);
         eq := a = b; ne := a <> b; lt := a < b; le := a <= b; gt := a > b; ge := a >= b;
         END_PROGRAM"
    )));
    let mut runner = TestRunner::new(Config {
        cases: 3000,
        ..Config::default()
    });
    let text = "[ab ]{0,40}";
    runner
        .run(&(text, text, count(), count()), |(a, b, l, q)| {
            let mut p = p.borrow_mut();
            p.set_str("a", &a).unwrap();
            p.set_str("b", &b).unwrap();
            p.set_input("l", Value::Int(l.into())).unwrap();
            p.set_input("q", Value::Int(q.into())).unwrap();
            p.scan(&mut Image::new(SIZES)).unwrap();
            let (ab, bb) = (a.as_bytes(), b.as_bytes());
            let get = |n: &str| p.get_str(n).unwrap().into_bytes();
            let ctx = format!("a={a:?} b={b:?} l={l} q={q}");
            prop_assert_eq!(get("lf"), model_left(ab, l), "LEFT {}", &ctx);
            prop_assert_eq!(get("rt"), model_right(ab, l), "RIGHT {}", &ctx);
            prop_assert_eq!(get("md"), model_mid(ab, l, q), "MID {}", &ctx);
            let mut cat = ab.to_vec();
            cat.extend_from_slice(bb);
            cat.truncate(CAP);
            prop_assert_eq!(get("cat"), cat, "CONCAT {}", &ctx);
            prop_assert_eq!(get("ins"), model_insert(ab, bb, q, CAP), "INSERT {}", &ctx);
            prop_assert_eq!(get("del"), model_delete(ab, l, q), "DELETE {}", &ctx);
            prop_assert_eq!(get("rep"), model_replace(ab, bb, l, q, CAP), "REPLACE {}", &ctx);
            prop_assert_eq!(
                get("rep3"),
                model_replace(ab, bb, l, q, 3),
                "REPLACE into 3 {}",
                &ctx
            );
            prop_assert_eq!(p.get("ln"), Some(Value::Int(ab.len() as i128)), "LEN {}", &ctx);
            prop_assert_eq!(
                p.get("fnd"),
                Some(Value::Int(model_find(ab, bb))),
                "FIND {}",
                &ctx
            );
            let o = model_cmp(ab, bb);
            let want = [
                ("eq", o == Ordering::Equal),
                ("ne", o != Ordering::Equal),
                ("lt", o == Ordering::Less),
                ("le", o != Ordering::Greater),
                ("gt", o == Ordering::Greater),
                ("ge", o != Ordering::Less),
            ];
            for (n, v) in want {
                prop_assert_eq!(p.get(n), Some(Value::Bool(v)), "{} {}", n, &ctx);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_stored_string_is_canonical_whatever_was_there_before() {
    // Two programs that end in the same text hold the same slots: a hot
    // swap's equality check and `state()` depend on it.
    let mut p1 =
        build("PROGRAM t VAR a : STRING[20]; END_VAR a := 'abcdefghijklmnopqrst'; a := 'xy'; END_PROGRAM");
    let mut p2 = build("PROGRAM t VAR a : STRING[20]; END_VAR a := 'xy'; END_PROGRAM");
    p1.scan(&mut Image::new(SIZES)).unwrap();
    p2.scan(&mut Image::new(SIZES)).unwrap();
    let mut q1 = build("PROGRAM t VAR a : STRING[20]; END_VAR END_PROGRAM");
    let mut q2 = build("PROGRAM t VAR a : STRING[20]; END_VAR END_PROGRAM");
    let (c1, c2) = (q1.restore(&p1.state()), q2.restore(&p2.state()));
    assert_eq!((c1.kept, c2.kept), (vec!["A".to_string()], vec!["A".to_string()]));
    assert_eq!(q1.get_str("a"), q2.get_str("a"));
}

// ---- aliasing ----

#[test]
fn a_function_may_read_what_it_overwrites() {
    let p = run(
        "s : STRING[10] := 'abc'; t : STRING[10] := 'abcdef'; u : STRING[10] := 'abcdef'; v : STRING[10] := 'abcdef';
         w : STRING[10] := 'abcdef'; x : STRING[10] := 'abcdef'; y : STRING[10] := 'abcdef'; z : STRING[10] := 'abc';",
        "s := CONCAT(s, s); t := CONCAT(t, t); u := MID(u, 3, 2); v := INSERT(v, v, 2); w := DELETE(w, 2, 2);
         x := REPLACE(x, x, 1, 1); y := RIGHT(y, 2); z := z;",
    );
    assert_eq!(s(&p, "s"), "abcabc");
    assert_eq!(s(&p, "t"), "abcdefabcd", "cut at the capacity");
    assert_eq!(s(&p, "u"), "bcd");
    assert_eq!(s(&p, "v"), "ababcdefcd");
    assert_eq!(s(&p, "w"), "adef");
    assert_eq!(s(&p, "x"), "abcdefbcde");
    assert_eq!(s(&p, "y"), "ef");
    assert_eq!(s(&p, "z"), "abc");
}

#[test]
fn the_same_expression_twice_in_one_statement_keeps_both_results() {
    let p = run(
        "a : STRING[4] := 'abcd'; r : STRING[20];",
        "r := CONCAT(LEFT(a, 1), RIGHT(a, 1), LEFT(a, 2), MID(a, 2, 2));",
    );
    assert_eq!(s(&p, "r"), "adabbc");
}

const FUNCS: &str = "
FUNCTION Shout : STRING[6] VAR_INPUT w : STRING[3]; END_VAR
  Shout := CONCAT(w, 'abcdef');
END_FUNCTION
FUNCTION Pick : STRING[4] VAR_INPUT a : STRING[4]; b : STRING[4]; first : BOOL; END_VAR
  IF first THEN Pick := a; ELSE Pick := b; END_IF;
END_FUNCTION
FUNCTION_BLOCK Fb
  VAR_INPUT i : STRING[4]; END_VAR
  VAR_OUTPUT o : STRING[10]; END_VAR
  VAR_IN_OUT buf : STRING[10]; END_VAR
  VAR n : INT; END_VAR
  o := CONCAT(i, '!');
  buf := CONCAT(buf, i);
  n := n + 1;
END_FUNCTION_BLOCK
";

#[test]
fn arguments_by_value_are_cut_and_results_are_cut() {
    let p = run_with(
        FUNCS,
        "r1, r2 : STRING[20]; k : STRING[4];",
        "r1 := Shout('abcdef'); r2 := CONCAT(Shout('x'), Shout('yzzzz'));
         k := Pick('abcd', 'wxyz', TRUE);",
    );
    assert_eq!(s(&p, "r1"), "abcabc", "the argument is cut at 3, the result at 6");
    assert_eq!(
        s(&p, "r2"),
        "xabcdeyzzabc"
    );
    assert_eq!(s(&p, "k"), "abcd");
}

#[test]
fn two_calls_of_one_function_in_one_expression_do_not_overwrite_each_other() {
    let p = run_with(
        FUNCS,
        "r : STRING[20]; same : BOOL;",
        "r := CONCAT(Pick('1111', '2222', TRUE), Pick('1111', '2222', FALSE), Pick('3333', '4444', TRUE));
         same := Pick('a', 'b', TRUE) = Pick('a', 'b', TRUE);",
    );
    assert_eq!(s(&p, "r"), "111122223333");
    assert_eq!(p.get("same"), Some(Value::Bool(true)));
    let p = run_with(
        FUNCS,
        "lt : BOOL;",
        "lt := Pick('a', 'b', TRUE) < Pick('a', 'b', FALSE);",
    );
    assert_eq!(p.get("lt"), Some(Value::Bool(true)));
}

#[test]
fn a_function_block_takes_strings_in_out_and_out() {
    let p = run_with(
        FUNCS,
        "f : Fb; shared : STRING[10] := 'x'; got : STRING[10]; arr : ARRAY[0..1] OF STRING[10];",
        "f(i := 'abcdefg', buf := shared); got := f.o; f(i := 'q', buf := arr[1]);",
    );
    assert_eq!(s(&p, "shared"), "xabcd");
    assert_eq!(s(&p, "got"), "abcd!");
    assert_eq!(s(&p, "arr[1]"), "q");
    assert_eq!(s(&p, "f.o"), "q!");
    assert_eq!(s(&p, "f.i"), "q");
}

#[test]
fn an_in_out_string_has_exactly_its_own_capacity_and_is_never_two_names() {
    let e = errors(&format!(
        "{FUNCS} PROGRAM t VAR f : Fb; small : STRING[5]; END_VAR f(i := 'a', buf := small); END_PROGRAM"
    ));
    assert!(e.contains("a VAR_IN_OUT takes exactly its own type"), "{e}");
    let two = "FUNCTION_BLOCK Two VAR_IN_OUT a : STRING[5]; b : STRING[5]; END_VAR a := b; b := 'x'; END_FUNCTION_BLOCK
        PROGRAM t VAR f : Two; s : STRING[5]; END_VAR f(a := s, b := s); END_PROGRAM";
    assert!(errors(two).contains("two names for the same variable"));
}

#[test]
fn an_output_string_binds_to_a_string_of_the_same_length_only() {
    let src = |dst: &str| {
        format!(
            "{FUNCS} PROGRAM t VAR f : Fb; shared : STRING[10]; d : {dst}; END_VAR f(i := 'a', buf := shared, o => d); END_PROGRAM"
        )
    };
    let mut p = build(&src("STRING[10]"));
    p.scan(&mut Image::new(SIZES)).unwrap();
    assert_eq!(s(&p, "d"), "a!");
    assert!(errors(&src("STRING[5]")).contains("output O is STRING[10], the target is STRING[5]"));
}

fn run_with(units: &str, vars: &str, body: &str) -> Program {
    let mut p = build(&format!(
        "{units}\nPROGRAM t\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM"
    ));
    p.scan(&mut Image::new(SIZES)).unwrap_or_else(|f| panic!("{f}"));
    p
}

#[test]
fn globals_and_externals_hold_strings() {
    let src = "VAR_GLOBAL g : STRING[6] := 'glob'; END_VAR
        FUNCTION touch : INT VAR_EXTERNAL g : STRING[6]; END_VAR g := CONCAT(g, 'al'); touch := LEN(g); END_FUNCTION
        PROGRAM t VAR n : INT; END_VAR n := touch(); END_PROGRAM";
    let mut p = build(src);
    p.scan(&mut Image::new(SIZES)).unwrap();
    assert_eq!(s(&p, "g"), "global");
    assert_eq!(int(&p, "n"), 6);
}

// ---- conversions ----

#[test]
fn numbers_become_text() {
    let p = run(
        "a, b, c, d, e, f, g, h : STRING[30]; x : REAL := 23.7; y : LREAL := 0.1; z : ULINT := 18446744073709551615;
         sm : LINT := -9223372036854775808;",
        "a := INT_TO_STRING(-42); b := BOOL_TO_STRING(TRUE); c := REAL_TO_STRING(x); d := TO_STRING(y);
         e := ULINT_TO_STRING(z); f := TO_STRING(7); g := LINT_TO_STRING(sm); h := TO_STRING(FALSE);",
    );
    assert_eq!(
        ["a", "b", "c", "d", "e", "f", "g", "h"].map(|v| s(&p, v)),
        [
            "-42",
            "TRUE",
            "23.7",
            "0.1",
            "18446744073709551615",
            "7",
            "-9223372036854775808",
            "FALSE"
        ]
    );
}

#[test]
fn text_becomes_numbers_or_zero() {
    let src = |body: &str| {
        format!("a : STRING[30]; i : INT; li : LINT; u : USINT; r : REAL; lr : LREAL; b : BOOL; {body}")
    };
    for (text, want_i) in [
        ("123", 123),
        ("-5", -5),
        ("+7", 7),
        ("0", 0),
        ("", 0),
        // Spaces around and a fraction or exponent are read and cut toward
        // zero, as ruSTy reads them (tests/oracle.rs pins each).
        (" 5", 5),
        ("5 ", 5),
        ("1 2", 0),
        ("12a", 0),
        ("1.5", 1),
        ("-1.9", -1),
        ("1e2", 100),
        ("1.", 0),
        (".5", 0),
        ("1.5.5", 0),
        ("40000.5", 0),
        ("--1", 0),
        ("-", 0),
        ("+", 0),
        ("32767", 32767),
        ("32768", 0),
        ("-32768", -32768),
        ("-32769", 0),
        ("99999999999999999999999999", 0),
        ("0x10", 0),
        ("1_0", 0),
    ] {
        let p = run(&src(""), &format!("a := '{text}'; i := STRING_TO_INT(a);"));
        assert_eq!(int(&p, "i"), want_i, "STRING_TO_INT({text:?})");
    }
    let p = run(
        &src(""),
        "a := '255'; u := STRING_TO_USINT(a); li := STRING_TO_LINT(a);",
    );
    assert_eq!((int(&p, "u"), int(&p, "li")), (255, 255));
    let p = run(&src(""), "a := '256'; u := STRING_TO_USINT(a);");
    assert_eq!(int(&p, "u"), 0);
    for (text, want) in [
        ("2.5", 2.5),
        ("-2.5e1", -25.0),
        ("1E2", 100.0),
        (".5", 0.5),
        ("5.", 5.0),
        ("inf", 0.0),
        ("NaN", 0.0),
        ("infinity", 0.0),
        ("1e999", 0.0),
        ("", 0.0),
        ("1,5", 0.0),
        ("e", 0.0),
        ("1e", 0.0),
    ] {
        let p = run(&src(""), &format!("a := '{text}'; lr := STRING_TO_LREAL(a);"));
        assert_eq!(p.get("lr"), Some(Value::Real(want)), "STRING_TO_LREAL({text:?})");
    }
    let p = run(&src(""), "a := '0.1'; r := STRING_TO_REAL(a);");
    assert_eq!(
        p.get("r"),
        Some(Value::Real(f64::from(0.1f32))),
        "REAL rounds to f32"
    );
    for (text, want) in [
        ("TRUE", true),
        ("true", true),
        ("TrUe", true),
        ("1", true),
        ("FALSE", false),
        ("0", false),
        ("yes", false),
        ("", false),
        ("2", false),
    ] {
        let p = run(&src(""), &format!("a := '{text}'; b := STRING_TO_BOOL(a);"));
        assert_eq!(p.get("b"), Some(Value::Bool(want)), "STRING_TO_BOOL({text:?})");
    }
}

#[test]
fn conversions_refuse_what_they_cannot_do() {
    for (body, want) in [
        ("a := TIME_TO_STRING(T#1s);", "TIME has no text form"),
        ("a := TO_STRING(T#1s);", "TIME has no text form"),
        ("i := STRING_TO_TIME(a);", "TIME has no text form"),
        ("a := INT_TO_STRING(a);", "expected INT, found STRING[8]"),
        ("a := TO_STRING(a);", "cannot convert STRING[8]"),
        ("i := STRING_TO_INT(i);", "needs a STRING, found INT"),
        ("i := STRING_TO_INT(1);", "needs a STRING, found a number"),
        ("i := STRING_TO_INT(a, a);", "takes 1 argument"),
        ("i := TO_INT(a);", "cannot convert STRING[8]"),
        ("a := INT_TO_STRING(70000);", "does not fit INT"),
        ("a := FOO_TO_STRING(1);", "unknown function"),
        ("i := STRING_TO_FOO(a);", "unknown function"),
    ] {
        let e = prog_errors("a : STRING[8]; i : INT;", body);
        assert!(e.contains(want), "{body}: {e}");
    }
}

proptest! {
    #[test]
    fn integers_round_trip_through_text(x: i64, u: u64, w: i16, b: u8) {
        let mut p = build(
            "PROGRAM t VAR_INPUT x : LINT; u : ULINT; w : INT; b : BYTE; END_VAR
             VAR tx, tu, tw, tb : STRING[24]; rx : LINT; ru : ULINT; rw : INT; rb : BYTE; END_VAR
             tx := LINT_TO_STRING(x); tu := ULINT_TO_STRING(u); tw := INT_TO_STRING(w); tb := BYTE_TO_STRING(b);
             rx := STRING_TO_LINT(tx); ru := STRING_TO_ULINT(tu); rw := STRING_TO_INT(tw); rb := STRING_TO_BYTE(tb);
             END_PROGRAM",
        );
        p.set_input("x", Value::Int(x.into())).unwrap();
        p.set_input("u", Value::Int(u.into())).unwrap();
        p.set_input("w", Value::Int(w.into())).unwrap();
        p.set_input("b", Value::Int(b.into())).unwrap();
        p.scan(&mut Image::new(SIZES)).unwrap();
        prop_assert_eq!(p.get_str("tx").unwrap(), x.to_string());
        prop_assert_eq!(p.get_str("tu").unwrap(), u.to_string());
        prop_assert_eq!(p.get("rx"), Some(Value::Int(x.into())));
        prop_assert_eq!(p.get("ru"), Some(Value::Int(u.into())));
        prop_assert_eq!(p.get("rw"), Some(Value::Int(w.into())));
        prop_assert_eq!(p.get("rb"), Some(Value::Int(b.into())));
    }

    #[test]
    fn reals_round_trip_through_text(x in any::<f32>().prop_filter("finite", |x| x.is_finite()), y in any::<f64>().prop_filter("finite", |y| y.is_finite())) {
        let mut p = build(
            "PROGRAM t VAR_INPUT x : REAL; y : LREAL; END_VAR
             VAR tx, ty : STRING[30]; rx : REAL; ry : LREAL; END_VAR
             tx := REAL_TO_STRING(x); ty := LREAL_TO_STRING(y); rx := STRING_TO_REAL(tx); ry := STRING_TO_LREAL(ty);
             END_PROGRAM",
        );
        p.set_input("x", Value::Real(f64::from(x))).unwrap();
        p.set_input("y", Value::Real(y)).unwrap();
        p.scan(&mut Image::new(SIZES)).unwrap();
        // -0.0 and subnormal text still read back to the same bits.
        prop_assert_eq!(p.get("rx"), Some(Value::Real(f64::from(x))), "{}", p.get_str("tx").unwrap());
        prop_assert_eq!(p.get("ry"), Some(Value::Real(y)), "{}", p.get_str("ty").unwrap());
    }

    /// Whatever text a string holds, converting it never panics and the
    /// result is a number the type holds.
    #[test]
    fn any_text_converts_without_panic(text in "[ -~]{0,30}") {
        let mut p = build(
            "PROGRAM t VAR a : STRING[30]; i : SINT; r : REAL; b : BOOL; END_VAR
             i := STRING_TO_SINT(a); r := STRING_TO_REAL(a); b := STRING_TO_BOOL(a); END_PROGRAM",
        );
        p.set_str("a", &text).unwrap();
        p.scan(&mut Image::new(SIZES)).unwrap();
        match p.get("i") {
            Some(Value::Int(i)) => prop_assert!((-128..=127).contains(&i)),
            other => prop_assert!(false, "{other:?}"),
        }
        match p.get("r") {
            Some(Value::Real(r)) => prop_assert!(r.is_finite()),
            other => prop_assert!(false, "{other:?}"),
        }
    }
}

// ---- the budget ----

#[test]
fn building_strings_in_a_loop_meets_the_watchdog() {
    let mut p = build(
        "PROGRAM t VAR s : STRING[255]; n : DINT; END_VAR WHILE TRUE DO s := CONCAT(s, 'x'); s := LEFT(s, 100); n := n + 1; END_WHILE; END_PROGRAM",
    );
    let fault = p.scan(&mut Image::new(SIZES)).unwrap_err();
    assert_eq!(fault.kind, FaultKind::Watchdog);
    // String work is charged by the byte: far fewer passes than the
    // statements alone would allow in the same budget.
    let Some(Value::Int(n)) = p.get("n") else { panic!() };
    assert!(n > 0 && n < 200_000, "{n}");
    assert_eq!(p.get_str("s").unwrap().len(), 100);
}

#[test]
fn a_tiny_budget_stops_a_long_string_copy() {
    let mut p = build("PROGRAM t VAR a : STRING[255]; b : STRING[255] := '$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41$41'; END_VAR a := b; b := a; END_PROGRAM");
    p.set_budget(3);
    assert_eq!(
        p.scan(&mut Image::new(SIZES)).unwrap_err().kind,
        FaultKind::Watchdog
    );
}

// ---- the host's side ----

#[test]
fn set_str_and_get_str_refuse_what_does_not_fit() {
    let mut p = build("PROGRAM t VAR a : STRING[4]; i : INT; END_VAR END_PROGRAM");
    assert_eq!(p.set_str("a", "abcd"), Ok(()));
    assert_eq!(p.set_str("a", "abcde"), Err(SetError::TooLong(4)));
    assert_eq!(p.set_str("a", "caf\u{e9}"), Err(SetError::NotAscii));
    assert_eq!(p.set_str("i", "1"), Err(SetError::NoSuchVariable));
    assert_eq!(p.set_str("nope", "1"), Err(SetError::NoSuchVariable));
    assert_eq!(
        p.get_str("a").as_deref(),
        Some("abcd"),
        "a refused write changes nothing"
    );
    assert_eq!(p.get_str("i"), None);
    assert_eq!(p.get("a"), None, "a string is no scalar");
    p.reset();
    assert_eq!(p.get_str("a").as_deref(), Some(""));
}

#[test]
fn a_string_port_is_skipped_by_ports_on_purpose() {
    let p = build(
        "PROGRAM t VAR_INPUT name : STRING[8]; n : INT; END_VAR VAR_OUTPUT msg : STRING[20]; ok : BOOL; END_VAR
         msg := CONCAT('hi ', name); END_PROGRAM",
    );
    let names: Vec<String> = p.ports().into_iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        ["N", "OK"],
        "STRING ports compile but are no signals yet (st-string-signals)"
    );
    let mut p = p;
    assert_eq!(p.set_input("name", Value::Int(0)), Err(SetError::NoSuchInput));
    p.set_str("name", "bob").unwrap();
    p.scan(&mut Image::new(SIZES)).unwrap();
    assert_eq!(p.get_str("msg").as_deref(), Some("hi bob"));
}

#[test]
fn retain_on_a_string_is_refused_and_names_the_follow_up() {
    for vars in [
        "VAR RETAIN s : STRING[4]; END_VAR",
        "TYPE R : STRUCT s : STRING[4]; END_STRUCT; END_TYPE VAR RETAIN r : R; END_VAR",
        "VAR RETAIN a : ARRAY[0..1] OF STRING[4]; END_VAR",
    ] {
        let (types, vars) = match vars.strip_prefix("TYPE") {
            Some(rest) => {
                let (t, v) = rest.split_once("END_TYPE").unwrap();
                (format!("TYPE{t}END_TYPE"), v.to_string())
            }
            None => (String::new(), vars.to_string()),
        };
        let e = errors(&format!("{types} PROGRAM t {vars} END_PROGRAM"));
        assert!(
            e.contains("cannot be RETAIN yet") && e.contains("st-string-signals"),
            "{vars}: {e}"
        );
    }
    // Elsewhere RETAIN stays a warning, strings or not.
    build(
        "FUNCTION_BLOCK F VAR RETAIN s : STRING[4]; END_VAR END_FUNCTION_BLOCK PROGRAM t VAR f : F; END_VAR END_PROGRAM",
    );
}

#[test]
fn a_hot_swap_carries_a_string_only_into_the_same_capacity() {
    let mut old = build(
        "PROGRAM t VAR a : STRING[8]; b : STRING[8]; c : INT; END_VAR a := 'kept'; b := 'cut'; END_PROGRAM",
    );
    old.scan(&mut Image::new(SIZES)).unwrap();
    let mut new = build("PROGRAM t VAR a : STRING[8]; b : STRING[4]; c : STRING[8]; END_VAR END_PROGRAM");
    let carry = new.restore(&old.state());
    assert_eq!(new.get_str("a").as_deref(), Some("kept"));
    assert_eq!(
        new.get_str("b").as_deref(),
        Some(""),
        "another capacity starts fresh"
    );
    assert_eq!(
        new.get_str("c").as_deref(),
        Some(""),
        "a string never takes an integer's place"
    );
    assert_eq!(carry.kept, ["A"]);
    assert_eq!(carry.reset, ["B", "C"]);
}

#[test]
fn a_state_with_a_string_encodes_and_decodes() {
    let mut p = build("PROGRAM t VAR a : STRING[8]; END_VAR a := 'abc'; END_PROGRAM");
    p.scan(&mut Image::new(SIZES)).unwrap();
    let bytes = p.state().encode().unwrap();
    let st = helmsman_iec::State::decode(&bytes).unwrap();
    let mut q = build("PROGRAM t VAR a : STRING[8]; END_VAR END_PROGRAM");
    q.restore(&st);
    assert_eq!(q.get_str("a").as_deref(), Some("abc"));
    // A forged state whose string runs past its capacity is not carried.
    let mut bad = bytes.clone();
    let n = bad.len();
    bad[n - 17] = 0xFF; // a value's low byte: the length byte of the string
    if let Ok(st) = helmsman_iec::State::decode(&bad) {
        let mut r = build("PROGRAM t VAR a : STRING[8]; END_VAR END_PROGRAM");
        let carry = r.restore(&st);
        assert!(carry.kept.is_empty(), "{carry:?}");
        assert_eq!(r.get_str("a").as_deref(), Some(""));
    }
}

#[test]
fn the_trace_shows_strings_beside_the_scalars() {
    let mut p = build(
        "TYPE R : STRUCT n : STRING[5]; k : INT; END_STRUCT; END_TYPE
         PROGRAM t VAR a : STRING[8]; r : R; arr : ARRAY[0..1] OF STRING[3]; i : INT; END_VAR
         a := 'abc'; r.n := 'hello world'; arr[1] := 'xyz!'; i := 1; END_PROGRAM",
    );
    let (r, trace) = p.scan_traced(&mut Image::new(SIZES), std::time::Duration::ZERO);
    r.unwrap();
    assert_eq!(trace.text("a").as_deref(), Some("abc"));
    assert_eq!(trace.text("R.N").as_deref(), Some("hello"));
    assert_eq!(trace.text("arr[1]").as_deref(), Some("xyz"));
    assert_eq!(trace.text("i"), None);
    let paths: Vec<&str> = trace.map().string_paths().collect();
    assert_eq!(paths, ["A", "R.N", "ARR[0]", "ARR[1]"]);
    // Strings are not scalars: the scalar list is what it was.
    let scalars: Vec<&str> = trace.map().vars().map(|(p, _)| p).collect();
    assert_eq!(scalars, ["R.K", "I"]);
}

#[test]
fn a_user_function_may_still_be_named_like_a_standard_string_function() {
    let p = run_with(
        "FUNCTION LEFT : INT VAR_INPUT x : INT; END_VAR LEFT := x * 2; END_FUNCTION",
        "n : INT;",
        "n := LEFT(21);",
    );
    assert_eq!(int(&p, "n"), 42);
}

// ---- hostile source ----

proptest! {
    /// Any text in or around a literal never panics the compiler, and
    /// whatever compiles scans.
    #[test]
    fn literal_soup_never_panics(body in "[a-zA-Z0-9 $'\"\\n\\t\\\\.,;:=()+\\-<>*]{0,60}") {
        let src = format!("PROGRAM t VAR a : STRING[10]; i : INT; END_VAR a := {body}; END_PROGRAM");
        if let Ok(mut p) = compile(&src, SIZES) {
            p.set_budget(10_000);
            let _ = p.scan(&mut Image::new(SIZES));
        }
    }

    /// A literal of any printable text round-trips exactly (the quote and
    /// the dollar written as escapes).
    #[test]
    fn a_literal_holds_what_was_written(text in "[ -~]{0,100}") {
        let lit = text.replace('$', "$$").replace('\'', "$'");
        let src = format!("PROGRAM t VAR a : STRING[100]; END_VAR a := '{lit}'; END_PROGRAM");
        let mut p = compile(&src, SIZES).unwrap();
        p.scan(&mut Image::new(SIZES)).unwrap();
        prop_assert_eq!(p.get_str("a").unwrap(), text);
    }

    /// Bytes that are not text at all (any char, so any UTF-8) are
    /// refused or taken, never a panic.
    #[test]
    fn arbitrary_characters_in_a_literal_never_panic(text in "\\PC{0,20}") {
        let src = format!("PROGRAM t VAR a : STRING[100]; END_VAR a := '{text}'; END_PROGRAM");
        let _ = compile(&src, SIZES);
    }
}
