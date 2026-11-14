//! The simulate-this-input popup: its guess from an input's name, the
//! file it writes, and a double-click on an input opening it.

use helmsman_sim::{Kind, Mode};

use crate::sim_popup::{Behaviour, Popup, guess};

fn outs() -> Vec<(String, bool)> {
    vec![
        ("alarm".into(), true),
        ("heat_on".into(), true),
        ("fill_valve".into(), true),
        ("belt_run".into(), true),
    ]
}

#[test]
fn the_guess_reads_the_common_names() {
    for (name, is_bool, kind, key) in [
        ("start_pb", true, Kind::Button, "input"),
        ("stop", true, Kind::Button, "input"),
        ("valve_open", true, Kind::Valve, "opened"),
        ("cyl_retracted", true, Kind::Valve, "closed"),
        ("part_seen", true, Kind::Eye, "seen"),
        ("pe1", true, Kind::Eye, "seen"),
        ("motor_running", true, Kind::Motor, "running"),
        ("tank_high", true, Kind::Tank, "high"),
        ("level_low", true, Kind::Tank, "low"),
        ("auto", true, Kind::Switch, "input"),
        ("temp_c", false, Kind::Heater, "temp"),
        ("temp_ma", false, Kind::Sensor, "raw"),
        ("level_m", false, Kind::Tank, "level"),
        ("motor_rpm", false, Kind::Motor, "speed"),
        ("valve_pos", false, Kind::Valve, "position"),
        ("mode_sel", false, Kind::Selector, "input"),
        ("line_pressure", false, Kind::Source, "value"),
    ] {
        assert_eq!(guess(name, is_bool), (kind, key), "{name}");
    }
}

#[test]
fn a_new_input_is_guessed_and_wired_to_the_likeliest_output() {
    let (spec, _) = helmsman_sim::parse("");
    let p = Popup::open(
        "TEMP_C",
        false,
        &spec,
        Mode::Normal,
        &outs(),
        &["temp_c_ok".into()],
    );
    assert_eq!(p.kind, Some(Kind::Heater));
    assert_eq!(p.name, "heater1");
    assert_eq!(
        p.line().as_deref(),
        Some("heater heater1 heat=heat_on temp=temp_c")
    );
    assert!(p.has_ok);
    let text = p.apply("").expect("applies");
    assert_eq!(text, "heater heater1 heat=heat_on temp=temp_c\n");
    // A tank's fill finds the valve; its optional drain stays empty.
    let p = Popup::open("level_m", false, &spec, Mode::Normal, &outs(), &[]);
    assert_eq!(
        p.line().as_deref(),
        Some("tank tank1 fill=fill_valve level=level_m")
    );
}

#[test]
fn an_input_in_the_file_opens_as_it_is_and_renames_in_place() {
    let text = "# oven\nheater oven heat=heat_on temp=temp_c tau=30s\nmode temp_c noise 2\n";
    let (spec, _) = helmsman_sim::parse(text);
    let mut p = Popup::open("temp_c", false, &spec, spec.mode("temp_c"), &outs(), &[]);
    assert_eq!((p.kind, p.name.as_str()), (Some(Kind::Heater), "oven"));
    assert_eq!(p.behaviour, Behaviour::Abnormal);
    assert_eq!((p.fault.as_str(), p.fault_value.as_str()), ("noise", "2"));
    p.name = "kiln".into();
    p.behaviour = Behaviour::Absent;
    let out = p.apply(text).expect("applies");
    assert_eq!(
        out,
        "# oven\nheater kiln heat=heat_on temp=temp_c tau=30s\nmode temp_c absent\n"
    );
    p.behaviour = Behaviour::Normal;
    assert!(!p.apply(text).expect("applies").contains("mode"));
}

#[test]
fn set_by_hand_drops_the_device_and_keeps_the_mode() {
    let text = "button b1 input=start_pb\n";
    let (spec, _) = helmsman_sim::parse(text);
    let mut p = Popup::open("start_pb", true, &spec, Mode::Normal, &outs(), &[]);
    p.set_kind(None, "", &spec, &outs());
    p.behaviour = Behaviour::Abnormal;
    p.fault = "stuck".into();
    p.fault_value = "TRUE".into();
    assert_eq!(p.apply(text).expect("applies"), "mode start_pb stuck 1\n");
}

#[test]
fn what_the_file_would_not_take_is_refused_in_its_words() {
    let (spec, _) = helmsman_sim::parse("");
    let mut p = Popup::open("temp_c", false, &spec, Mode::Normal, &[], &[]);
    // No output to heat it: the line lacks `heat=`.
    let e = p.apply("").expect_err("refused");
    assert!(e.contains("needs `heat=`"), "{e}");
    p.values.iter_mut().find(|(k, _)| k == "heat").unwrap().1 = "heat_on".into();
    p.values.iter_mut().find(|(k, _)| k == "tau").unwrap().1 = "soon".into();
    assert!(p.apply("").expect_err("refused").contains("a time"));
    p.values.iter_mut().find(|(k, _)| k == "tau").unwrap().1 = "5s".into();
    p.behaviour = Behaviour::Abnormal;
    p.fault = "drift".into();
    assert!(p.apply("").expect_err("refused").contains("needs a rate"));
    // A problem the file already had is not this popup's to refuse.
    p.fault_value = "0.5".into();
    let broken = "pump p run=x\n";
    let out = p.apply(broken).expect("applies");
    assert!(out.starts_with("pump p run=x\nheater heater1"), "{out}");
}

#[test]
fn absent_says_how_the_program_can_tell() {
    let (spec, _) = helmsman_sim::parse("");
    let mut p = Popup::open("temp_c", false, &spec, Mode::Absent, &outs(), &[]);
    assert_eq!(p.behaviour, Behaviour::Absent);
    assert!(p.explain().contains("Declare temp_c_ok : BOOL"));
    p.has_ok = true;
    assert!(p.explain().contains("temp_c_ok reads FALSE"));
}
