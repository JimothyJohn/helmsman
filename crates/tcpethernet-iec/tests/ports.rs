//! A PROGRAM's `VAR_INPUT`s and `VAR_OUTPUT`s are its ports: the host sets
//! inputs by name before a scan and reads outputs after it. Only scalars,
//! never located, and `set_input` never wraps or converts.

use proptest::prelude::*;
use tcpethernet_iec::{ImageSizes, Port, PortDir, SetError, Ty, Value, compile};

const SIZES: ImageSizes = ImageSizes { i: 4, q: 4, m: 0 };

const LAMP: &str = "
PROGRAM lamp
VAR_INPUT run : BOOL; level : INT; temp : REAL; END_VAR
VAR_OUTPUT light : BOOL := TRUE; scaled : DINT; END_VAR
VAR n : USINT; END_VAR
  light := run AND level > 10;
  scaled := level * 2;
  n := n + 1;
END_PROGRAM";

#[test]
fn ports_are_inputs_then_outputs_in_declaration_order() {
    let p = compile(LAMP, SIZES).unwrap();
    let port = |name: &str, dir, ty| Port {
        name: name.into(),
        dir,
        ty,
    };
    assert_eq!(
        p.ports(),
        vec![
            port("RUN", PortDir::Input, Ty::Bool),
            port("LEVEL", PortDir::Input, Ty::Int),
            port("TEMP", PortDir::Input, Ty::Real),
            port("LIGHT", PortDir::Output, Ty::Bool),
            port("SCALED", PortDir::Output, Ty::DInt),
        ]
    );
}

#[test]
fn inputs_set_before_a_scan_drive_the_outputs() {
    let mut p = compile(LAMP, SIZES).unwrap();
    let mut img = tcpethernet_iec::Image::new(SIZES);
    assert_eq!(p.get("light"), Some(Value::Bool(true)), "initial value");
    p.set_input("Run", Value::Bool(true)).unwrap();
    p.set_input("LEVEL", Value::Int(11)).unwrap();
    p.scan(&mut img).unwrap();
    assert_eq!(p.get("light"), Some(Value::Bool(true)));
    assert_eq!(p.get("scaled"), Some(Value::Int(22)));
    p.set_input("level", Value::Int(-32768)).unwrap();
    p.scan(&mut img).unwrap();
    assert_eq!(p.get("light"), Some(Value::Bool(false)));
    assert_eq!(p.get("scaled"), Some(Value::Int(-65536)));
}

#[test]
fn set_input_refuses_what_does_not_fit_and_keeps_the_old_value() {
    let mut p = compile(LAMP, SIZES).unwrap();
    p.set_input("level", Value::Int(5)).unwrap();
    for (name, v, want) in [
        ("level", Value::Int(32768), SetError::Range(Ty::Int)),
        ("level", Value::Int(-32769), SetError::Range(Ty::Int)),
        ("level", Value::Real(5.0), SetError::Kind(Ty::Int)),
        ("level", Value::Bool(true), SetError::Kind(Ty::Int)),
        ("run", Value::Int(1), SetError::Kind(Ty::Bool)),
        ("temp", Value::Int(1), SetError::Kind(Ty::Real)),
        ("temp", Value::Real(f64::NAN), SetError::Range(Ty::Real)),
        ("temp", Value::Real(f64::INFINITY), SetError::Range(Ty::Real)),
        ("temp", Value::Real(1e300), SetError::Range(Ty::Real)),
        ("light", Value::Bool(false), SetError::NoSuchInput),
        ("n", Value::Int(1), SetError::NoSuchInput),
        ("nope", Value::Int(1), SetError::NoSuchInput),
        ("", Value::Int(1), SetError::NoSuchInput),
    ] {
        assert_eq!(p.set_input(name, v), Err(want), "{name} <- {v:?}");
    }
    assert_eq!(p.get("level"), Some(Value::Int(5)));
    assert_eq!(p.get("light"), Some(Value::Bool(true)));
}

#[test]
fn a_real_input_is_rounded_to_f32() {
    let mut p = compile(LAMP, SIZES).unwrap();
    p.set_input("temp", Value::Real(0.1)).unwrap();
    assert_eq!(p.get("temp"), Some(Value::Real(f64::from(0.1f32))));
}

#[test]
fn ports_must_be_unlocated_mutable_scalars() {
    for (src, want) in [
        (
            "PROGRAM p VAR_INPUT a AT %IX0.0 : BOOL; END_VAR END_PROGRAM",
            "bound by name, not located",
        ),
        (
            "PROGRAM p VAR_OUTPUT t : TON; END_VAR END_PROGRAM",
            "must be an elementary type",
        ),
        (
            "PROGRAM p VAR_INPUT a : ARRAY[0..1] OF INT; END_VAR END_PROGRAM",
            "must be an elementary type",
        ),
        (
            "PROGRAM p VAR_IN_OUT a : INT; END_VAR END_PROGRAM",
            "VAR_IN_OUT is not supported",
        ),
    ] {
        let err = compile(src, SIZES).unwrap_err();
        assert!(err.iter().any(|d| d.msg.contains(want)), "{src}: {err:?}");
    }
}

#[test]
fn writes_to_outputs_are_type_checked() {
    // Ports are program variables like any other inside the body; the
    // checker still type-checks what is written to them.
    let src = "PROGRAM p VAR_OUTPUT o : BOOL; END_VAR o := 3; END_PROGRAM";
    assert!(compile(src, SIZES).is_err());
}

proptest! {
    /// Every in-range integer is stored exactly; one past either end is refused.
    #[test]
    fn integer_inputs_are_exact_at_the_edges(
        ty in prop::sample::select(vec!["SINT", "INT", "DINT", "LINT", "USINT", "UINT", "UDINT", "ULINT", "BYTE", "WORD", "DWORD", "LWORD"]),
        pick in any::<i128>(),
    ) {
        let src = format!("PROGRAM p VAR_INPUT x : {ty}; END_VAR END_PROGRAM");
        let mut p = compile(&src, SIZES).unwrap();
        let t = p.ports()[0].ty;
        let (lo, hi) = t.range();
        let span = hi - lo;
        let v = lo + pick.rem_euclid(span.saturating_add(1).max(1));
        prop_assert_eq!(p.set_input("x", Value::Int(v)), Ok(()));
        prop_assert_eq!(p.get("x"), Some(Value::Int(v)));
        prop_assert_eq!(p.set_input("x", Value::Int(hi + 1)), Err(SetError::Range(t)));
        prop_assert_eq!(p.set_input("x", Value::Int(lo - 1)), Err(SetError::Range(t)));
        prop_assert_eq!(p.get("x"), Some(Value::Int(v)));
    }
}
