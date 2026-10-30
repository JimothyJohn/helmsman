//! The signal model: another module's output as a signal source (held
//! when its source is, refused when it loops or has no source), units on
//! the table, and per-signal poll rates on a link. Stepped by hand with
//! explicit times, so every step is deterministic.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::local::Channels;
use helmsman_io::text::parse;
use helmsman_io::{IoMap, Value};
use helmsman_plc::link::links;
use helmsman_plc::{BindError, ConfigError, Module, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn module(name: &str, src: &str) -> Module {
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap_or_else(|d| panic!("{name}: {d:?}"));
    Module {
        name: name.into(),
        program,
        period: ms(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

fn map(src: &str) -> IoMap {
    let f = parse(src).unwrap();
    f.map.validate().unwrap();
    f.map
}

fn table(src: &str) -> SignalTable {
    let f = parse(src).unwrap();
    SignalTable::new(&f.map, &f.safe).unwrap()
}

/// `producer` turns a box input into a rate; `consumer` reads the rate
/// and lights a lamp above a threshold. The rate never touches a device.
const MAP: &str = "
in  start bool box di:0
in  rate  real module producer unit=m/s
out lamp  bool box do:0 safe=false unit=on/off
";

const PRODUCER: &str = "
PROGRAM producer
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT rate : REAL; END_VAR
VAR zero : INT; END_VAR
  IF start THEN rate := 2.5; ELSE rate := 10 / zero; END_IF;
END_PROGRAM";

const CONSUMER: &str = "
PROGRAM consumer
VAR_INPUT RATE : REAL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
  lamp := rate > 1.0;
END_PROGRAM";

#[test]
fn a_module_reads_another_modules_output_by_signal_name() {
    let mut t = table(MAP);
    let (start, rate, lamp) = (
        t.index("start").unwrap(),
        t.index("rate").unwrap(),
        t.index("lamp").unwrap(),
    );
    assert_eq!(
        (t.unit(rate), t.unit(lamp), t.unit(start)),
        (Some("m/s"), Some("on/off"), None)
    );
    assert_eq!(t.source(rate), Some("producer"));
    // Consumer first in scan order: it sees the producer's last tick.
    let mods = vec![module("consumer", CONSUMER), module("producer", PRODUCER)];
    let mut rt = Runtime::with_signals(32, 32, mods, &t).unwrap();

    assert_eq!(t.value(rate), None, "nothing until the producer scans");
    t.set_input(start, Some(Value::Bool(true)));
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(rt.status("consumer"), Some(Status::Held));
    assert_eq!(t.value(rate), Some(Value::Real(2.5)));
    assert_eq!(t.command(lamp), Some(Value::Bool(false)));

    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(rt.status("consumer"), Some(Status::Running));
    assert_eq!(t.command(lamp), Some(Value::Bool(true)));

    // Nothing but the producer sets the rate.
    t.set_input(rate, Some(Value::Real(0.0)));
    assert_eq!(t.value(rate), Some(Value::Real(2.5)));
}

#[test]
fn a_reader_is_held_with_outputs_safe_while_its_source_is_held_or_faulted() {
    let mut t = table(MAP);
    let (start, rate, lamp) = (
        t.index("start").unwrap(),
        t.index("rate").unwrap(),
        t.index("lamp").unwrap(),
    );
    // Producer first: the consumer sees its value in the same tick.
    let mods = vec![module("producer", PRODUCER), module("consumer", CONSUMER)];
    let mut rt = Runtime::with_signals(32, 32, mods, &t).unwrap();
    t.set_input(start, Some(Value::Bool(true)));
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(t.command(lamp), Some(Value::Bool(true)));

    // The producer's own input goes missing: it is held, so is the consumer.
    t.set_input(start, None);
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(rt.status("producer"), Some(Status::Held));
    assert_eq!(t.value(rate), None);
    assert_eq!(rt.status("consumer"), Some(Status::Held));
    assert_eq!(t.command(lamp), Some(Value::Bool(false)), "safe");

    t.set_input(start, Some(Value::Bool(true)));
    rt.tick_with(ms(20), &[], &mut t);
    assert_eq!(rt.status("consumer"), Some(Status::Running));
    assert_eq!(t.command(lamp), Some(Value::Bool(true)));

    // The producer faults (divides by zero): the consumer is held for good.
    t.set_input(start, Some(Value::Bool(false)));
    let faults = rt.tick_with(ms(30), &[], &mut t);
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(t.value(rate), None);
    for now in [40, 50, 60] {
        rt.tick_with(ms(now), &[], &mut t);
        assert_eq!(rt.status("consumer"), Some(Status::Held));
        assert_eq!(t.command(lamp), Some(Value::Bool(false)));
    }
}

#[test]
fn a_module_reading_its_own_output_is_refused() {
    let t = table("in loop bool module selfish\n");
    let selfish = module(
        "selfish",
        "PROGRAM s VAR_INPUT loop : BOOL; END_VAR VAR_OUTPUT x : BOOL; END_VAR x := loop; END_PROGRAM",
    );
    let errs = Runtime::with_signals(32, 32, vec![selfish], &t).unwrap_err();
    assert!(
        errs.contains(&ConfigError::Port {
            module: "selfish".into(),
            err: BindError::ReadsOwnOutput("LOOP".into()),
        }),
        "{errs:?}"
    );
}

#[test]
fn only_the_named_module_may_drive_its_output_signal() {
    let t = table(MAP);
    let impostor = module(
        "impostor",
        "PROGRAM i VAR_OUTPUT rate : REAL; END_VAR rate := 1.0; END_PROGRAM",
    );
    let errs = Runtime::with_signals(32, 32, vec![module("producer", PRODUCER), impostor], &t).unwrap_err();
    assert!(
        errs.contains(&ConfigError::Port {
            module: "impostor".into(),
            err: BindError::DrivesInput("RATE".into()),
        }),
        "{errs:?}"
    );
}

#[test]
fn a_module_output_nobody_loaded_drives_is_refused_for_its_readers() {
    let t = table(MAP);
    // No producer at all.
    let errs = Runtime::with_signals(32, 32, vec![module("consumer", CONSUMER)], &t).unwrap_err();
    assert_eq!(
        errs,
        [ConfigError::NoSource {
            module: "consumer".into(),
            signal: "RATE".into()
        }]
    );
    // A module called producer that does not drive the rate.
    let lazy = module(
        "producer",
        "PROGRAM p VAR_INPUT start : BOOL; END_VAR END_PROGRAM",
    );
    let errs = Runtime::with_signals(32, 32, vec![lazy, module("consumer", CONSUMER)], &t).unwrap_err();
    assert_eq!(
        errs,
        [ConfigError::NoSource {
            module: "consumer".into(),
            signal: "RATE".into()
        }]
    );
    // A signal no module reads may name a module that is not loaded.
    let t = table("in spare bool module elsewhere\nin start bool box di:0\n");
    Runtime::with_signals(
        32,
        32,
        vec![module(
            "p",
            "PROGRAM p VAR_INPUT start : BOOL; END_VAR END_PROGRAM",
        )],
        &t,
    )
    .unwrap();
}

/// A module `name` reading every signal in `reads` and driving `drives`,
/// all BOOL.
fn node(name: &str, reads: &[String], drives: &str) -> Module {
    let ins: Vec<String> = reads.iter().map(|r| format!("{r} : BOOL;")).collect();
    let ins = ins.join(" ");
    let src = format!(
        "PROGRAM {name} VAR_INPUT {ins} END_VAR VAR_OUTPUT {drives} : BOOL; END_VAR {drives} := TRUE; END_PROGRAM"
    );
    module(name, &src)
}

#[test]
fn modules_reading_each_other_in_a_loop_are_refused() {
    // a -> b -> c -> a, plus d reading a (not in the loop).
    let t = table(
        "in a_out bool module a\nin b_out bool module b\nin c_out bool module c\nin d_out bool module d\n",
    );
    let mods = vec![
        node("a", &["c_out".into()], "a_out"),
        node("b", &["a_out".into()], "b_out"),
        node("c", &["b_out".into()], "c_out"),
        node("d", &["a_out".into()], "d_out"),
    ];
    let errs = Runtime::with_signals(32, 32, mods, &t).unwrap_err();
    assert_eq!(
        errs,
        [ConfigError::Cycle(vec!["a".into(), "b".into(), "c".into()])]
    );
    assert_eq!(
        errs[0].to_string(),
        "modules read each other's outputs in a loop: a -> b -> c"
    );
    // Two modules feeding each other.
    let mods = vec![
        node("a", &["b_out".into()], "a_out"),
        node("b", &["a_out".into()], "b_out"),
    ];
    let errs = Runtime::with_signals(32, 32, mods, &t).unwrap_err();
    assert_eq!(errs, [ConfigError::Cycle(vec!["a".into(), "b".into()])]);
}

#[test]
fn an_output_signal_is_still_readable_through_its_command() {
    // The old way to read another module's output keeps working.
    let mut t = table("out lamp bool box do:0\nout seen bool box do:1\n");
    let mods = vec![
        node("w", &[], "lamp"),
        module(
            "r",
            "PROGRAM r VAR_INPUT lamp : BOOL; END_VAR VAR_OUTPUT seen : BOOL; END_VAR seen := lamp; END_PROGRAM",
        ),
    ];
    let mut rt = Runtime::with_signals(32, 32, mods, &t).unwrap();
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(rt.status("r"), Some(Status::Running));
    assert_eq!(t.command(t.index("seen").unwrap()), Some(Value::Bool(true)));
}

proptest! {
    /// Any dependency graph between modules is accepted exactly when it
    /// has no loop, and a reported loop is a real one.
    #[test]
    fn a_dependency_graph_is_accepted_iff_it_is_acyclic(
        edges in prop::collection::vec(prop::collection::vec(any::<bool>(), 5), 5)
    ) {
        let n = 5;
        // reads[b] contains a when edges[a][b]: b reads a's output.
        let src: Vec<String> = (0..n).map(|a| format!("in o{a} bool module m{a}\n")).collect();
        let src = src.concat();
        let t = table(&src);
        let mods: Vec<Module> = (0..n)
            .map(|b| {
                let reads: Vec<String> = (0..n).filter(|&a| a != b && edges[a][b]).map(|a| format!("o{a}")).collect();
                node(&format!("m{b}"), &reads, &format!("o{b}"))
            })
            .collect();
        // Brute force: a loop exists iff some node reaches itself.
        let mut reach = [[false; 5]; 5];
        for a in 0..n {
            for b in 0..n {
                reach[a][b] = a != b && edges[a][b];
            }
        }
        for k in 0..n {
            for a in 0..n {
                for b in 0..n {
                    reach[a][b] |= reach[a][k] && reach[k][b];
                }
            }
        }
        let cyclic = (0..n).any(|a| reach[a][a]);
        match Runtime::with_signals(32, 32, mods, &t) {
            Ok(_) => prop_assert!(!cyclic),
            Err(errs) => {
                prop_assert!(cyclic, "{:?}", errs);
                let [ConfigError::Cycle(names)] = errs.as_slice() else {
                    return Err(TestCaseError::fail(format!("{errs:?}")));
                };
                let idx: Vec<usize> = names.iter().map(|s| s[1..].parse().unwrap()).collect();
                for (k, &b) in idx.iter().enumerate() {
                    let a = idx[(k + idx.len() - 1) % idx.len()];
                    prop_assert!(edges[a][b], "{} does not read {}", b, a);
                }
            }
        }
    }
}

#[test]
fn each_input_is_read_at_its_own_poll_rate() {
    let m =
        map("in fast bool box di:0 poll_ms=10\nin slow bool box di:1 poll_ms=100\nin dflt bool box di:2\n");
    let panel = Channels::new();
    let mut ls = links(&m, helmsman_io::open, Box::new(panel.clone())).unwrap();
    let [link] = ls.as_mut_slice() else { panic!() };
    assert_eq!(link.poll, ms(10), "the link cycles at its fastest input");
    let t = Mutex::new(SignalTable::new(&m, &BTreeMap::new()).unwrap());
    let get = |name: &str| {
        let t = t.lock().unwrap();
        t.value(t.index(name).unwrap())
    };
    link.cycle_at(&t, ms(0)).unwrap();
    assert_eq!(
        (get("fast"), get("slow"), get("dflt")),
        (
            Some(Value::Bool(false)),
            Some(Value::Bool(false)),
            Some(Value::Bool(false))
        )
    );
    for n in 0..3 {
        panel.set_input(n, true);
    }
    link.cycle_at(&t, ms(10)).unwrap();
    assert_eq!(
        (get("fast"), get("slow"), get("dflt")),
        (
            Some(Value::Bool(true)),
            Some(Value::Bool(false)),
            Some(Value::Bool(false))
        )
    );
    // The box's default period is 50 ms.
    link.cycle_at(&t, ms(50)).unwrap();
    assert_eq!(
        (get("slow"), get("dflt")),
        (Some(Value::Bool(false)), Some(Value::Bool(true)))
    );
    link.cycle_at(&t, ms(100)).unwrap();
    assert_eq!(get("slow"), Some(Value::Bool(true)));
    // A late cycle reads what is due once, not once per missed period.
    panel.set_input(1, false);
    link.cycle_at(&t, ms(1_000)).unwrap();
    assert_eq!(get("slow"), Some(Value::Bool(false)));
    panel.set_input(1, true);
    link.cycle_at(&t, ms(1_050)).unwrap();
    assert_eq!(get("slow"), Some(Value::Bool(false)), "next due at 1100");
    // cycle() still reads everything.
    link.cycle(&t).unwrap();
    assert_eq!(get("slow"), Some(Value::Bool(true)));
}

#[test]
fn a_module_output_signal_has_no_link() {
    let m = map("in rate real module producer\nin x bool box di:0\n");
    let ls = links(&m, helmsman_io::open, Box::new(Channels::new())).unwrap();
    assert_eq!(ls.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["box"]);
    let m = map("in rate real module producer\n");
    let ls = links(&m, helmsman_io::open, Box::new(Channels::new())).unwrap();
    assert_eq!(ls.len(), 0);
}
