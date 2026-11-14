//! The `plant.sim` reader: what it takes, what it refuses and says why,
//! and the popup's rewrites.

use std::fmt::Write as _;

use proptest::prelude::*;

use crate::text::{Kind, Mode, number, parse, range, seconds, set_device, set_mode};

const OVEN: &str = "\
# The oven.
heater oven heat=heat_on temp=temp_c ambient=20 max=250 tau=60s
sensor pt1 of=oven.temp raw=temp_ma range=0..300 signal=4-20mA
mode temp_c stuck 80
seed 7
";

#[test]
fn a_file_reads_as_its_devices_modes_and_seed() {
    let (spec, problems) = parse(OVEN);
    assert_eq!(problems, []);
    assert_eq!(spec.devices.len(), 2);
    let oven = &spec.devices[0];
    assert_eq!(
        (oven.kind, oven.name.as_str(), oven.line),
        (Kind::Heater, "oven", 2)
    );
    assert!((oven.num("tau", 0.0) - 60.0).abs() < 1e-9);
    assert!((oven.num("start", oven.num("ambient", 0.0)) - 20.0).abs() < 1e-9);
    assert_eq!(oven.port("heat"), Some("heat_on"));
    assert_eq!(spec.mode("TEMP_C"), Mode::Stuck(Some(80.0)));
    assert_eq!(spec.mode("temp_ma"), Mode::Normal);
    assert_eq!(spec.seed, 7);
    let (owner, key, _) = spec.owner("Temp_C").expect("owned");
    assert_eq!((owner.name.as_str(), key), ("oven", "temp"));
}

#[test]
fn a_cylinder_is_a_valve() {
    let (spec, problems) = parse("cylinder c1 open=ext opened=out closed=in travel=1s");
    assert_eq!(problems, []);
    assert_eq!(spec.devices[0].kind, Kind::Valve);
}

#[test]
fn every_problem_names_its_line_and_the_rest_still_reads() {
    let text = "\
heater h1 heat=on temp=t
pump p1 run=x
heater h2 heat=on
heater h3 heat=on temp=t2 colour=red
heater h1 heat=on temp=t3
tank t1 fill=f level=t
motor m1 run=r ramp=fast
mode t sideways
mode
seed -1
source s1 value=v shape=square
";
    let (spec, problems) = parse(text);
    assert_eq!(
        spec.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["h1"]
    );
    let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
    assert_eq!(lines, [2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    let says = |line: usize, part: &str| {
        let p = problems.iter().find(|p| p.line == line).expect("a problem");
        assert!(p.msg.contains(part), "line {line}: {}", p.msg);
    };
    says(2, "no device kind");
    says(3, "needs `temp=`");
    says(4, "takes no `colour`");
    says(5, "named h1 already");
    says(6, "driven by h1 already");
    says(7, "a time");
    says(8, "no mode");
    says(9, "mode INPUT WORD");
    says(10, "whole number");
    says(11, "one of ramp");
}

#[test]
fn comments_and_blank_lines_are_skipped() {
    let (spec, problems) = parse("\n# nothing\n   \nbutton b input=start_pb # the start\n");
    assert_eq!(problems, []);
    assert_eq!(spec.devices[0].port("input"), Some("start_pb"));
}

#[test]
fn modes_read_their_values() {
    let m = |w: &str, v: Option<&str>| Mode::parse(w, v);
    assert_eq!(m("stuck", None), Ok(Mode::Stuck(None)));
    assert_eq!(m("stuck", Some("TRUE")), Ok(Mode::Stuck(Some(1.0))));
    assert_eq!(m("drift", Some("-0.5")), Ok(Mode::Drift(-0.5)));
    assert_eq!(m("noise", Some("2")), Ok(Mode::Noise(2.0)));
    assert_eq!(m("chatter", Some("50ms")), Ok(Mode::Chatter(0.05)));
    assert_eq!(m("SLOW", Some("4")), Ok(Mode::Slow(4.0)));
    assert_eq!(m("absent", None), Ok(Mode::Absent));
    assert!(m("noise", Some("0")).is_err());
    assert!(m("slow", None).is_err());
    assert!(m("drift", Some("fast")).is_err());
    assert!(m("chatter", Some("-1s")).is_err());
}

#[test]
fn a_mode_line_written_reads_back_the_same() {
    for mode in [
        Mode::Stuck(None),
        Mode::Stuck(Some(80.0)),
        Mode::Drift(0.25),
        Mode::Noise(1.5),
        Mode::Over,
        Mode::Under,
        Mode::Slow(3.0),
        Mode::Chatter(0.05),
        Mode::Absent,
    ] {
        let (spec, problems) = parse(&mode.to_line("x"));
        assert_eq!(problems, [], "{mode:?}");
        assert_eq!(spec.mode("x"), mode);
    }
}

#[test]
fn set_mode_replaces_the_line_in_place_and_normal_removes_it() {
    let t = set_mode(OVEN, "TEMP_C", Mode::Absent);
    assert_eq!(t.lines().nth(3), Some("mode TEMP_C absent"));
    assert_eq!(t.lines().count(), OVEN.lines().count());
    let t = set_mode(&t, "temp_c", Mode::Normal);
    assert!(!t.contains("mode "));
    assert_eq!(t.lines().count(), OVEN.lines().count() - 1);
    let t = set_mode(&t, "temp_ma", Mode::Noise(0.2));
    assert!(t.ends_with("mode temp_ma noise 0.2\n"));
    assert_eq!(set_mode("", "x", Mode::Normal), "");
}

#[test]
fn set_device_replaces_appends_and_removes() {
    let t = set_device(OVEN, "OVEN", "heater oven heat=heat_on temp=temp_c max=300");
    assert_eq!(
        t.lines().nth(1),
        Some("heater oven heat=heat_on temp=temp_c max=300")
    );
    let t = set_device(&t, "b1", "button b1 input=start_pb");
    assert!(t.ends_with("button b1 input=start_pb\n"));
    let t = set_device(&t, "pt1", "");
    assert!(!t.contains("sensor"));
    let (_, problems) = parse(&t);
    assert_eq!(problems, []);
}

#[test]
fn a_device_line_written_reads_back_the_same() {
    let (spec, _) = parse(OVEN);
    for d in &spec.devices {
        let (again, problems) = parse(&d.to_line());
        assert_eq!(problems, []);
        assert_eq!(again.devices[0].values, d.values);
    }
}

#[test]
fn every_kind_reads_with_only_its_required_keys() {
    for k in Kind::ALL {
        let mut line = format!("{} d", k.word());
        for (i, key) in k.keys().iter().filter(|x| x.required).enumerate() {
            let _ = write!(line, " {}=v{i}", key.name);
        }
        let (spec, problems) = parse(&line);
        assert_eq!(problems, [], "{line}");
        assert_eq!(spec.devices[0].kind, k);
        assert_ne!(k.about(), "");
        assert_eq!(Kind::from_word(&k.word().to_uppercase()), Some(k));
    }
}

#[test]
fn times_numbers_and_ranges() {
    assert_eq!(seconds("500ms"), Some(0.5));
    assert_eq!(seconds("2"), Some(2.0));
    assert_eq!(seconds("1.5m"), Some(90.0));
    assert_eq!(seconds("1h"), Some(3600.0));
    assert_eq!(seconds("-1s"), None);
    assert_eq!(seconds("inf"), None);
    assert_eq!(seconds("NaNs"), None);
    assert_eq!(number("1e13"), None);
    assert_eq!(range("0..300"), Some((0.0, 300.0)));
    assert_eq!(range("-5..-1"), Some((-5.0, -1.0)));
    assert_eq!(range("3..3"), None);
    assert_eq!(range("3"), None);
}

#[test]
fn a_huge_file_stops_at_the_line_limit() {
    let text = "button b input=x\n".to_string()
        + &"# x\n".repeat(crate::text::MAX_LINES + 10)
        + "button c input=y\n";
    let (spec, _) = parse(&text);
    assert_eq!(spec.devices.len(), 1);
}

proptest! {
    #[test]
    fn any_text_reads_without_a_panic(text in "\\PC{0,400}") {
        let _ = parse(&text);
    }

    #[test]
    fn any_line_of_known_words_reads_or_says_why(
        kind in prop::sample::select(Kind::ALL.to_vec()),
        pairs in prop::collection::vec(("[a-z_]{1,10}", "[-a-z0-9._]{0,10}"), 0..8),
    ) {
        let mut line = format!("{} d", kind.word());
        for (k, v) in &pairs {
            let _ = write!(line, " {k}={v}");
        }
        let (spec, problems) = parse(&line);
        prop_assert_eq!(spec.devices.len() + problems.len(), 1);
    }

    #[test]
    fn set_mode_leaves_exactly_one_line_for_the_input(
        before in prop::collection::vec(prop::sample::select(vec!["stuck", "over", "absent", "normal"]), 0..4),
    ) {
        let mut t = String::from("heater h heat=a temp=t\n");
        for w in &before {
            t = set_mode(&t, "t", Mode::parse(w, None).unwrap());
        }
        let t = set_mode(&t, "t", Mode::Under);
        prop_assert_eq!(t.matches("mode t").count(), 1);
        let (spec, problems) = parse(&t);
        prop_assert!(problems.is_empty());
        prop_assert_eq!(spec.mode("t"), Mode::Under);
    }
}
