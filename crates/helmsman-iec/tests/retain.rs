//! `RETAIN` variables kept across a restart (the language's half): which
//! variables the saved state holds, its bytes and their decoding, the
//! restore by name and type, and the checker's refusals.

use std::time::Duration;

use helmsman_iec::{Carry, Image, ImageSizes, MAX_RETAIN, Program, State, Value, compile};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 2, q: 2, m: 4 };

fn prog(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{d:?}\n{src}"))
}

fn errors(src: &str) -> String {
    match compile(src, SIZES) {
        Ok(_) => panic!("compiled:\n{src}"),
        Err(d) => d.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
    }
}

fn run(p: &mut Program, img: &mut Image, scans: u64) {
    for t in 0..scans {
        p.scan_at(img, Duration::from_millis(t * 10)).unwrap();
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

const LINE: &str = "
TYPE Mode : (Idle, Run, Fault); END_TYPE
TYPE Lot : STRUCT id : UDINT; weight : REAL; END_STRUCT; END_TYPE
PROGRAM line
VAR RETAIN
  count : UDINT;
  parts : CTU;
  mode : Mode;
  lot : Lot;
  last : ARRAY[1..3] OF INT;
  ok : BOOL;
END_VAR
VAR NON_RETAIN fresh : BOOL := TRUE; END_VAR
VAR scans : UDINT; END_VAR
  count := count + 1;
  scans := scans + 1;
  parts(CU := NOT parts.CU, PV := 1000);
  mode := Run;
  lot.id := 42; lot.weight := 1.5;
  last[2] := -7;
  ok := TRUE;
  fresh := FALSE;
END_PROGRAM";

/// Only `RETAIN` variables are saved; a fresh compile restored from the
/// bytes has their values, and every other variable at its initial value.
#[test]
fn retain_variables_survive_encode_decode_and_the_rest_start_fresh() {
    let mut old = prog(LINE);
    let mut img = Image::new(SIZES);
    run(&mut old, &mut img, 25);
    let saved = old.retain_state();
    assert_eq!(
        saved.names().collect::<Vec<_>>(),
        ["COUNT", "PARTS", "MODE", "LOT", "LAST", "OK"]
    );
    let bytes = saved.encode().unwrap();
    let back = State::decode(&bytes).unwrap();
    assert_eq!(back.encode().unwrap(), bytes, "decoding is the inverse");

    let mut new = prog(LINE);
    assert!(new.has_retain());
    let carry = new.restore_retain(&back);
    assert_eq!(
        carry,
        Carry {
            kept: names(&["COUNT", "PARTS", "MODE", "LOT", "LAST", "OK"]),
            reset: vec![],
            dropped: vec![],
        }
    );
    assert_eq!(new.get("count"), Some(Value::Int(25)));
    assert_eq!(new.get("parts.CV"), Some(Value::Int(13)));
    assert_eq!(new.get("lot.weight"), Some(Value::Real(1.5)));
    assert_eq!(new.get("last[2]"), Some(Value::Int(-7)));
    assert_eq!(new.get("ok"), Some(Value::Bool(true)));
    assert_eq!(
        new.get("fresh"),
        Some(Value::Bool(true)),
        "NON_RETAIN starts fresh"
    );
    assert_eq!(new.get("scans"), Some(Value::Int(0)), "a plain VAR starts fresh");
    // And it keeps counting from there.
    let mut img = Image::new(SIZES);
    run(&mut new, &mut img, 1);
    assert_eq!(new.get("count"), Some(Value::Int(26)));
}

/// A RETAIN variable whose type changed starts at its initial value and is
/// named as reset; one no longer RETAIN (or gone) is dropped, even if the
/// program still declares it, so a plain VAR never picks up a saved value.
#[test]
fn a_changed_type_resets_and_a_variable_no_longer_retain_is_dropped() {
    let mut old = prog(
        "PROGRAM p VAR RETAIN a : INT; b : INT; c : INT; END_VAR
         a := 5; b := 6; c := 7; END_PROGRAM",
    );
    run(&mut old, &mut Image::new(SIZES), 1);
    let saved = State::decode(&old.retain_state().encode().unwrap()).unwrap();
    let mut new = prog(
        "PROGRAM p VAR RETAIN a : DINT; d : INT := 3; END_VAR VAR c : INT; END_VAR
         END_PROGRAM",
    );
    let carry = new.restore_retain(&saved);
    assert_eq!(
        carry,
        Carry {
            kept: vec![],
            reset: names(&["A", "D"]),
            dropped: names(&["B", "C"]),
        }
    );
    assert_eq!(new.get("a"), Some(Value::Int(0)));
    assert_eq!(
        new.get("c"),
        Some(Value::Int(0)),
        "not RETAIN any more: not restored"
    );
    assert_eq!(new.get("d"), Some(Value::Int(3)));
}

/// A program without RETAIN saves an empty state, and restoring one into
/// it changes nothing.
#[test]
fn a_program_without_retain_saves_nothing() {
    let mut p = prog("PROGRAM p VAR n : INT; END_VAR n := n + 1; END_PROGRAM");
    assert!(!p.has_retain());
    let s = p.retain_state();
    assert!(s.is_empty());
    assert_eq!(p.restore_retain(&s), Carry::default());
}

/// Bytes that claim a variable's type but hold a value that does not fit
/// it (out of range, wrong kind) never land in memory.
#[test]
fn a_saved_value_that_does_not_fit_is_not_restored() {
    let mut p = prog("PROGRAM p VAR RETAIN small : USINT := 9; END_VAR END_PROGRAM");
    let good = p.retain_state().encode().unwrap();
    // version, count, name (2 + 5), shape Elem(USInt), count 1, Int tag.
    let at = 1 + 4 + 2 + 5 + 2 + 4;
    assert_eq!(good[at], 1, "an INT value");
    let mut bad = good.clone();
    bad[at + 1..at + 17].copy_from_slice(&1000i128.to_le_bytes());
    let carry = p.restore_retain(&State::decode(&bad).unwrap());
    assert_eq!(carry.reset, names(&["SMALL"]));
    assert_eq!(p.get("small"), Some(Value::Int(9)));
    let mut kind = good;
    kind[at] = 2;
    kind.truncate(at + 1 + 8);
    let decoded = State::decode(&kind).unwrap();
    assert_eq!(
        p.restore_retain(&decoded).reset,
        names(&["SMALL"]),
        "a REAL in a USINT"
    );
}

/// Every truncation of a valid encoding is refused.
#[test]
fn a_truncated_encoding_is_refused() {
    let mut p = prog(LINE);
    run(&mut p, &mut Image::new(SIZES), 3);
    let bytes = p.retain_state().encode().unwrap();
    for n in 0..bytes.len() {
        assert!(
            State::decode(&bytes[..n]).is_err(),
            "decoded {n} of {} bytes",
            bytes.len()
        );
    }
    let mut longer = bytes;
    longer.push(0);
    assert!(State::decode(&longer).is_err(), "a trailing byte");
}

/// RETAIN where it cannot be kept compiled before RETAIN was kept (it was
/// ignored), so it still compiles and runs: with a warning that names
/// the variable, and nothing saved for it.
#[test]
fn retain_where_it_is_not_kept_warns_and_still_compiles() {
    for (src, want) in [
        (
            "FUNCTION_BLOCK fb VAR RETAIN n : INT; END_VAR n := n + 1; END_FUNCTION_BLOCK
             PROGRAM p VAR x : fb; END_VAR x(); END_PROGRAM",
            "N: RETAIN inside a function block is not kept across a restart yet: declare the instance RETAIN in the program",
        ),
        (
            "FUNCTION f : INT VAR RETAIN n : INT; END_VAR f := n; END_FUNCTION
             PROGRAM p VAR x : INT; END_VAR x := f(); END_PROGRAM",
            "N: a FUNCTION keeps nothing between calls, so RETAIN in it is not kept",
        ),
        (
            "PROGRAM p VAR_TEMP RETAIN n : INT; END_VAR n := 1; END_PROGRAM",
            "N: VAR_TEMP starts fresh on every scan, so RETAIN on it is not kept",
        ),
        (
            "PROGRAM p VAR RETAIN n AT %QX0.0 : BOOL; END_VAR n := TRUE; END_PROGRAM",
            "N: a located variable lives in the process image, so RETAIN on it is not kept",
        ),
    ] {
        let mut p = prog(src);
        let warnings: Vec<String> = p.warnings().iter().map(ToString::to_string).collect();
        assert_eq!(warnings.len(), 1, "{src}\n{warnings:?}");
        assert!(warnings[0].ends_with(want), "{src}\n gave {}", warnings[0]);
        assert!(!p.has_retain(), "nothing is saved for it");
        run(&mut p, &mut Image::new(SIZES), 3);
    }
    // A program with RETAIN kept has no warning.
    assert!(prog(LINE).warnings().is_empty());
}

/// What was never valid stays an error.
#[test]
fn retain_that_was_never_valid_is_refused() {
    for (src, want) in [
        (
            "PROGRAM p VAR RETAIN CONSTANT n : INT := 1; END_VAR END_PROGRAM",
            "N: a CONSTANT cannot be RETAIN",
        ),
        (
            "PROGRAM p VAR RETAIN NON_RETAIN n : INT; END_VAR END_PROGRAM",
            "RETAIN or NON_RETAIN, not both",
        ),
    ] {
        let e = errors(src);
        assert!(e.contains(want), "{src}\n gave {e}");
    }
}

/// The size cap refuses loudly, at the variable that passes it; exactly
/// the cap compiles.
#[test]
fn more_retain_values_than_the_cap_are_refused() {
    let at_cap = format!("PROGRAM p VAR RETAIN a : ARRAY[1..{MAX_RETAIN}] OF BOOL; END_VAR END_PROGRAM");
    let p = prog(&at_cap);
    assert_eq!(p.retain_state().values("A").map(<[Value]>::len), Some(MAX_RETAIN));
    let over =
        format!("PROGRAM p VAR RETAIN a : ARRAY[1..{MAX_RETAIN}] OF BOOL; b : INT; END_VAR END_PROGRAM");
    let e = errors(&over);
    assert!(
        e.contains("B: the RETAIN variables take") && e.contains("at most 65536"),
        "{e}"
    );
    // Plain variables are not counted.
    prog(&format!(
        "PROGRAM p VAR RETAIN a : INT; END_VAR VAR big : ARRAY[0..{MAX_RETAIN}] OF BOOL; END_VAR END_PROGRAM"
    ));
}

proptest! {
    /// Arbitrary bytes decode or are refused; never a panic.
    #[test]
    fn decoding_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = State::decode(&bytes);
    }

    /// A valid encoding with bytes flipped decodes or is refused, never a
    /// panic, and whatever decodes restores without one.
    #[test]
    fn a_damaged_encoding_never_panics(flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..8)) {
        let mut p = prog(LINE);
        run(&mut p, &mut Image::new(SIZES), 2);
        let mut bytes = p.retain_state().encode().unwrap();
        for (at, x) in flips {
            let n = bytes.len();
            bytes[at % n] ^= x;
        }
        if let Ok(s) = State::decode(&bytes) {
            let mut fresh = prog(LINE);
            let _ = fresh.restore_retain(&s);
            run(&mut fresh, &mut Image::new(SIZES), 1);
        }
    }
}
