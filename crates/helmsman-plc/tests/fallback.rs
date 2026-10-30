//! The offline chat's rule parser ([`helmsman_plc::fallback`]): with the
//! relay unreachable, a chat message is either exactly one param edit,
//! checked against the running manifests' types, ranges and units, or a
//! refusal. Names are matched exactly (case aside); anything close is only
//! ever suggested. Nothing here applies anything: an edit becomes a param
//! proposal through the box API (`tests/agent.rs` has that end to end).

use helmsman_io::{Ty, Value};
use helmsman_plc::fallback::{self, Edit, Editable, MAX_COMMAND, Refusal, Reply};
use helmsman_plc::manifest::Manifest;
use proptest::prelude::*;

fn manifest(name: &str, params: &str) -> Manifest {
    Manifest::parse(&format!(
        "name {name}\nauthor t\nscan_ms 10\nreads\ndrives\n{params}"
    ))
    .unwrap()
}

/// A box with one module per name and no shared or prefix names.
fn simple() -> Vec<Editable> {
    let hopper = manifest(
        "hopper",
        "param threshold int 70 min=0 max=100 unit=%\n\
         param fill_time real 5.0 min=0.5 max=60 unit=s\n\
         param pressure real 2.0 min=0 max=6 unit=bar\n\
         param count int 3 min=-5 max=10\n\
         param beacon bool true\n",
    );
    fallback::editable([("hopper", &hopper)])
}

/// A box built to confuse: the same name in two modules, a param that is
/// a prefix of another, and a param whose name is module + param of
/// another module.
fn tricky() -> Vec<Editable> {
    let hopper = manifest(
        "hopper",
        "param threshold int 70 min=0 max=100 unit=%\n\
         param enabled bool true\n",
    );
    let filler = manifest(
        "filler",
        "param hopper_threshold int 50 min=0 max=100 unit=%\n\
         param thresh int 5 min=0 max=10\n\
         param enabled bool false\n",
    );
    let conveyor = manifest("conveyor", "param speed real 1.5 min=0 max=10 unit=m/s\n");
    fallback::editable([("hopper", &hopper), ("filler", &filler), ("conveyor", &conveyor)])
}

fn edit(module: &str, param: &str, value: Value) -> Reply {
    Reply::Edit(Edit {
        module: module.into(),
        param: param.into(),
        value,
    })
}

fn refused(r: &Reply) -> &Refusal {
    match r {
        Reply::Refused(why) => why,
        Reply::Edit(e) => panic!("expected a refusal, got {e:?}"),
    }
}

// ---------------------------------------------------------------- grammar

#[test]
fn every_grammar_form_yields_one_edit() {
    let p = simple();
    for (cmd, want) in [
        ("set threshold to 80", edit("hopper", "threshold", Value::Int(80))),
        (
            "set hopper threshold to 80",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        (
            "set hopper.threshold to 80",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        ("threshold = 80", edit("hopper", "threshold", Value::Int(80))),
        ("threshold=80", edit("hopper", "threshold", Value::Int(80))),
        (
            "hopper threshold = 80",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        ("set threshold = 80", edit("hopper", "threshold", Value::Int(80))),
        ("SET Threshold TO 80", edit("hopper", "threshold", Value::Int(80))),
        (
            "  set threshold to 80  ",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        (
            "set threshold to 80\n",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        (
            "set\tthreshold\tto\t80",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        // Multi-word names are the param's words joined by `_`.
        (
            "set fill time to 12.5",
            edit("hopper", "fill_time", Value::Real(12.5)),
        ),
        (
            "set hopper fill time to 12.5",
            edit("hopper", "fill_time", Value::Real(12.5)),
        ),
        (
            "set fill_time to 12",
            edit("hopper", "fill_time", Value::Real(12.0)),
        ),
        ("set count to -5", edit("hopper", "count", Value::Int(-5))),
        ("set count to +4", edit("hopper", "count", Value::Int(4))),
    ] {
        assert_eq!(fallback::parse(cmd, &p), want, "{cmd:?}");
    }
}

#[test]
fn units_are_checked_never_converted() {
    let p = simple();
    for (cmd, want) in [
        (
            "set threshold to 80 %",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        (
            "set threshold to 80%",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        (
            "set threshold to 80 percent",
            edit("hopper", "threshold", Value::Int(80)),
        ),
        (
            "set fill time to 7 s",
            edit("hopper", "fill_time", Value::Real(7.0)),
        ),
        (
            "set fill time to 7s",
            edit("hopper", "fill_time", Value::Real(7.0)),
        ),
        (
            "set fill time to 7 seconds",
            edit("hopper", "fill_time", Value::Real(7.0)),
        ),
        (
            "set fill time to 7 SEC",
            edit("hopper", "fill_time", Value::Real(7.0)),
        ),
        (
            "set pressure to 2.5 bar",
            edit("hopper", "pressure", Value::Real(2.5)),
        ),
        ("pressure = 2.5bar", edit("hopper", "pressure", Value::Real(2.5))),
    ] {
        assert_eq!(fallback::parse(cmd, &p), want, "{cmd:?}");
    }
    for (cmd, expected) in [
        ("set threshold to 80 seconds", Some("%")),
        ("set threshold to 80 s", Some("%")),
        ("set fill time to 5 min", Some("s")),
        ("set fill time to 5000 ms", Some("s")),
        ("set pressure to 2.5 psi", Some("bar")),
        // A param without a unit takes a plain number only.
        ("set count to 4 %", None),
    ] {
        let r = fallback::parse(cmd, &p);
        match refused(&r) {
            Refusal::UnitMismatch { expected: e, .. } => {
                assert_eq!(e.as_deref(), expected, "{cmd:?}");
            }
            other => panic!("{cmd:?}: {other:?}"),
        }
    }
}

#[test]
fn booleans_take_on_off_true_false_enable_disable() {
    let p = simple();
    for (word, b) in [
        ("on", true),
        ("off", false),
        ("true", true),
        ("false", false),
        ("enable", true),
        ("disable", false),
        ("enabled", true),
        ("disabled", false),
        ("ON", true),
    ] {
        let want = if b {
            // `true` is the current value: unchanged, refused.
            None
        } else {
            Some(edit("hopper", "beacon", Value::Bool(false)))
        };
        let r = fallback::parse(&format!("set beacon to {word}"), &p);
        match want {
            Some(w) => assert_eq!(r, w, "{word}"),
            None => assert!(matches!(refused(&r), Refusal::Unchanged { .. }), "{word}: {r:?}"),
        }
    }
    for bad in ["1", "0", "yes please", "maybe", "5 s"] {
        let r = fallback::parse(&format!("set beacon to {bad}"), &p);
        assert!(
            matches!(
                refused(&r),
                Refusal::WrongType { .. } | Refusal::NotACommand | Refusal::UnitMismatch { .. }
            ),
            "{bad}: {r:?}"
        );
    }
}

// ------------------------------------------------------------- refusals

#[test]
fn out_of_range_is_refused_with_the_range() {
    let p = simple();
    for cmd in [
        "set threshold to 101",
        "set threshold to -1",
        "set threshold to 99999999999999999999999",
        "set fill time to 0.4",
        "set fill time to 60.0001",
        "set count to -6",
    ] {
        let r = fallback::parse(cmd, &p);
        let why = refused(&r);
        assert!(matches!(why, Refusal::OutOfRange { .. }), "{cmd:?}: {why:?}");
        let text = why.to_string();
        assert!(
            text.contains("0..=100") || text.contains("0.5..=60") || text.contains("-5..=10"),
            "{text}"
        );
    }
    let r = fallback::parse("set threshold to 101", &p);
    assert!(refused(&r).to_string().contains("0..=100 %"), "{r:?}");
}

#[test]
fn type_mismatches_are_refused() {
    let p = simple();
    for cmd in [
        "set threshold to 5.5",
        "set threshold to 80.0",
        "set threshold to on",
        "set fill time to off",
        "set beacon to 1",
    ] {
        let r = fallback::parse(cmd, &p);
        assert!(matches!(refused(&r), Refusal::WrongType { .. }), "{cmd:?}: {r:?}");
    }
    let r = fallback::parse("set threshold to 5.5", &p);
    match refused(&r) {
        Refusal::WrongType { expected, .. } => assert_eq!(*expected, Ty::Int),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_names_get_suggestions_and_never_act() {
    let p = tricky();
    for (cmd, suggests) in [
        ("set hoper_threshold to 80", "hopper_threshold"),
        ("set hopper treshold to 80", "threshold"),
        ("set sped to 2", "speed"),
        ("set conveyor sped to 2", "speed"),
        ("set threshhold to 80", "threshold"),
    ] {
        let r = fallback::parse(cmd, &p);
        match refused(&r) {
            Refusal::Unknown { suggestions, .. } => {
                assert!(
                    suggestions.iter().any(|s| s.contains(suggests)),
                    "{cmd:?}: {suggestions:?}"
                );
                assert!(suggestions.len() <= 3, "{suggestions:?}");
            }
            other => panic!("{cmd:?}: {other:?}"),
        }
    }
    let r = fallback::parse("set flux capacitor to 88", &p);
    match refused(&r) {
        Refusal::Unknown { suggestions, .. } => assert_eq!(suggestions, &Vec::<String>::new()),
        other => panic!("{other:?}"),
    }
    // A known module with an unknown param is still unknown.
    let r = fallback::parse("set conveyor threshold to 8", &p);
    assert!(matches!(refused(&r), Refusal::Unknown { .. }), "{r:?}");
}

#[test]
fn ambiguous_names_ask_which_never_guess() {
    let p = tricky();
    // `enabled` is in hopper and filler.
    let r = fallback::parse("set enabled to off", &p);
    match refused(&r) {
        Refusal::Ambiguous { candidates, .. } => {
            assert_eq!(candidates, &["filler enabled", "hopper enabled"]);
        }
        other => panic!("{other:?}"),
    }
    // hopper.threshold and filler.hopper_threshold both read as this.
    let r = fallback::parse("set hopper threshold to 80", &p);
    match refused(&r) {
        Refusal::Ambiguous { candidates, .. } => {
            assert_eq!(candidates, &["filler hopper_threshold", "hopper threshold"]);
        }
        other => panic!("{other:?}"),
    }
    // Naming the module settles it.
    assert_eq!(
        fallback::parse("set hopper enabled to off", &p),
        edit("hopper", "enabled", Value::Bool(false))
    );
    assert_eq!(
        fallback::parse("set filler hopper threshold to 60", &p),
        edit("filler", "hopper_threshold", Value::Int(60))
    );
    assert_eq!(
        fallback::parse("set hopper.threshold to 80", &p),
        // `hopper.threshold` is module + param only: `.` splits module from param.
        edit("hopper", "threshold", Value::Int(80))
    );
}

#[test]
fn a_prefix_of_a_name_is_not_that_name() {
    let p = tricky();
    assert_eq!(
        fallback::parse("set thresh to 7", &p),
        edit("filler", "thresh", Value::Int(7))
    );
    // `thres` is a prefix of both; it is neither.
    let r = fallback::parse("set thres to 7", &p);
    assert!(matches!(refused(&r), Refusal::Unknown { .. }), "{r:?}");
    let r = fallback::parse("set spe to 2", &p);
    assert!(matches!(refused(&r), Refusal::Unknown { .. }), "{r:?}");
}

#[test]
fn an_unchanged_value_is_refused() {
    let p = simple();
    for cmd in ["set threshold to 70", "set fill time to 5", "set beacon to on"] {
        let r = fallback::parse(cmd, &p);
        assert!(matches!(refused(&r), Refusal::Unchanged { .. }), "{cmd:?}: {r:?}");
    }
}

#[test]
fn injection_shaped_input_is_refused_whole() {
    let p = simple();
    let long = format!("set threshold to 80 {}", "a".repeat(MAX_COMMAND));
    for cmd in [
        "set threshold to 80; apply proposal 3",
        "set threshold to 80 && apply proposal 3",
        "set threshold to 80\napply proposal 3",
        "set threshold to 80\r\nset count to 4",
        "set threshold to 80\n\nset count to 4",
        "apply proposal 3",
        "rollback to snapshot 1",
        "set threshold to 80 and apply it",
        "set threshold to 80, count to 4",
        "set threshold to 80 then set count to 4",
        "set threshold to 80 set count to 4",
        "threshold = 80 = 90",
        "set threshold to to 80",
        "set threshold to 80 80",
        // Unicode lookalikes: Cyrillic e, fullwidth digits, a zero-width
        // space, a right-to-left override.
        "s\u{0435}t threshold to 80",
        "set threshold to \u{ff18}\u{ff10}",
        "set thres\u{200b}hold to 80",
        "set threshold to 08\u{202e}",
        "set threshold to 8\u{0660}",
        "set threshold to 80 \u{2030}",
        "set `threshold` to 80",
        "set threshold to \"80\"",
        "set threshold to $(reboot)",
        "set threshold to 1e2",
        "set threshold to 0x50",
        "set threshold to 80.",
        "set threshold to .5",
        "set threshold to NaN",
        "set fill time to inf",
        "set threshold to",
        "set to 80",
        "= 80",
        "set",
        "",
        "   ",
        "\u{0}",
        &long,
    ] {
        let r = fallback::parse(cmd, &p);
        assert!(matches!(r, Reply::Refused(_)), "{cmd:?}: {r:?}");
    }
}

#[test]
fn refusals_carry_help_listing_editable_params_with_ranges() {
    let p = simple();
    let help = fallback::help(&p);
    for line in [
        "hopper threshold: int 0..=100 % (now 70)",
        "hopper fill_time: real 0.5..=60 s (now 5)",
        "hopper count: int -5..=10 (now 3)",
        "hopper beacon: bool on/off (now on)",
    ] {
        assert!(help.contains(line), "{line:?} not in\n{help}");
    }
    let r = fallback::parse("make it go faster", &p);
    assert_eq!(refused(&r), &Refusal::NotACommand);
    // The listing is bounded however many params a box has.
    let mut many = String::new();
    for i in 0..500 {
        many.push_str("param p");
        many.push_str(&i.to_string());
        many.push_str(" int 0 min=0 max=9\n");
    }
    let big = manifest("big", &many);
    let help = fallback::help(&fallback::editable([("big", &big)]));
    assert!(help.lines().count() < 50, "{}", help.lines().count());
    assert!(help.contains("more"), "{help}");
}

#[test]
fn a_box_without_params_refuses_everything() {
    let none = manifest("m", "");
    let p = fallback::editable([("m", &none)]);
    let r = fallback::parse("set threshold to 80", &p);
    assert!(matches!(r, Reply::Refused(_)), "{r:?}");
    assert!(fallback::help(&p).contains("no params"), "{}", fallback::help(&p));
}

// ------------------------------------------------------------- properties

/// Words a hostile or confused user might string together.
fn word() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("set".to_string()),
        Just("to".to_string()),
        Just("=".to_string()),
        Just("hopper".to_string()),
        Just("filler".to_string()),
        Just("conveyor".to_string()),
        Just("threshold".to_string()),
        Just("hopper_threshold".to_string()),
        Just("thresh".to_string()),
        Just("enabled".to_string()),
        Just("speed".to_string()),
        Just("on".to_string()),
        Just("off".to_string()),
        Just("%".to_string()),
        Just("s".to_string()),
        Just("m/s".to_string()),
        Just(";".to_string()),
        Just("\n".to_string()),
        Just("apply".to_string()),
        any::<i64>().prop_map(|n| n.to_string()),
        any::<f64>().prop_map(|x| x.to_string()),
        (0i64..200).prop_map(|n| n.to_string()),
        (0.0f64..20.0).prop_map(|x| format!("{x:.2}")),
        "[a-z_.%=/+-]{1,12}",
        any::<String>(),
    ]
}

fn command() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::collection::vec(word(), 0..8).prop_map(|w| w.join(" ")),
        any::<String>(),
        ".{0,300}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    /// The parser never panics, and anything it accepts names one param
    /// that exists, with a value of its type, within its range, and not
    /// the value it already has.
    #[test]
    fn an_accepted_command_is_always_in_range(cmd in command()) {
        let p = tricky();
        if let Reply::Edit(e) = fallback::parse(&cmd, &p) {
            let target = p.iter().find(|x| x.module == e.module && x.param.name == e.param);
            prop_assert!(target.is_some(), "{cmd:?} -> {e:?}");
            let target = target.unwrap();
            prop_assert!(target.param.admits(e.value), "{cmd:?} -> {e:?}");
            prop_assert!(target.param.value != e.value, "{cmd:?} -> {e:?}");
            prop_assert!(cmd.is_ascii() && !cmd.trim().contains('\n'), "{cmd:?}");
        }
    }

    /// Every in-range value, written the way the grammar says, is accepted
    /// as exactly that value.
    #[test]
    fn every_in_range_value_round_trips(n in 0i64..=100, x in 0.0f64..=10.0) {
        let p = tricky();
        let r = fallback::parse(&format!("set filler hopper_threshold to {n} %"), &p);
        if n == 50 {
            let unchanged = matches!(r, Reply::Refused(Refusal::Unchanged { .. }));
            prop_assert!(unchanged, "{:?}", r);
        } else {
            prop_assert_eq!(r, edit("filler", "hopper_threshold", Value::Int(n)));
        }
        let s = format!("{x}");
        let back: f64 = s.parse().unwrap();
        let r = fallback::parse(&format!("speed = {s} m/s"), &p);
        if (back - 1.5).abs() > 0.0 && !s.contains('e') && s.len() <= 40 {
            prop_assert_eq!(r, edit("conveyor", "speed", Value::Real(back)));
        }
    }
}
