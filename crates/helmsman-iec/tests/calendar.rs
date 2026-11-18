//! DATE, TOD and DT: literals, comparison, arithmetic with TIME, the
//! conversions, the text forms and the wall clock.
//!
//! The shape of the types (64-bit nanoseconds, DATE and DT from
//! 1970-01-01, TOD from midnight) and the arithmetic table are ruSTy's,
//! pinned against it by `tests/oracle.rs` (`calendar_agrees_with_rusty`)
//! where docker runs. The rest (the refusals, the text forms, the wall
//! clock, what is out of range) is Helmsman's and is pinned here; see
//! docs/SOURCES.md, "DATE, TOD and DT". The expected values below come
//! from a calendar written independently of `dt.rs`: a loop over years
//! and months, and Zeller's congruence for the weekday.

use core::time::Duration;

use helmsman_iec::{FaultKind, Image, ImageSizes, Program, Ty, Value, compile};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };
const SEC: i128 = 1_000_000_000;
const DAY: i128 = 86_400 * SEC;

fn build(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{src}\n{d:#?}"))
}

fn run(vars: &str, body: &str) -> Program {
    let mut p = build(&format!("PROGRAM t\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM"));
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::ZERO)
        .unwrap_or_else(|f| panic!("{f}\n{vars}\n{body}"));
    p
}

fn int(p: &Program, path: &str) -> i128 {
    match p.get(path) {
        Some(Value::Int(i)) => i,
        other => panic!("{path} = {other:?}"),
    }
}

fn b(p: &Program, path: &str) -> bool {
    match p.get(path) {
        Some(Value::Bool(x)) => x,
        other => panic!("{path} = {other:?}"),
    }
}

fn s(p: &Program, path: &str) -> String {
    p.get_str(path).unwrap_or_else(|| panic!("no string {path}"))
}

fn errors(vars: &str, body: &str) -> String {
    let src = format!("PROGRAM t\nVAR\n{vars}\nEND_VAR\n{body}\nEND_PROGRAM");
    match compile(&src, SIZES) {
        Ok(_) => panic!("compiled: {src}"),
        Err(d) => d.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
    }
}

// ---- an independent calendar ----

fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn month_len(y: i64, m: i64) -> i64 {
    [
        31,
        if leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ][usize::try_from(m - 1).unwrap()]
}

/// Days from 1970-01-01 by counting.
fn days(y: i64, m: i64, d: i64) -> i128 {
    let mut n: i64 = 0;
    if y >= 1970 {
        for yr in 1970..y {
            n += if leap(yr) { 366 } else { 365 };
        }
    } else {
        for yr in y..1970 {
            n -= if leap(yr) { 366 } else { 365 };
        }
    }
    for mo in 1..m {
        n += month_len(y, mo);
    }
    i128::from(n + d - 1)
}

/// 0 = Sunday, by Zeller's congruence.
fn zeller(y: i64, m: i64, d: i64) -> i128 {
    let (yy, mm) = if m < 3 { (y - 1, m + 12) } else { (y, m) };
    let (k, j) = (yy % 100, yy / 100);
    let h = (d + 13 * (mm + 1) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    i128::from((h + 6) % 7)
}

fn tod_ns(h: i128, m: i128, sec: i128, nanos: i128) -> i128 {
    ((h * 60 + m) * 60 + sec) * SEC + nanos
}

// ---- literals ----

#[test]
fn literals_are_nanoseconds_from_the_epoch() {
    let p = run(
        "d : DATE := D#1970-01-01; d2 : DATE := DATE#2024-02-29; t : TOD := TOD#08:30:15.25; \
         t2 : TOD := TIME_OF_DAY#23:59:59.999999999; x : DT := DT#2024-02-29-08:30:00; \
         x2 : DT := DATE_AND_TIME#1969-12-31-23:59:59; z : DATE; zt : TOD; zx : DT;",
        "",
    );
    assert_eq!(int(&p, "d"), 0);
    assert_eq!(int(&p, "d2"), days(2024, 2, 29) * DAY);
    assert_eq!(int(&p, "t"), tod_ns(8, 30, 15, 250_000_000));
    assert_eq!(int(&p, "t2"), DAY - 1);
    assert_eq!(int(&p, "x"), days(2024, 2, 29) * DAY + tod_ns(8, 30, 0, 0));
    assert_eq!(int(&p, "x2"), -SEC);
    assert_eq!(
        (int(&p, "z"), int(&p, "zt"), int(&p, "zx")),
        (0, 0, 0),
        "zero is the epoch and midnight"
    );
}

#[test]
fn literals_are_case_insensitive_and_may_sit_in_expressions() {
    let p = run(
        "a : BOOL; t : TOD;",
        "a := d#2000-01-01 < Date#2000-01-02; t := tod#1:2:3;",
    );
    assert!(b(&p, "a"));
    assert_eq!(int(&p, "t"), tod_ns(1, 2, 3, 0));
}

#[test]
fn the_ends_of_the_range() {
    let p = run(
        "lo : DATE := D#1677-09-22; hi : DATE := D#2262-04-11; \
         dlo : DT := DT#1677-09-21-00:12:43.145224192; dhi : DT := DT#2262-04-11-23:47:16.854775807;",
        "",
    );
    assert_eq!(int(&p, "lo"), days(1677, 9, 22) * DAY);
    assert_eq!(int(&p, "hi"), days(2262, 4, 11) * DAY);
    assert_eq!(int(&p, "dlo"), i128::from(i64::MIN));
    assert_eq!(int(&p, "dhi"), i128::from(i64::MAX));
}

#[test]
fn hostile_literals_are_refused_with_the_reason() {
    for (lit, want) in [
        ("D#2024-02-30", "no such day"),
        ("D#2023-02-29", "no such day"),
        ("D#1900-02-29", "no such day"),
        ("D#2024-13-01", "month"),
        ("D#2024-00-01", "month"),
        ("D#2024-01-00", "no such day"),
        ("D#2024-1", "a date is"),
        ("D#24-01-01", "a date is"),
        ("D#2024/01/01", "a date is"),
        ("D#-2024-01-01", "a date is"),
        ("D#", "a date is"),
        ("D#20240101", "a date is"),
        ("D#1677-09-21", "beyond the range"),
        ("D#2262-04-12", "beyond the range"),
        ("D#0001-01-01", "beyond the range"),
        ("D#9999-12-31", "beyond the range"),
        ("D#10000-01-01", "a date is"),
        ("D#2024-01-01-10:00:00", "a date is"),
        ("TOD#24:00:00", "hour"),
        ("TOD#23:60:00", "minutes"),
        ("TOD#23:59:60", "minutes"),
        ("TOD#12:00", "a time of day is"),
        ("TOD#12", "a time of day is"),
        ("TOD#12:00:00.", "a time of day is"),
        ("TOD#12:00:00.1234567890", "at most 9 digits"),
        ("TOD#-1:00:00", "a time of day is"),
        ("TOD#abc", "a time of day is"),
        ("TOD#", "a time of day is"),
        ("TOD#123:00:00", "a time of day is"),
        ("DT#2024-01-01", "a date and time is"),
        ("DT#2024-01-01-", "a time of day is"),
        ("DT#2024-01-01-25:00:00", "hour"),
        ("DT#2024-02-30-10:00:00", "no such day"),
        ("DT#2262-04-11-23:47:16.854775808", "beyond the range"),
        ("DT#1677-09-21-00:12:43.145224191", "beyond the range"),
        ("DT#2024-01-0110:00:00", "a date and time is"),
        ("LDT#2024-01-01-10:00:00", "not supported"),
        ("LDATE#2024-01-01", "not supported"),
        ("LTOD#10:00:00", "not supported"),
    ] {
        let kind = if lit.starts_with("TOD") || lit.starts_with("LTOD") {
            "TOD"
        } else if lit.starts_with("DT") || lit.starts_with("LDT") {
            "DT"
        } else {
            "DATE"
        };
        let e = errors(&format!("v : {kind};"), &format!("v := {lit};"));
        assert!(e.contains(want), "{lit}: wanted {want:?} in\n{e}");
    }
}

#[test]
fn an_endless_literal_is_refused_not_scanned_forever() {
    let long = "1".repeat(100_000);
    let e = errors("v : DATE;", &format!("v := D#{long};"));
    assert!(e.contains("too long"), "{e}");
}

// ---- types ----

#[test]
fn the_long_names_are_the_same_types() {
    let p = run(
        "a : TIME_OF_DAY := TOD#01:00:00; b : TOD; c : DATE_AND_TIME := DT#1970-01-02-00:00:00; d : DT;",
        "b := a; d := c;",
    );
    assert_eq!(int(&p, "b"), tod_ns(1, 0, 0, 0));
    assert_eq!(int(&p, "d"), DAY);
    assert_eq!(Ty::from_name("TIME_OF_DAY"), Some(Ty::Tod));
    assert_eq!(Ty::from_name("DATE_AND_TIME"), Some(Ty::Dt));
    assert_eq!(Ty::Tod.name(), "TOD");
}

#[test]
fn ltod_and_friends_are_refused_as_types() {
    for t in ["LDATE", "LTOD", "LDT", "LTIME"] {
        let e = errors(&format!("v : {t};"), "");
        assert!(e.contains("not supported yet"), "{t}: {e}");
    }
}

#[test]
fn they_are_no_numbers() {
    for (vars, body, want) in [
        ("d : DATE;", "d := 5;", "cannot be a DATE"),
        ("t : TOD;", "t := 0;", "cannot be a TOD"),
        ("x : DT; n : DINT;", "n := x;", "expected DINT, found DT"),
        ("x : DT; n : LINT;", "x := n;", "expected DT, found LINT"),
        ("t : TOD; i : TIME;", "t := i;", "expected TOD, found TIME"),
        ("t : TOD; i : TIME;", "i := t;", "expected TIME, found TOD"),
        ("d : DATE; x : DT;", "x := d;", "expected DT, found DATE"),
        ("d : DATE; x : DT;", "d := x;", "expected DATE, found DT"),
        ("d : DATE; n : DINT;", "n := DATE_TO_DINT(d);", "does not convert"),
        ("d : DATE; n : DINT;", "d := DINT_TO_DATE(n);", "does not convert"),
        ("d : DATE; n : DINT;", "d := TO_DATE(n);", "does not convert"),
        ("t : TOD; i : TIME;", "t := TIME_TO_TOD(i);", "does not convert"),
        ("t : TOD; i : TIME;", "i := TOD_TO_TIME(t);", "does not convert"),
        ("t : TOD; x : DT;", "x := TOD_TO_DT(t);", "does not convert"),
        (
            "d : DATE; x : DT;",
            "d := DT_TO_TOD(x);",
            "expected DATE, found TOD",
        ),
        ("t : TOD;", "t := -t;", "unary -"),
        ("t : TOD;", "t := NOT t;", "NOT needs"),
        ("t : TOD; b : BOOL;", "b := t AND t;", "BOOL or a bit string"),
        ("t : TOD;", "t := t * 2;", "needs a numeric type"),
        ("t : TOD;", "t := t / 2;", "needs a numeric type"),
        ("d : DATE;", "d := d MOD 2;", "needs an integer type"),
        ("t : TOD;", "t := t ** 2;", "REAL or LREAL"),
        ("d : DATE;", "d := ABS(d);", "ABS needs a number"),
        ("d : DATE; n : DINT;", "n := d.0;", ""),
    ] {
        let e = errors(vars, body);
        assert!(e.contains(want), "{body}: wanted {want:?} in\n{e}");
    }
}

// ---- comparison ----

#[test]
fn comparison_follows_the_time_line() {
    let p = run(
        "a : DT := DT#1969-12-31-23:59:59; z : DT := DT#1970-01-01-00:00:00; \
         lt : BOOL; ge : BOOL; eq : BOOL; ne : BOOL; le : BOOL; gt : BOOL;",
        "lt := a < z; ge := a >= z; eq := a = z; ne := a <> z; le := a <= a; gt := z > a;",
    );
    assert!(b(&p, "lt") && b(&p, "ne") && b(&p, "le") && b(&p, "gt"));
    assert!(!b(&p, "ge") && !b(&p, "eq"));
}

#[test]
fn only_like_types_compare() {
    for (vars, body) in [
        ("d : DATE; x : DT; r : BOOL;", "r := d < x;"),
        ("t : TOD; i : TIME; r : BOOL;", "r := t < i;"),
        ("t : TOD; r : BOOL;", "r := t < 5;"),
        ("d : DATE; r : BOOL;", "r := d = 0;"),
        ("t : TOD; x : DT; r : BOOL;", "r := t = x;"),
    ] {
        let e = errors(vars, body);
        assert!(
            e.contains("no implicit conversion") || e.contains("cannot be a"),
            "{body}: {e}"
        );
    }
}

#[test]
fn after_22_00_is_a_comparison() {
    let p = run(
        "late : BOOL; early : BOOL; t : TOD := TOD#22:00:00; u : TOD := TOD#21:59:59.999999999;",
        "late := t >= TOD#22:00:00; early := u >= TOD#22:00:00;",
    );
    assert!(b(&p, "late") && !b(&p, "early"));
}

#[test]
fn min_max_limit_sel_and_mux_work_on_calendar_values() {
    let p = run(
        "a : TOD := TOD#08:00:00; c : TOD := TOD#17:00:00; lo : TOD; hi : TOD; l : TOD; s : TOD; m : TOD;",
        "lo := MIN(a, c); hi := MAX(a, c); l := LIMIT(a, TOD#12:00:00, c); s := SEL(TRUE, a, c); m := MUX(1, a, c);",
    );
    assert_eq!(int(&p, "lo"), tod_ns(8, 0, 0, 0));
    assert_eq!(int(&p, "hi"), tod_ns(17, 0, 0, 0));
    assert_eq!(int(&p, "l"), tod_ns(12, 0, 0, 0));
    assert_eq!(int(&p, "s"), tod_ns(17, 0, 0, 0));
    assert_eq!(int(&p, "m"), tod_ns(17, 0, 0, 0));
}

// ---- arithmetic ----

#[test]
fn a_time_of_day_wraps_at_midnight() {
    let p = run(
        "a : TOD := TOD#23:59:59.999999999; b : TOD := TOD#00:00:00; c : TOD; d : TOD; e : TOD; f : TOD;",
        "c := a + T#1ns; d := b - T#1ns; e := a + T#25h; f := b - T#49h;",
    );
    assert_eq!(int(&p, "c"), 0);
    assert_eq!(int(&p, "d"), DAY - 1);
    assert_eq!(int(&p, "e"), tod_ns(23, 59, 59, 999_999_999) + 3_600 * SEC - DAY);
    assert_eq!(
        int(&p, "f"),
        tod_ns(23, 0, 0, 0),
        "two days and an hour back from midnight"
    );
}

#[test]
fn a_time_of_day_never_leaves_its_day_whatever_the_duration() {
    let p = run(
        "a : TOD := TOD#12:00:00; c : TOD; d : TOD;",
        "c := a + T#106751d23h47m16s854ms775us807ns; d := a - T#106751d23h47m16s854ms775us807ns;",
    );
    for v in ["c", "d"] {
        assert!((0..DAY).contains(&int(&p, v)), "{v} = {}", int(&p, v));
    }
}

#[test]
fn a_date_and_time_moves_by_a_duration_across_the_year_and_the_leap_day() {
    let p = run(
        "a : DT := DT#2023-12-31-23:59:59; b : DT := DT#2024-02-28-12:00:00; c : DT := DT#2100-02-28-12:00:00; \
         d : DT := DT#2024-03-01-00:00:00; \
         a1 : DT; b1 : DT; b2 : DT; c1 : DT; d1 : DT;",
        "a1 := a + T#1s; b1 := b + T#1d; b2 := b1 + T#1d; c1 := c + T#1d; d1 := d - T#1ns;",
    );
    assert_eq!(int(&p, "a1"), days(2024, 1, 1) * DAY);
    assert_eq!(int(&p, "b1"), days(2024, 2, 29) * DAY + tod_ns(12, 0, 0, 0));
    assert_eq!(int(&p, "b2"), days(2024, 3, 1) * DAY + tod_ns(12, 0, 0, 0));
    assert_eq!(
        int(&p, "c1"),
        days(2100, 3, 1) * DAY + tod_ns(12, 0, 0, 0),
        "2100 is no leap year"
    );
    assert_eq!(int(&p, "d1"), days(2024, 2, 29) * DAY + DAY - 1);
}

#[test]
fn differences_are_durations() {
    let p = run(
        "a : DT := DT#2024-03-01-00:00:00; z : DT := DT#2024-02-28-00:00:00; \
         d1 : DATE := D#2024-03-01; d0 : DATE := D#2023-03-01; t1 : TOD := TOD#06:00:00; t0 : TOD := TOD#18:30:00; \
         x : TIME; y : TIME; w : TIME; v : TIME;",
        "x := a - z; y := d1 - d0; w := t1 - t0; v := z - a;",
    );
    assert_eq!(int(&p, "x"), 2 * DAY, "2024-02-28 to 03-01 spans the leap day");
    assert_eq!(int(&p, "y"), 366 * DAY);
    assert_eq!(
        int(&p, "w"),
        -(12 * 3_600 + 30 * 60) * SEC,
        "a TOD difference is not wrapped"
    );
    assert_eq!(int(&p, "v"), -2 * DAY);
}

#[test]
fn a_time_scaled_into_a_date_and_back() {
    let p = run(
        "a : DT := DT#2024-01-01-00:00:00; z : DT := DT#2024-01-01-00:00:00; x : DT; n : INT;",
        "x := a + T#1d * 3; n := TIME_TO_INT((x - z) / 86400000);",
    );
    assert_eq!(int(&p, "x"), days(2024, 1, 4) * DAY);
    assert_eq!(int(&p, "n"), 3);
}

#[test]
fn undefined_arithmetic_is_refused_with_what_is_defined() {
    for (vars, body) in [
        ("d : DATE; i : TIME;", "d := d + i;"),
        ("d : DATE; i : TIME;", "d := d - i;"),
        ("d : DATE;", "d := d + d;"),
        ("t : TOD;", "t := t + t;"),
        ("x : DT;", "x := x + x;"),
        ("x : DT; i : TIME;", "x := i + x;"),
        ("t : TOD; i : TIME;", "t := i + t;"),
        ("x : DT; d : DATE; i : TIME;", "i := x - d;"),
        ("x : DT; d : DATE;", "d := x - d;"),
        ("x : DT;", "x := x + 1;"),
        ("x : DT;", "x := x - 1.5;"),
        ("x : DT;", "x := 1 + x;"),
        ("t : TOD; i : TIME;", "i := t - i;"),
    ] {
        let e = errors(vars, body);
        assert!(
            e.contains("are the only date arithmetic") || e.contains("expected"),
            "{body}: {e}"
        );
    }
}

#[test]
fn arithmetic_that_leaves_the_range_wraps_like_time_does() {
    let p = run(
        "a : DT := DT#2262-04-11-23:47:16.854775807; b : DT := DT#1677-09-21-00:12:43.145224192; \
         up : DT; down : DT; span : TIME;",
        "up := a + T#1ns; down := b - T#1ns; span := a - b;",
    );
    assert_eq!(int(&p, "up"), i128::from(i64::MIN));
    assert_eq!(int(&p, "down"), i128::from(i64::MAX));
    assert_eq!(
        int(&p, "span"),
        -1,
        "the whole range is 2^64 - 1 ns: it wraps in a TIME"
    );
}

// ---- conversions ----

#[test]
fn a_dt_splits_into_its_date_and_time_of_day() {
    let p = run(
        "x : DT := DT#2024-02-29-08:30:15.5; d : DATE; t : TOD; d2 : DATE; t2 : TOD; d3 : DATE; \
         y : DT := DT#1969-12-31-23:00:00; dy : DATE; ty : TOD; back : DT;",
        "d := DT_TO_DATE(x); t := DT_TO_TOD(x); d2 := DATE_AND_TIME_TO_DATE(x); t2 := DATE_AND_TIME_TO_TIME_OF_DAY(x); \
         d3 := TO_DATE(x); dy := DT_TO_DATE(y); ty := DT_TO_TOD(y); back := DATE_TO_DT(dy) + (ty - TOD#00:00:00);",
    );
    assert_eq!(int(&p, "d"), days(2024, 2, 29) * DAY);
    assert_eq!(int(&p, "t"), tod_ns(8, 30, 15, 500_000_000));
    assert_eq!(
        (int(&p, "d2"), int(&p, "t2"), int(&p, "d3")),
        (int(&p, "d"), int(&p, "t"), int(&p, "d"))
    );
    assert_eq!(
        int(&p, "dy"),
        days(1969, 12, 31) * DAY,
        "before 1970 the day rounds down"
    );
    assert_eq!(int(&p, "ty"), tod_ns(23, 0, 0, 0));
    assert_eq!(int(&p, "back"), int(&p, "y"));
}

#[test]
fn the_smallest_dt_has_a_date_below_the_range_which_clamps() {
    let p = run(
        "x : DT := DT#1677-09-21-00:12:43.145224192; d : DATE;",
        "d := DT_TO_DATE(x);",
    );
    assert_eq!(int(&p, "d"), i128::from(i64::MIN));
}

#[test]
fn a_tod_is_a_duration_from_midnight() {
    let p = run(
        "t : TOD := TOD#07:15:00; i : TIME; u : TOD;",
        "i := t - TOD#00:00:00; u := TOD#00:00:00 + T#18h; ",
    );
    assert_eq!(int(&p, "i"), tod_ns(7, 15, 0, 0));
    assert_eq!(int(&p, "u"), tod_ns(18, 0, 0, 0));
}

#[test]
fn concat_and_the_day_of_the_week() {
    let p = run(
        "d : DATE := D#2024-02-29; t : TOD := TOD#13:05:00; x : DT; w : SINT; w2 : SINT; w3 : SINT; \
         c : DATE; ct : TOD; bad : DATE; badt : TOD; y : INT := 2024; m : USINT := 3;",
        "x := CONCAT_DATE_TOD(d, t); w := DAY_OF_WEEK(d); w2 := DAY_OF_WEEK(x); w3 := DAY_OF_WEEK(D#1970-01-01); \
         c := CONCAT_DATE(y, m, 31); ct := CONCAT_TOD(23, 59, 59, 999); bad := CONCAT_DATE(2023, 2, 29); badt := CONCAT_TOD(24, 0, 0, 0);",
    );
    assert_eq!(int(&p, "x"), days(2024, 2, 29) * DAY + tod_ns(13, 5, 0, 0));
    assert_eq!(int(&p, "w"), zeller(2024, 2, 29));
    assert_eq!(int(&p, "w"), 4, "a Thursday");
    assert_eq!(int(&p, "w2"), 4);
    assert_eq!(int(&p, "w3"), 4);
    assert_eq!(int(&p, "c"), days(2024, 3, 31) * DAY);
    assert_eq!(int(&p, "ct"), tod_ns(23, 59, 59, 999_000_000));
    assert_eq!(
        int(&p, "bad"),
        0,
        "no such date is the epoch, as ruSTy's CONCAT_DATE"
    );
    assert_eq!(int(&p, "badt"), 0);
}

#[test]
fn the_calendar_functions_check_their_arguments() {
    for (vars, body, want) in [
        ("x : DT;", "x := DT_NOW(1);", "takes 0 argument"),
        (
            "w : SINT;",
            "w := DAY_OF_WEEK(TOD#01:00:00);",
            "needs a DATE or a DT",
        ),
        ("w : SINT;", "w := DAY_OF_WEEK(5);", "needs a DATE or a DT"),
        ("w : SINT;", "w := DAY_OF_WEEK();", "takes 1 argument"),
        (
            "x : DT;",
            "x := CONCAT_DATE_TOD(TOD#01:00:00, D#2024-01-01);",
            "expected DATE, found TOD",
        ),
        (
            "x : DT;",
            "x := CONCAT_DATE_TOD(D#2024-01-01);",
            "takes 2 argument",
        ),
        ("d : DATE;", "d := CONCAT_DATE(1, 2);", "takes 3 argument"),
        ("d : DATE;", "d := CONCAT_DATE(2024, 1.5, 1);", "cannot be a LINT"),
        ("t : TOD;", "t := CONCAT_TOD(1, 2, 3);", "takes 4 argument"),
        (
            "t : TOD;",
            "t := CONCAT_TOD(1, 2, 3, TOD#00:00:01);",
            "expected LINT",
        ),
        ("t : TOD;", "t := DT_NOW();", "expected TOD, found DT"),
    ] {
        let e = errors(vars, body);
        assert!(e.contains(want), "{body}: wanted {want:?} in\n{e}");
    }
}

// ---- text ----

#[test]
fn text_forms_read_as_the_literal_does() {
    let p = run(
        "d : DATE := D#2024-02-29; t : TOD := TOD#08:30:00.5; x : DT := DT#1969-12-31-23:59:59.000000001; \
         t0 : TOD; a : STRING[40]; b : STRING[40]; c : STRING[40]; e : STRING[40]; f : STRING[40]; \
         ts : STRING[10];",
        "a := TO_STRING(d); b := TOD_TO_STRING(t); c := DT_TO_STRING(x); e := DATE_TO_STRING(d); \
         f := TIME_OF_DAY_TO_STRING(t0); ts := DATE_AND_TIME_TO_STRING(x);",
    );
    assert_eq!(s(&p, "a"), "D#2024-02-29");
    assert_eq!(s(&p, "b"), "TOD#08:30:00.5");
    assert_eq!(s(&p, "c"), "DT#1969-12-31-23:59:59.000000001");
    assert_eq!(s(&p, "e"), "D#2024-02-29");
    assert_eq!(s(&p, "f"), "TOD#00:00:00");
    assert_eq!(
        s(&p, "ts"),
        "DT#1969-12",
        "cut at the destination, like every string"
    );
}

#[test]
fn text_reads_back_and_a_bad_text_is_the_zero_value() {
    let p = run(
        "d : DATE; t : TOD; x : DT; bad : DATE; bt : TOD; bx : DT; bare : DATE; long : TOD; sp : DT; \
         lo : DATE; ec : DATE;",
        "d := STRING_TO_DATE('D#2024-02-29'); t := STRING_TO_TOD('TOD#08:30:00.5'); x := STRING_TO_DT('DT#2024-02-29-08:30:00'); \
         bad := STRING_TO_DATE('D#2024-02-30'); bt := STRING_TO_TOD('25:00:00'); bx := STRING_TO_DT('nonsense'); \
         bare := STRING_TO_DATE('2024-03-01'); long := STRING_TO_TOD('time_of_day#00:00:01'); \
         sp := STRING_TO_DT('  date_and_time#2024-01-01-00:00:00  '); lo := STRING_TO_DATE('TOD#12:00:00'); ec := STRING_TO_DATE('');",
    );
    assert_eq!(int(&p, "d"), days(2024, 2, 29) * DAY);
    assert_eq!(int(&p, "t"), tod_ns(8, 30, 0, 500_000_000));
    assert_eq!(int(&p, "x"), days(2024, 2, 29) * DAY + tod_ns(8, 30, 0, 0));
    for v in ["bad", "bt", "bx", "lo", "ec"] {
        assert_eq!(int(&p, v), 0, "{v}");
    }
    assert_eq!(int(&p, "bare"), days(2024, 3, 1) * DAY);
    assert_eq!(int(&p, "long"), SEC);
    assert_eq!(int(&p, "sp"), days(2024, 1, 1) * DAY);
}

#[test]
fn text_never_reads_a_wrong_prefix_as_the_type() {
    let p = run(
        "d : DATE; t : TOD;",
        "d := STRING_TO_DATE('DT#2024-01-01'); t := STRING_TO_TOD('D#2024-01-01');",
    );
    assert_eq!((int(&p, "d"), int(&p, "t")), (0, 0));
}

// ---- where they live ----

#[test]
fn they_live_in_structs_arrays_function_blocks_and_functions() {
    let src = "
TYPE Shift : STRUCT start : TOD; stop : TOD; day : DATE; END_STRUCT; END_TYPE
FUNCTION_BLOCK Window
VAR_INPUT now : TOD; from : TOD; till : TOD; END_VAR
VAR_OUTPUT open : BOOL; END_VAR
  IF from <= till THEN open := now >= from AND now < till;
  ELSE open := now >= from OR now < till; END_IF;
END_FUNCTION_BLOCK
FUNCTION Next_day : DATE
VAR_INPUT d : DATE; END_VAR
  Next_day := DT_TO_DATE(CONCAT_DATE_TOD(d, TOD#00:00:00) + T#1d);
END_FUNCTION
PROGRAM t
VAR
  sh : Shift := (start := TOD#06:00:00, stop := TOD#14:00:00, day := D#2024-02-28);
  hours : ARRAY[0..2] OF TOD := [TOD#01:00:00, TOD#02:00:00, TOD#23:30:00];
  w : Window; a : BOOL; b : BOOL; c : BOOL; nx : DATE; first : TOD; last : TOD;
END_VAR
  w(now := TOD#23:30:00, from := TOD#22:00:00, till := TOD#06:00:00); a := w.open;
  w(now := TOD#12:00:00, from := TOD#22:00:00, till := TOD#06:00:00); b := w.open;
  w(now := hours[1], from := sh.start, till := sh.stop); c := w.open;
  nx := Next_day(Next_day(sh.day));
  first := hours[0]; last := hours[2];
END_PROGRAM";
    let mut p = build(src);
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::ZERO).unwrap();
    assert!(b(&p, "a"), "23:30 is inside the shift that crosses midnight");
    assert!(!b(&p, "b"));
    assert!(!b(&p, "c"), "02:00 is before 06:00");
    assert_eq!(
        int(&p, "nx"),
        days(2024, 3, 1) * DAY,
        "28 Feb + 2 days is 1 Mar in a leap year"
    );
    assert_eq!(
        (int(&p, "first"), int(&p, "last")),
        (tod_ns(1, 0, 0, 0), tod_ns(23, 30, 0, 0))
    );
}

#[test]
fn constants_fold_and_initial_values_may_be_expressions_of_constants() {
    let p = run("CLOSE : TOD := TOD#17:00:00; ", "");
    assert_eq!(int(&p, "CLOSE"), tod_ns(17, 0, 0, 0));
    let p = run(
        "a : TOD := TOD#12:00:00 + T#1h; x : DT := DT#2024-01-01-00:00:00 + T#1d;",
        "",
    );
    assert_eq!(int(&p, "a"), tod_ns(13, 0, 0, 0));
    assert_eq!(int(&p, "x"), days(2024, 1, 2) * DAY);
}

#[test]
fn a_program_port_of_these_types_works_but_is_no_signal() {
    let p = build(
        "PROGRAM t\nVAR_OUTPUT stamp : TOD := TOD#01:00:00; END_VAR\nVAR_INPUT n : DINT; END_VAR\nEND_PROGRAM",
    );
    let names: Vec<_> = p.ports().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["N"], "the TOD port is skipped like a STRING one");
}

#[test]
fn they_are_not_a_case_selector_or_a_loop_counter() {
    let e = errors("t : TOD;", "CASE t OF 1: t := TOD#01:00:00; END_CASE;");
    assert!(e.contains("TOD") || e.contains("integer"), "{e}");
    let e = errors("d : DATE;", "FOR d := D#2024-01-01 TO D#2024-01-05 DO END_FOR;");
    assert!(!e.is_empty());
}

// ---- the wall clock ----

#[test]
fn the_wall_clock_counts_from_the_epoch_until_the_host_sets_it() {
    let mut p =
        build("PROGRAM t\nVAR x : DT; now : TOD; END_VAR\nx := DT_NOW(); now := DT_TO_TOD(x);\nEND_PROGRAM");
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::from_secs(90)).unwrap();
    assert_eq!(int(&p, "x"), 90 * SEC);
    assert_eq!(int(&p, "now"), 90 * SEC);
}

#[test]
fn the_wall_clock_follows_scan_at_from_the_time_the_host_gave() {
    let start = i64::try_from(days(2024, 2, 28) * DAY + tod_ns(21, 59, 58, 0)).unwrap();
    let src = "PROGRAM t\nVAR late : BOOL; day : DATE; x : DT; wd : SINT; END_VAR\n\
               x := DT_NOW(); late := DT_TO_TOD(x) >= TOD#22:00:00; day := DT_TO_DATE(x); wd := DAY_OF_WEEK(day);\nEND_PROGRAM";
    let mut p = build(src);
    p.set_wall_clock(start);
    let mut img = Image::new(SIZES);
    let mut seen = Vec::new();
    for sec in [0u64, 1, 2, 3, 2 * 3600 + 2, 26 * 3600] {
        p.scan_at(&mut img, Duration::from_secs(sec)).unwrap();
        seen.push((b(&p, "late"), int(&p, "day") / DAY, int(&p, "wd")));
    }
    let d = days(2024, 2, 28);
    assert_eq!(
        seen,
        [
            (false, d, zeller(2024, 2, 28)),
            (false, d, zeller(2024, 2, 28)),
            (true, d, zeller(2024, 2, 28)),
            (true, d, zeller(2024, 2, 28)),
            (false, d + 1, zeller(2024, 2, 29)),
            (true, d + 1, zeller(2024, 2, 29)),
        ]
    );
}

#[test]
fn a_reset_keeps_the_clock_the_host_set() {
    let mut p = build("PROGRAM t\nVAR x : DT; END_VAR\nx := DT_NOW();\nEND_PROGRAM");
    p.set_wall_clock(5 * 1_000_000_000);
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::from_secs(1)).unwrap();
    p.reset();
    p.scan_at(&mut img, Duration::from_secs(2)).unwrap();
    assert_eq!(int(&p, "x"), 7 * SEC);
}

#[test]
fn a_wall_clock_at_the_end_of_the_range_wraps_instead_of_faulting() {
    let mut p = build("PROGRAM t\nVAR x : DT; END_VAR\nx := DT_NOW();\nEND_PROGRAM");
    p.set_wall_clock(i64::MAX);
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::from_nanos(1)).unwrap();
    assert_eq!(int(&p, "x"), i128::from(i64::MIN));
}

// ---- the box's state ----

#[test]
fn a_retained_date_survives_a_restart() {
    let src = "PROGRAM t\nVAR RETAIN last : DT; END_VAR\nlast := DT#2024-02-29-08:30:00;\nEND_PROGRAM";
    let mut p = build(src);
    let mut img = Image::new(SIZES);
    p.scan_at(&mut img, Duration::ZERO).unwrap();
    let kept = p.retain_state();
    let mut q = build(src.replace("last := DT#2024-02-29-08:30:00;", "").as_str());
    assert_eq!(int(&q, "last"), 0);
    let bytes = kept.encode().unwrap();
    let back = helmsman_iec::State::decode(&bytes).unwrap();
    q.restore_retain(&back);
    assert_eq!(int(&q, "last"), days(2024, 2, 29) * DAY + tod_ns(8, 30, 0, 0));
}

#[test]
fn the_wall_clock_is_no_initial_value() {
    let e = errors("x : DT := DT_NOW();", "");
    assert!(e.contains("initial value"), "{e}");
}

#[test]
fn a_division_in_a_duration_is_still_a_fault_not_a_date() {
    let mut p = build("PROGRAM t\nVAR x : DT; n : DINT; END_VAR\nx := x + T#1s / n;\nEND_PROGRAM");
    let mut img = Image::new(SIZES);
    assert_eq!(
        p.scan_at(&mut img, Duration::ZERO).unwrap_err().kind,
        FaultKind::DivisionByZero
    );
}

// ---- properties ----

fn valid_date() -> impl Strategy<Value = (i64, i64, i64)> {
    // Years the 64-bit range holds a whole day of: 1678 to 2261.
    (1678i64..=2261, 1i64..=12).prop_flat_map(|(y, m)| (Just(y), Just(m), 1..=month_len(y, m)))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn a_date_literal_is_the_independent_day_count((y, m, d) in valid_date()) {
        let p = run("v : DATE;", &format!("v := D#{y:04}-{m:02}-{d:02};"));
        prop_assert_eq!(int(&p, "v"), days(y, m, d) * DAY);
    }

    #[test]
    fn a_dt_literal_is_day_count_plus_time(
        (y, m, d) in valid_date(), h in 0i128..24, mi in 0i128..60, sec in 0i128..60, ns in 0i128..1_000_000_000
    ) {
        let p = run("v : DT;", &format!("v := DT#{y:04}-{m:02}-{d:02}-{h:02}:{mi:02}:{sec:02}.{ns:09};"));
        prop_assert_eq!(int(&p, "v"), days(y, m, d) * DAY + tod_ns(h, mi, sec, ns));
    }

    #[test]
    fn day_of_week_is_zellers((y, m, d) in valid_date()) {
        let p = run("w : SINT; x : SINT;", &format!("w := DAY_OF_WEEK(D#{y:04}-{m:02}-{d:02}); x := DAY_OF_WEEK(DT#{y:04}-{m:02}-{d:02}-23:59:59.999999999);"));
        prop_assert_eq!(int(&p, "w"), zeller(y, m, d));
        prop_assert_eq!(int(&p, "x"), zeller(y, m, d));
    }

    #[test]
    fn date_text_round_trips_and_reads_the_independent_date((y, m, d) in valid_date()) {
        let text = format!("D#{y:04}-{m:02}-{d:02}");
        let p = run("v : DATE; s : STRING[20]; w : DATE;", &format!("v := {text}; s := TO_STRING(v); w := STRING_TO_DATE(s);"));
        prop_assert_eq!(s(&p, "s"), text);
        prop_assert_eq!(int(&p, "w"), days(y, m, d) * DAY);
    }

    #[test]
    fn a_tod_plus_any_duration_is_the_euclidean_remainder(
        h in 0i128..24, mi in 0i128..60, sec in 0i128..60, ns in 0i128..1_000_000_000,
        // up to about 10 days either way, in nanoseconds
        by in -864_000_000_000_000i64..864_000_000_000_000
    ) {
        let t = tod_ns(h, mi, sec, ns);
        let p = run("a : TOD; p : TOD; q : TOD;", &format!(
            "a := TOD#{h:02}:{mi:02}:{sec:02}.{ns:09}; p := a + T#{}ns; q := a - T#{}ns;",
            by.abs(), by.abs()
        ));
        prop_assert_eq!(int(&p, "p"), (t + i128::from(by.abs())).rem_euclid(DAY));
        prop_assert_eq!(int(&p, "q"), (t - i128::from(by.abs())).rem_euclid(DAY));
        prop_assert!((0..DAY).contains(&int(&p, "p")) && (0..DAY).contains(&int(&p, "q")));
    }

    #[test]
    fn dt_difference_and_sum_undo_each_other(
        (y1, m1, d1) in valid_date(), (y2, m2, d2) in valid_date(), h in 0i128..24
    ) {
        let (a, z) = (format!("DT#{y1:04}-{m1:02}-{d1:02}-{h:02}:00:00"), format!("DT#{y2:04}-{m2:02}-{d2:02}-00:00:00"));
        let p = run("a : DT; z : DT; gap : TIME; back : DT; n : BOOL;",
            &format!("a := {a}; z := {z}; gap := a - z; back := z + gap; n := back = a;"));
        let want = (days(y1, m1, d1) * DAY + tod_ns(h, 0, 0, 0)) - days(y2, m2, d2) * DAY;
        // The gap is a TIME: it wraps when it does not fit 64 bits, and the sum wraps back.
        prop_assert_eq!(int(&p, "gap"), i128::from(want as i64));
        prop_assert!(b(&p, "n"));
    }

    #[test]
    fn a_dt_splits_into_date_plus_time_of_day((y, m, d) in valid_date(), h in 0i128..24, ns in 0i128..3_600_000_000_000) {
        let p = run("x : DT; d : DATE; t : TOD; again : DT;", &format!(
            "x := DT#{y:04}-{m:02}-{d:02}-{h:02}:00:00 + T#{ns}ns; d := DT_TO_DATE(x); t := DT_TO_TOD(x); again := CONCAT_DATE_TOD(d, t);"));
        let x = days(y, m, d) * DAY + tod_ns(h, 0, 0, 0) + ns;
        prop_assert_eq!(int(&p, "d"), x.div_euclid(DAY) * DAY);
        prop_assert_eq!(int(&p, "t"), x.rem_euclid(DAY));
        prop_assert_eq!(int(&p, "again"), x);
    }

    #[test]
    fn comparison_agrees_with_the_independent_order(
        (y1, m1, d1) in valid_date(), (y2, m2, d2) in valid_date()
    ) {
        let p = run("lt : BOOL; eq : BOOL; ge : BOOL;", &format!(
            "lt := D#{y1:04}-{m1:02}-{d1:02} < D#{y2:04}-{m2:02}-{d2:02}; eq := D#{y1:04}-{m1:02}-{d1:02} = D#{y2:04}-{m2:02}-{d2:02}; \
             ge := D#{y1:04}-{m1:02}-{d1:02} >= D#{y2:04}-{m2:02}-{d2:02};"));
        let (a, z) = (days(y1, m1, d1), days(y2, m2, d2));
        prop_assert_eq!((b(&p, "lt"), b(&p, "eq"), b(&p, "ge")), (a < z, a == z, a >= z));
    }

    #[test]
    fn no_text_makes_the_lexer_or_the_checker_panic(body in "[0-9:.\\-DTOLdtol# ]{0,40}") {
        for ty in ["DATE", "TOD", "DT"] {
            let _ = compile(&format!("PROGRAM t\nVAR v : {ty}; END_VAR\nv := {body};\nEND_PROGRAM"), SIZES);
            let _ = compile(&format!("PROGRAM t\nVAR v : {ty}; END_VAR\nv := {ty}#{body};\nEND_PROGRAM"), SIZES);
        }
    }

    #[test]
    fn no_text_reads_as_a_date_that_does_not_exist(text in "[0-9:.\\-DTOdto# ]{0,40}") {
        let p = run("d : DATE; t : TOD; x : DT; s : STRING[80];", &format!(
            "s := '{text}'; d := STRING_TO_DATE(s); t := STRING_TO_TOD(s); x := STRING_TO_DT(s);"));
        prop_assert!((i128::from(i64::MIN)..=i128::from(i64::MAX)).contains(&int(&p, "d")));
        prop_assert!((0..DAY).contains(&int(&p, "t")), "a TOD from text stays inside its day");
        // Whatever it read, it prints back as a literal that reads as itself.
        let q = run("d : DATE; s : STRING[40]; e : DATE;", &format!("d := {}; s := TO_STRING(d); e := STRING_TO_DATE(s);",
            helmsman_iec::dt::literal(Ty::Date, int(&p, "d"))));
        prop_assert_eq!(int(&q, "e"), int(&q, "d"));
    }
}

#[test]
fn every_day_from_1678_to_2261_round_trips_through_text() {
    // One program per year, a day each: the whole table of days against
    // the independent count. 584 programs.
    for y in 1678..=2261i64 {
        let mut body = String::new();
        let mut want = Vec::new();
        for m in 1..=12 {
            for d in [1, month_len(y, m)] {
                body.push_str(&format!(
                    "a[{}] := STRING_TO_DATE('D#{y:04}-{m:02}-{d:02}'); ",
                    want.len()
                ));
                want.push(days(y, m, d) * DAY);
            }
        }
        let p = run(&format!("a : ARRAY[0..{}] OF DATE;", want.len() - 1), &body);
        for (k, w) in want.iter().enumerate() {
            assert_eq!(int(&p, &format!("a[{k}]")), *w, "{y} #{k}");
        }
    }
}
