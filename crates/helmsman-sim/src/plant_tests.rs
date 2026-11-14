//! The devices in motion, held to the physics each one stands for and to
//! what each mode does to an input.

use std::collections::BTreeMap;

use std::fmt::Write as _;

use proptest::prelude::*;

use crate::plant::{Feed, Plant, Val};
use crate::text::{Kind, Mode, parse};

fn plant(text: &str) -> Plant {
    let (spec, problems) = parse(text);
    assert_eq!(problems, [], "{text}");
    Plant::new(spec)
}

/// Runs `p` for `ms` in steps of `dt` with `outs` as the program's
/// outputs and hand-set inputs, and gives the last feed of each input.
fn run(p: &mut Plant, ms: u64, dt: u64, outs: &[(&str, Val)]) -> BTreeMap<String, Feed> {
    let outs: Vec<(String, Val)> = outs.iter().map(|(n, v)| (n.to_ascii_uppercase(), *v)).collect();
    let read = |n: &str| {
        outs.iter()
            .find(|(o, _)| o.eq_ignore_ascii_case(n))
            .map(|(_, v)| *v)
    };
    let mut last = BTreeMap::new();
    for _ in 0..ms / dt {
        for f in p.step(dt, &read) {
            last.insert(f.name.to_ascii_lowercase(), f);
        }
    }
    last
}

fn num(f: &BTreeMap<String, Feed>, name: &str) -> f64 {
    match f[name].value {
        Some(Val::Num(x)) => x,
        v => panic!("{name} reads {v:?}"),
    }
}

fn on(f: &BTreeMap<String, Feed>, name: &str) -> bool {
    match f[name].value {
        Some(Val::Bool(b)) => b,
        v => panic!("{name} reads {v:?}"),
    }
}

const OVEN: &str = "heater oven heat=heat_on temp=temp_c ambient=20 max=220 tau=10s";

#[test]
fn a_heater_reaches_63_percent_of_the_way_in_one_time_constant() {
    let mut p = plant(OVEN);
    let f = run(&mut p, 10_000, 10, &[("heat_on", Val::Bool(true))]);
    let want = 20.0 + 200.0 * (1.0 - (-1.0_f64).exp());
    assert!((num(&f, "temp_c") - want).abs() < 0.01, "{}", num(&f, "temp_c"));
    // And cools back toward ambient with the heat off, never past it.
    let f = run(&mut p, 120_000, 10, &[("heat_on", Val::Bool(false))]);
    let t = num(&f, "temp_c");
    assert!((20.0..20.01).contains(&t), "{t}");
}

#[test]
fn a_heater_at_half_power_settles_halfway() {
    let mut p = plant(OVEN);
    let f = run(&mut p, 200_000, 100, &[("heat_on", Val::Num(50.0))]);
    assert!((num(&f, "temp_c") - 120.0).abs() < 0.01);
}

#[test]
fn the_step_size_does_not_change_where_a_heater_gets() {
    let (mut a, mut b) = (plant(OVEN), plant(OVEN));
    let fa = run(&mut a, 7_000, 1, &[("heat_on", Val::Bool(true))]);
    let fb = run(&mut b, 7_000, 100, &[("heat_on", Val::Bool(true))]);
    assert!((num(&fa, "temp_c") - num(&fb, "temp_c")).abs() < 1e-6);
}

#[test]
fn a_tank_fills_to_its_switches_and_stops_at_full() {
    let mut p =
        plant("tank t fill=fill drain=drain level=lvl high=hi low=lo max=2 fill_rate=0.5 drain_rate=0.25");
    let f = run(&mut p, 100, 100, &[]);
    assert!(on(&f, "lo") && !on(&f, "hi"));
    let f = run(&mut p, 3_000, 100, &[("fill", Val::Bool(true))]);
    assert!((num(&f, "lvl") - 1.5).abs() < 1e-9);
    assert!(!on(&f, "lo"));
    let f = run(&mut p, 10_000, 100, &[("fill", Val::Bool(true))]);
    assert!((num(&f, "lvl") - 2.0).abs() < 1e-12);
    assert!(on(&f, "hi"));
    // Filling and draining at once: the net rate.
    let f = run(
        &mut p,
        4_000,
        100,
        &[("fill", Val::Bool(true)), ("drain", Val::Num(100.0))],
    );
    assert!((num(&f, "lvl") - 2.0).abs() < 1e-9);
    let f = run(&mut p, 20_000, 100, &[("drain", Val::Bool(true))]);
    assert!(num(&f, "lvl").abs() < 1e-12);
}

#[test]
fn a_motor_ramps_linearly_and_says_when_it_runs() {
    let mut p = plant("motor m run=go speed=rpm running=running rated=1000 ramp=4s");
    let f = run(&mut p, 1_000, 10, &[("go", Val::Bool(true))]);
    assert!((num(&f, "rpm") - 250.0).abs() < 1e-6);
    assert!(on(&f, "running"));
    let f = run(&mut p, 10_000, 10, &[("go", Val::Num(50.0))]);
    assert!((num(&f, "rpm") - 500.0).abs() < 1e-6);
    let f = run(&mut p, 1_500, 10, &[]);
    assert!((num(&f, "rpm") - 125.0).abs() < 1e-6);
    assert!(on(&f, "running"));
    let f = run(&mut p, 500, 10, &[]);
    assert!(num(&f, "rpm").abs() < 1e-6 && !on(&f, "running"));
}

#[test]
fn a_valve_travels_between_its_limits_and_springs_closed() {
    let mut p = plant("valve v open=cmd opened=is_open closed=is_closed position=pos travel=2s");
    let f = run(&mut p, 10, 10, &[]);
    assert!(on(&f, "is_closed") && !on(&f, "is_open"));
    let f = run(&mut p, 1_000, 10, &[("cmd", Val::Bool(true))]);
    assert!((num(&f, "pos") - 50.0).abs() < 1e-6);
    assert!(!on(&f, "is_closed") && !on(&f, "is_open"));
    let f = run(&mut p, 1_000, 10, &[("cmd", Val::Bool(true))]);
    assert!(on(&f, "is_open"));
    let f = run(&mut p, 2_000, 10, &[]);
    assert!(on(&f, "is_closed"));
}

#[test]
fn a_double_acting_cylinder_holds_where_it_is_between_commands() {
    let mut p = plant("cylinder c open=ext close=ret position=pos travel=1s");
    run(&mut p, 500, 10, &[("ext", Val::Bool(true))]);
    let f = run(&mut p, 5_000, 10, &[]);
    assert!((num(&f, "pos") - 50.0).abs() < 1e-6);
    let f = run(&mut p, 5_000, 10, &[("ret", Val::Bool(true))]);
    assert!(num(&f, "pos").abs() < 1e-9);
}

#[test]
fn a_photo_eye_sees_a_part_each_interval_of_belt_time() {
    let mut p = plant("eye pe seen=eye belt=belt every=2s length=500ms");
    let mut seen = Vec::new();
    let read = |n: &str| (n == "belt").then_some(Val::Bool(true));
    for _ in 0..600 {
        let f = p.step(10, &read);
        seen.push(f[0].value == Some(Val::Bool(true)));
    }
    let rises = seen.windows(2).filter(|w| !w[0] && w[1]).count();
    assert_eq!(rises, 3);
    assert_eq!(seen.iter().filter(|s| **s).count(), 150);
    // A stopped belt brings no part.
    let mut p = plant("eye pe seen=eye belt=belt every=2s length=500ms");
    let f = run(&mut p, 10_000, 10, &[]);
    assert!(!on(&f, "eye"));
}

#[test]
fn buttons_switches_and_selectors_read_the_hand_set_value() {
    let mut p = plant("button b input=start_pb\nswitch s input=auto\nselector m input=mode_sel positions=3");
    let f = run(
        &mut p,
        10,
        10,
        &[("start_pb", Val::Bool(true)), ("mode_sel", Val::Num(2.0))],
    );
    assert!(on(&f, "start_pb"));
    assert!(!on(&f, "auto"));
    assert!((num(&f, "mode_sel") - 2.0).abs() < 1e-12);
    assert_eq!(p.range("mode_sel"), Some((0.0, 2.0)));
}

#[test]
fn each_source_shape() {
    let at = |line: &str, ms: u64| {
        let mut p = plant(line);
        num(&run(&mut p, ms, 10, &[]), "v")
    };
    let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    close(
        at("source s value=v shape=ramp from=0 to=100 over=10s", 2_500),
        25.0,
    );
    close(
        at("source s value=v shape=ramp from=0 to=100 over=10s", 20_000),
        100.0,
    );
    close(
        at(
            "source s value=v shape=ramp from=0 to=100 over=10s repeat=yes",
            12_500,
        ),
        25.0,
    );
    close(
        at("source s value=v shape=exp from=0 to=10 tau=2s", 2_000),
        10.0 * (1.0 - (-1.0_f64).exp()),
    );
    close(
        at(
            "source s value=v shape=geometric from=1 to=1000 ratio=2 every=1s",
            3_500,
        ),
        8.0,
    );
    close(
        at(
            "source s value=v shape=geometric from=1 to=1000 ratio=2 every=1s",
            60_000,
        ),
        1000.0,
    );
    close(
        at("source s value=v shape=pulse from=0 to=1 on=1s off=3s", 500),
        1.0,
    );
    close(
        at("source s value=v shape=pulse from=0 to=1 on=1s off=3s", 2_000),
        0.0,
    );
    close(
        at("source s value=v shape=pulse from=0 to=1 on=1s off=3s", 4_500),
        1.0,
    );
    close(at("source s value=v shape=step from=5 to=9 at=3s", 2_990), 5.0);
    close(at("source s value=v shape=step from=5 to=9 at=3s", 3_000), 9.0);
    close(
        at("source s value=v shape=sine from=-1 to=1 period=4s", 1_000),
        1.0,
    );
    close(
        at("source s value=v shape=sine from=-1 to=1 period=4s", 3_000),
        -1.0,
    );
}

#[test]
fn a_random_walk_stays_in_bounds_and_is_the_same_walk_for_a_seed() {
    let walk = |seed: u64| {
        let mut p = plant(&format!(
            "source s value=v shape=walk from=0 to=10 step=1 every=100ms\nseed {seed}"
        ));
        let read = |_: &str| None;
        (0..2_000)
            .map(|_| p.step(10, &read)[0].value.map_or(f64::NAN, Val::num))
            .collect::<Vec<f64>>()
    };
    let a = walk(3);
    assert_eq!(a, walk(3));
    assert_ne!(a, walk(4));
    assert!(a.iter().all(|x| (0.0..=10.0).contains(x)));
    assert!(a.iter().any(|x| *x > 2.0), "it walks");
}

#[test]
fn a_sensor_scales_its_quantity_to_its_signal() {
    let text = "\
heater oven heat=h temp=t ambient=0 max=300 tau=0s
sensor ma of=oven.temp raw=t_ma range=0..300 signal=4-20mA
sensor v of=oven raw=t_v range=0..300 signal=0-10V
sensor c of=oven raw=t_counts range=0..300 signal=counts counts=0..27648
sensor ao of=valve_out raw=ao_ma range=0..100
";
    let mut p = plant(text);
    let f = run(
        &mut p,
        10,
        10,
        &[("h", Val::Num(50.0)), ("valve_out", Val::Num(25.0))],
    );
    assert!((num(&f, "t") - 150.0).abs() < 1e-9);
    assert!((num(&f, "t_ma") - 12.0).abs() < 1e-9);
    assert!((num(&f, "t_v") - 5.0).abs() < 1e-9);
    assert!((num(&f, "t_counts") - 13_824.0).abs() < 1e-9);
    assert!((num(&f, "ao_ma") - 8.0).abs() < 1e-9);
    // Held inside the signal's span past the measured range.
    let f = run(
        &mut p,
        10,
        10,
        &[("h", Val::Num(100.0)), ("valve_out", Val::Num(-50.0))],
    );
    assert!((num(&f, "t_ma") - 20.0).abs() < 1e-9);
    assert!((num(&f, "ao_ma") - 4.0).abs() < 1e-9);
    assert_eq!(p.range("t_ma"), Some((4.0, 20.0)));
    assert_eq!(p.range("t_counts"), Some((0.0, 27_648.0)));
}

#[test]
fn stuck_holds_where_it_was_or_the_value_given() {
    let mut p = plant(&format!("{OVEN}\nmode temp_c stuck"));
    let heat = [("heat_on", Val::Bool(true))];
    let f = run(&mut p, 10_000, 10, &heat);
    // Stuck from the first step: next to ambient.
    assert!((num(&f, "temp_c") - 20.0).abs() < 0.5);
    p.set_mode("temp_c", Mode::Normal);
    let f = run(&mut p, 10, 10, &heat);
    let was = num(&f, "temp_c");
    assert!(was > 100.0, "the heater ran on underneath: {was}");
    p.set_mode("TEMP_C", Mode::Stuck(None));
    let f = run(&mut p, 10_000, 10, &heat);
    assert!((num(&f, "temp_c") - was).abs() < 1.0, "stuck near {was}");
    p.set_mode("temp_c", Mode::Stuck(Some(80.0)));
    let f = run(&mut p, 10, 10, &heat);
    assert!((num(&f, "temp_c") - 80.0).abs() < 1e-12);
    // A BOOL sticks on.
    let mut p = plant("valve v open=cmd opened=is_open\nmode is_open stuck true");
    let f = run(&mut p, 10, 10, &[]);
    assert!(on(&f, "is_open"));
}

#[test]
fn drift_grows_from_when_it_began() {
    let mut p = plant("source s value=v shape=step from=10 to=10\nmode v drift 0.5");
    let f = run(&mut p, 4_000, 10, &[]);
    assert!((num(&f, "v") - 11.99).abs() < 0.02, "{}", num(&f, "v"));
}

#[test]
fn noise_stays_within_its_size_and_is_the_same_for_a_seed() {
    let noisy = || {
        let mut p = plant("source s value=v shape=step from=50 to=50\nmode v noise 2\nseed 9");
        let read = |_: &str| None;
        (0..1_000)
            .map(|_| p.step(10, &read)[0].value.map_or(f64::NAN, Val::num))
            .collect::<Vec<f64>>()
    };
    let a = noisy();
    assert_eq!(a, noisy());
    assert!(a.iter().all(|x| (48.0..=52.0).contains(x)));
    let mean = a.iter().sum::<f64>() / 1_000.0;
    assert!((mean - 50.0).abs() < 0.2, "{mean}");
    assert!(a.iter().any(|x| *x > 51.5) && a.iter().any(|x| *x < 48.5));
}

#[test]
fn over_and_under_go_a_tenth_of_the_span_past_the_range() {
    let mut p = plant(&format!("{OVEN}\nmode temp_c over"));
    let f = run(&mut p, 10, 10, &[]);
    assert!((num(&f, "temp_c") - 240.0).abs() < 1e-9);
    p.set_mode("temp_c", Mode::Under);
    let f = run(&mut p, 10, 10, &[]);
    assert!((num(&f, "temp_c") - 0.0).abs() < 1e-9);
    let mut p = plant("sensor s of=x raw=ma range=0..10\nmode ma under");
    let f = run(&mut p, 10, 10, &[]);
    assert!((num(&f, "ma") - 2.4).abs() < 1e-9);
}

#[test]
fn slow_makes_the_whole_device_respond_that_many_times_slower() {
    let mut p = plant("motor m run=go speed=rpm running=r rated=1000 ramp=4s\nmode r slow 2");
    let f = run(&mut p, 2_000, 10, &[("go", Val::Bool(true))]);
    assert!((num(&f, "rpm") - 250.0).abs() < 1e-6);
}

#[test]
fn chatter_bounces_after_each_change_and_then_settles() {
    let mut p = plant("switch s input=sw\nmode sw chatter 100ms\nseed 5");
    let mut read_on = false;
    let mut trace = Vec::new();
    for k in 0..100 {
        if k == 20 {
            read_on = true;
        }
        let read = |_: &str| Some(Val::Bool(read_on));
        trace.push(p.step(10, &read)[0].value == Some(Val::Bool(true)));
    }
    assert!(trace[..20].iter().all(|b| !b), "steady before the change");
    let bounce = &trace[20..30];
    assert!(bounce.windows(2).any(|w| w[0] != w[1]), "it bounces: {bounce:?}");
    assert!(trace[31..].iter().all(|b| *b), "settled on");
}

#[test]
fn absent_holds_nothing_new_and_says_so() {
    let mut p = plant(OVEN);
    run(&mut p, 1_000, 10, &[("heat_on", Val::Bool(true))]);
    p.set_mode("temp_c", Mode::Absent);
    let f = run(&mut p, 1_000, 10, &[("heat_on", Val::Bool(true))]);
    assert_eq!(f["temp_c"].value, None);
    assert!(!f["temp_c"].ok);
    p.set_mode("temp_c", Mode::Normal);
    let f = run(&mut p, 10, 10, &[("heat_on", Val::Bool(true))]);
    assert!(f["temp_c"].ok);
    assert!(num(&f, "temp_c") > 50.0, "the heater ran on while it was lost");
}

#[test]
fn a_mode_on_an_input_no_device_drives_works_on_its_hand_set_value() {
    let mut p = plant("mode stop_pb stuck true");
    let f = run(&mut p, 10, 10, &[("stop_pb", Val::Bool(false))]);
    assert!(on(&f, "stop_pb"));
    p.set_mode("estop", Mode::Absent);
    let f = run(&mut p, 10, 10, &[("estop", Val::Bool(true))]);
    assert_eq!(f["estop"].value, None);
}

#[test]
fn an_edited_file_carries_the_devices_it_keeps() {
    let mut p = plant(OVEN);
    let f = run(&mut p, 5_000, 10, &[("heat_on", Val::Bool(true))]);
    let was = num(&f, "temp_c");
    let (spec, _) = parse(&format!("{OVEN} start=20\ntank t fill=f level=l start=0.5"));
    let mut q = p.carried(spec);
    assert_eq!(q.now_ms(), 5_000);
    let f = run(&mut q, 10, 10, &[]);
    assert!((num(&f, "temp_c") - was).abs() < 1.0);
    assert!((num(&f, "l") - 0.5).abs() < 1e-9);
    q.reset();
    assert_eq!(q.now_ms(), 0);
    let f = run(&mut q, 10, 10, &[]);
    assert!((num(&f, "temp_c") - 20.0).abs() < 1e-9);
}

#[test]
fn an_unknown_quantity_reads_the_output_of_that_name_or_zero() {
    let mut p = plant("heater oven heat=h temp=t\nsensor s of=oven.level raw=ma range=0..100");
    let f = run(&mut p, 10, 10, &[]);
    assert!((num(&f, "ma") - 4.0).abs() < 1e-9);
    assert_eq!(p.spec().devices[1].kind, Kind::Sensor);
}

fn any_value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("0".to_string()),
        Just("-1".to_string()),
        Just("1e12".to_string()),
        Just("-1e12".to_string()),
        Just("0.001".to_string()),
        (-1000.0f64..1000.0).prop_map(|x| format!("{x}")),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn every_device_stays_finite_whatever_its_settings(
        kind in prop::sample::select(Kind::ALL.to_vec()),
        values in prop::collection::vec(any_value(), 20),
        shape in prop::sample::select(crate::text::SHAPES.to_vec()),
        outs in prop::collection::vec(prop_oneof![
            any::<bool>().prop_map(Val::Bool),
            (-1e6f64..1e6).prop_map(Val::Num),
        ], 4),
        mode in prop::sample::select(vec![
            Mode::Normal, Mode::Stuck(None), Mode::Drift(1.0), Mode::Noise(1.0),
            Mode::Over, Mode::Under, Mode::Slow(3.0), Mode::Chatter(0.05), Mode::Absent,
        ]),
    ) {
        let mut line = format!("{} d", kind.word());
        let mut ports = 0;
        for (k, v) in kind.keys().iter().zip(&values) {
            let v = match k.role {
                crate::text::Role::Out | crate::text::Role::InBool | crate::text::Role::InNum => {
                    ports += 1;
                    format!("p{ports}")
                }
                crate::text::Role::Of => "o1".to_string(),
                crate::text::Role::Time => match v.as_str() {
                    "1e12" | "-1e12" => "1h".to_string(),
                    v => format!("{}s", v.trim_start_matches('-')),
                },
                crate::text::Role::Range => "0..10".to_string(),
                crate::text::Role::Word(_) if k.name == "shape" => shape.to_string(),
                crate::text::Role::Word(ws) => ws[0].to_string(),
                crate::text::Role::Number => v.clone(),
            };
            let _ = write!(line, " {}={v}", k.name);
        }
        let (spec, problems) = parse(&line);
        prop_assert!(problems.is_empty(), "{line}: {problems:?}");
        let mut p = Plant::new(spec);
        for k in 1..=ports {
            p.set_mode(&format!("p{k}"), mode);
        }
        let read = |n: &str| {
            let i: usize = n.trim_start_matches(['p', 'o']).parse().unwrap_or(0);
            outs.get(i % outs.len()).copied()
        };
        for dt in [0, 10, 1000, 10, 60_000, 10] {
            for f in p.step(dt, &read) {
                if let Some(Val::Num(x)) = f.value {
                    prop_assert!(x.is_finite(), "{line}: {} = {x}", f.name);
                }
            }
        }
    }
}
