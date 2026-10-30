//! Retained state carried from one compiled program to another (the hot
//! swap's half in the language): by name, only where the type has the same
//! shape, never a value that does not fit the new slot.

use std::time::Duration;

use helmsman_iec::{Carry, Image, ImageSizes, Program, Value, compile};
use proptest::prelude::*;

const SIZES: ImageSizes = ImageSizes { i: 2, q: 2, m: 4 };

fn prog(src: &str) -> Program {
    compile(src, SIZES).unwrap_or_else(|d| panic!("{d:?}\n{src}"))
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Scans `p` every 10 ms over `from..to`.
fn run(p: &mut Program, img: &mut Image, from: u64, to: u64) {
    for t in (from..to).step_by(10) {
        p.scan_at(img, ms(t)).unwrap();
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

const OLD: &str = "
PROGRAM line
VAR t : TON; c : CTU; n : INT; level : INT; tag : BOOL; END_VAR
  t(IN := TRUE, PT := T#500ms);
  c(CU := NOT c.CU, PV := 1000);
  n := n + 1;
  level := 7;
END_PROGRAM";

/// Same timer and counter, a new body, `level` now a REAL, `tag` renamed.
const NEW: &str = "
PROGRAM line
VAR t : TON; c : CTU; n : INT; level : REAL; label : BOOL; extra : DINT := 5; END_VAR
  t(IN := TRUE, PT := T#500ms);
  c(CU := NOT c.CU, PV := 1000);
  n := n + 2;
END_PROGRAM";

#[test]
fn a_timer_keeps_its_elapsed_time_and_a_counter_its_count() {
    let mut old = prog(OLD);
    let mut img = Image::new(SIZES);
    run(&mut old, &mut img, 0, 300);
    assert_eq!(old.get("t.ET"), Some(Value::Int(290_000_000)));
    let cv = old.get("c.CV").unwrap();
    assert_ne!(cv, Value::Int(0));

    let mut new = prog(NEW);
    let carry = new.restore(&old.state());
    assert_eq!(
        carry,
        Carry {
            kept: names(&["T", "C", "N"]),
            reset: names(&["LEVEL", "LABEL", "EXTRA"]),
            dropped: names(&["TAG"]),
        }
    );
    assert_eq!(new.get("c.CV"), Some(cv));
    assert_eq!(new.get("n"), Some(Value::Int(30)));
    assert_eq!(
        new.get("level"),
        Some(Value::Real(0.0)),
        "INT -> REAL starts at its initial value"
    );
    assert_eq!(new.get("extra"), Some(Value::Int(5)));
    // The timer started at 0 under the old program and fires at 500 ms, not
    // 500 ms after the swap.
    run(&mut new, &mut img, 300, 490);
    assert_eq!(new.get("t.Q"), Some(Value::Bool(false)));
    run(&mut new, &mut img, 490, 510);
    assert_eq!(new.get("t.Q"), Some(Value::Bool(true)));
    assert_eq!(new.get("n"), Some(Value::Int(30 + 2 * 21)));
}

#[test]
fn the_same_source_carries_everything() {
    let mut old = prog(OLD);
    let mut img = Image::new(SIZES);
    run(&mut old, &mut img, 0, 100);
    let mut new = prog(OLD);
    let carry = new.restore(&old.state());
    assert_eq!(carry.kept, names(&["T", "C", "N", "LEVEL", "TAG"]));
    assert_eq!(carry.reset, Vec::<String>::new());
    assert_eq!(carry.dropped, Vec::<String>::new());
    for v in ["t.ET", "t.Q", "c.CV", "n", "level", "tag"] {
        assert_eq!(new.get(v), old.get(v), "{v}");
    }
}

#[test]
fn every_kind_of_type_change_resets_just_that_variable() {
    let old = prog(
        "
TYPE mode : (IDLE, RUN, DONE); END_TYPE
TYPE pt : STRUCT x : INT; y : INT; END_STRUCT; END_TYPE
FUNCTION_BLOCK acc VAR_INPUT x : INT; END_VAR VAR sum : DINT; END_VAR sum := sum + x; END_FUNCTION_BLOCK
PROGRAM p
VAR
  a : SINT := 100; b : INT := 1000; m : mode := DONE; arr : ARRAY[0..2] OF INT := [1, 2, 3];
  s : pt := (x := 4, y := 5); f : acc; keep : UINT := 9; i_was_int : INT := -3;
END_VAR
  f(x := 3);
END_PROGRAM",
    );
    let mut old = old;
    old.scan_at(&mut Image::new(SIZES), ms(0)).unwrap();
    let mut new = prog(
        "
TYPE mode : (IDLE, RUN, DONE, FAULT); END_TYPE
TYPE pt : STRUCT x : INT; y : INT; z : INT; END_STRUCT; END_TYPE
FUNCTION_BLOCK acc VAR_INPUT x : INT; END_VAR VAR sum : LINT; END_VAR sum := sum + x; END_FUNCTION_BLOCK
PROGRAM p
VAR
  a : INT; b : SINT; m : mode; arr : ARRAY[1..3] OF INT; s : pt; f : acc; keep : UINT; i_was_int : UINT;
END_VAR
END_PROGRAM",
    );
    let carry = new.restore(&old.state());
    assert_eq!(carry.kept, names(&["KEEP"]));
    assert_eq!(carry.reset, names(&["A", "B", "M", "ARR", "S", "F", "I_WAS_INT"]));
    assert_eq!(new.get("keep"), Some(Value::Int(9)));
    for v in ["a", "b", "m", "arr[1]", "s.x", "f.sum", "i_was_int"] {
        assert_eq!(new.get(v), Some(Value::Int(0)), "{v}");
    }
}

#[test]
fn enums_structs_arrays_and_blocks_of_the_same_shape_carry() {
    let src = |body: &str| {
        format!(
            "
TYPE mode : (IDLE, RUN, DONE); END_TYPE
TYPE pt : STRUCT x : INT; y : REAL; END_STRUCT; END_TYPE
FUNCTION_BLOCK acc
  VAR_INPUT x : INT; END_VAR
  VAR sum : DINT; END_VAR
  VAR CONSTANT step : INT := {body}; END_VAR
  sum := sum + x * step;
END_FUNCTION_BLOCK
PROGRAM p
VAR m : mode; arr : ARRAY[1..3] OF pt; f : acc; g : ARRAY[0..1] OF acc; END_VAR
  m := RUN;
  arr[2].y := 2.5;
  f(x := 1);
  g[1](x := 2);
END_PROGRAM"
        )
    };
    let mut old = prog(&src("1"));
    old.scan_at(&mut Image::new(SIZES), ms(0)).unwrap();
    let mut new = prog(&src("10"));
    let carry = new.restore(&old.state());
    assert_eq!(carry.kept, names(&["M", "ARR", "F", "G"]));
    assert_eq!(new.get("m"), Some(Value::Int(1)));
    assert_eq!(new.get("arr[2].y"), Some(Value::Real(2.5)));
    assert_eq!(new.get("f.sum"), Some(Value::Int(1)));
    assert_eq!(new.get("g[1].sum"), Some(Value::Int(2)));
    // A constant inside a carried block is the new program's.
    assert_eq!(new.get("f.step"), Some(Value::Int(10)));
    new.scan_at(&mut Image::new(SIZES), ms(10)).unwrap();
    assert_eq!(new.get("f.sum"), Some(Value::Int(11)));
}

#[test]
fn constants_and_located_variables_are_not_state() {
    let mut old = prog(
        "
PROGRAM p
VAR CONSTANT limit : INT := 3; END_VAR
VAR out AT %QX0.0 : BOOL; n : INT; END_VAR
VAR_TEMP scratch : INT; END_VAR
  n := limit; out := TRUE; scratch := 4;
END_PROGRAM",
    );
    old.scan_at(&mut Image::new(SIZES), ms(0)).unwrap();
    assert_eq!(old.state().names().collect::<Vec<_>>(), vec!["N"]);
    let mut new = prog(
        "
PROGRAM p
VAR CONSTANT limit : INT := 8; END_VAR
VAR out AT %QX0.0 : BOOL; n : INT; END_VAR
  n := n + limit;
END_PROGRAM",
    );
    new.restore(&old.state());
    new.scan_at(&mut Image::new(SIZES), ms(10)).unwrap();
    assert_eq!(new.get("n"), Some(Value::Int(11)), "old n, new constant");
}

#[test]
fn ports_carry_like_any_variable() {
    let mut old =
        prog("PROGRAM p VAR_INPUT x : INT; END_VAR VAR_OUTPUT y : INT; END_VAR y := x * 2; END_PROGRAM");
    old.set_input("x", Value::Int(21)).unwrap();
    old.scan_at(&mut Image::new(SIZES), ms(0)).unwrap();
    let mut new =
        prog("PROGRAM p VAR_INPUT x : INT; END_VAR VAR_OUTPUT y : INT; END_VAR y := x + 1; END_PROGRAM");
    new.restore(&old.state());
    assert_eq!(
        new.get("y"),
        Some(Value::Int(42)),
        "the output holds until the new scan"
    );
}

/// Elementary types a generated variable may take, with an initial value
/// for each and how `get` reads it.
const TYPES: [(&str, &str, Value); 7] = [
    ("BOOL", "TRUE", Value::Bool(true)),
    ("SINT", "-100", Value::Int(-100)),
    ("INT", "1000", Value::Int(1000)),
    ("UINT", "60000", Value::Int(60000)),
    ("REAL", "1.5", Value::Real(1.5)),
    ("LREAL", "0.1", Value::Real(0.1)),
    ("TIME", "T#2s", Value::Int(2_000_000_000)),
];

fn decls(vars: &[(usize, usize, bool)], with_init: bool) -> (String, Vec<(String, usize, bool)>) {
    let mut seen = Vec::new();
    let mut text = String::new();
    for &(name, ty, arr) in vars {
        let name = format!("v{name}");
        if seen.iter().any(|(n, _, _)| n == &name) {
            continue;
        }
        let (t, init, _) = TYPES[ty];
        let decl = match (arr, with_init) {
            (true, true) => format!("{name} : ARRAY[0..1] OF {t} := [{init}, {init}];"),
            (true, false) => format!("{name} : ARRAY[0..1] OF {t};"),
            (false, true) => format!("{name} : {t} := {init};"),
            (false, false) => format!("{name} : {t};"),
        };
        text.push_str(&decl);
        seen.push((name, ty, arr));
    }
    (
        format!("PROGRAM p VAR {text} dummy : BOOL; END_VAR END_PROGRAM"),
        seen,
    )
}

fn zero_of(ty: usize) -> Value {
    match TYPES[ty].2 {
        Value::Bool(_) => Value::Bool(false),
        Value::Int(_) => Value::Int(0),
        Value::Real(_) => Value::Real(0.0),
    }
}

proptest! {
    /// Random old and new declarations: a variable carries exactly when
    /// the name and the type are the same; otherwise it is at its initial
    /// value. Never a panic, never a value of another type.
    #[test]
    fn carries_exactly_the_same_name_and_type(
        old in prop::collection::vec((0usize..6, 0usize..TYPES.len(), any::<bool>()), 0..8),
        new in prop::collection::vec((0usize..6, 0usize..TYPES.len(), any::<bool>()), 0..8),
    ) {
        let (old_src, old_vars) = decls(&old, true);
        let (new_src, new_vars) = decls(&new, false);
        let old_p = prog(&old_src);
        let mut new_p = prog(&new_src);
        let carry = new_p.restore(&old_p.state());
        for (name, ty, arr) in &new_vars {
            let same = old_vars.iter().any(|(n, t, a)| n == name && t == ty && a == arr);
            let want = if same { TYPES[*ty].2 } else { zero_of(*ty) };
            let path = if *arr { format!("{name}[1]") } else { name.clone() };
            prop_assert_eq!(new_p.get(&path), Some(want), "{} in\n{}\n->\n{}", path, old_src, new_src);
            prop_assert_eq!(carry.kept.contains(&name.to_ascii_uppercase()), same);
        }
    }
}
