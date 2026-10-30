//! A traced scan on the diagram: values on wires, gates open or closed,
//! and "on because …".

mod common;

use std::time::Duration;

use common::{SIZES, build, prog};
use helmsman_diagram::explain::{Because, Step, Test, Why};
use helmsman_diagram::{Gate, NodeKind, annotate, explain};
use helmsman_iec::trace::Trace;
use helmsman_iec::{Image, Program, Value};

fn scan(p: &mut Program, inputs: u8) -> Trace {
    let mut img = Image::new(SIZES);
    img.i[0] = inputs;
    let (r, t) = p.scan_traced(&mut img, Duration::from_millis(10));
    r.unwrap();
    t
}

const MOTOR: &str = "FUNCTION_BLOCK motor
VAR_INPUT run, fault : BOOL; END_VAR
VAR_OUTPUT on : BOOL; END_VAR
IF run THEN
  IF NOT fault THEN
    on := TRUE;
  ELSE
    on := FALSE;
  END_IF;
ELSE
  on := FALSE;
END_IF;
END_FUNCTION_BLOCK
PROGRAM t
VAR start AT %IX0.0 : BOOL; trip AT %IX0.1 : BOOL; m : motor; lamp : BOOL; END_VAR
m(run := start, fault := trip);
lamp := m.on;
END_PROGRAM
";

fn test(text: &str, value: bool, wanted: bool, inputs: Vec<Because>) -> Test {
    Test {
        text: text.into(),
        value: Some(Value::Bool(value)),
        wanted,
        inputs,
    }
}

fn input(path: &str, value: bool) -> Because {
    Because {
        path: path.into(),
        value: Some(Value::Bool(value)),
        why: Why::Input,
    }
}

/// `M.RUN` / `M.FAULT`, passed in from START / TRIP on line 16.
fn argument(path: &str, value: bool, from: &str, p: &Program) -> Because {
    let call = p.trace_map().at_line(16).next().unwrap();
    Because {
        path: path.into(),
        value: Some(Value::Bool(value)),
        why: Why::Argument {
            instance: "M".into(),
            call: Some((call, 16)),
            inputs: vec![input(from, value)],
        },
    }
}

#[test]
fn on_because_follows_the_output_through_nested_ifs_in_an_fb_call() {
    let (mut p, m) = build(MOTOR);
    let map = p.trace_map();
    let line = |l: u32| map.at_line(l).next().unwrap();
    let t = scan(&mut p, 0b01); // start, no trip
    let why = explain(&m, &t, "lamp", 4);
    let expected = Because {
        path: "LAMP".into(),
        value: Some(Value::Bool(true)),
        why: Why::Assigned {
            pou: "T".into(),
            stmt: line(17),
            line: 17,
            text: "LAMP := M.ON".into(),
            gates: vec![],
            inputs: vec![Because {
                path: "M.ON".into(),
                value: Some(Value::Bool(true)),
                why: Why::Assigned {
                    pou: "MOTOR".into(),
                    stmt: line(6),
                    line: 6,
                    text: "ON := TRUE".into(),
                    gates: vec![
                        Step {
                            stmt: line(4),
                            line: 4,
                            kind: helmsman_diagram::RegionKind::If,
                            label: "IF RUN".into(),
                            open: true,
                            tests: vec![test(
                                "RUN",
                                true,
                                true,
                                vec![argument("M.RUN", true, "START", &p)],
                            )],
                        },
                        Step {
                            stmt: line(5),
                            line: 5,
                            kind: helmsman_diagram::RegionKind::If,
                            label: "IF NOT FAULT".into(),
                            open: true,
                            tests: vec![test(
                                "NOT FAULT",
                                true,
                                true,
                                vec![argument("M.FAULT", false, "TRIP", &p)],
                            )],
                        },
                    ],
                    inputs: vec![],
                },
            }],
        },
    };
    assert_eq!(why, expected, "{:#?}", why.outline());
    assert_eq!(
        why.outline(),
        [
            "LAMP = TRUE because line 17: LAMP := M.ON",
            "  M.ON = TRUE because line 6: ON := TRUE",
            "    IF RUN (line 4) open",
            "      RUN = TRUE",
            "        M.RUN = TRUE passed in by M(...) on line 16",
            "          START = TRUE (input)",
            "    IF NOT FAULT (line 5) open",
            "      NOT FAULT = TRUE",
            "        M.FAULT = FALSE passed in by M(...) on line 16",
            "          TRIP = FALSE (input)",
        ]
    );
}

#[test]
fn off_because_the_inner_else_ran_with_the_conditions_it_needed_false() {
    let (mut p, m) = build(MOTOR);
    let t = scan(&mut p, 0b11); // start and trip
    let why = explain(&m, &t, "lamp", 4);
    let Why::Assigned { inputs, .. } = &why.why else {
        panic!("{why:#?}")
    };
    let Why::Assigned {
        line, text, gates, ..
    } = &inputs[0].why
    else {
        panic!("{why:#?}")
    };
    assert_eq!((*line, text.as_str()), (8, "ON := FALSE"));
    let labels: Vec<(&str, bool)> = gates.iter().map(|g| (g.label.as_str(), g.open)).collect();
    assert_eq!(labels, [("IF RUN", true), ("ELSE", true)]);
    let inner: Vec<(&str, Option<Value>, bool)> = gates[1]
        .tests
        .iter()
        .map(|t| (t.text.as_str(), t.value, t.wanted))
        .collect();
    assert_eq!(
        inner,
        [("NOT FAULT", Some(Value::Bool(false)), false)],
        "the ELSE needed NOT FAULT to fail"
    );

    // Stopped: the outer ELSE.
    let t = scan(&mut p, 0b00);
    let why = explain(&m, &t, "m.on", 1);
    let Why::Assigned { line, gates, .. } = &why.why else {
        panic!("{why:#?}")
    };
    assert_eq!(*line, 11);
    assert_eq!(gates[0].tests[0].text, "RUN");
    assert_eq!(gates[0].tests[0].value, Some(Value::Bool(false)));
    assert!(!gates[0].tests[0].wanted);
    assert!(
        gates[0].tests[0].inputs.iter().all(|b| b.why == Why::Elided),
        "depth 1 stops at the gate"
    );
}

#[test]
fn held_says_which_gate_kept_each_assignment_from_running() {
    let src = prog(
        "go AT %IX0.0 : BOOL; armed AT %IX0.1 : BOOL; lamp : BOOL;",
        "IF go THEN\n  IF armed THEN lamp := TRUE; END_IF;\nEND_IF;",
    );
    let (mut p, m) = build(&src);
    let t = scan(&mut p, 0b01); // go, not armed
    let why = explain(&m, &t, "lamp", 3);
    let Why::Held { blocked, .. } = &why.why else {
        panic!("{why:#?}")
    };
    assert_eq!(blocked.len(), 1);
    let gate = blocked[0].gate.as_ref().unwrap();
    assert_eq!(
        (gate.label.as_str(), gate.open),
        ("IF ARMED", false),
        "the outer IF was open"
    );
    assert_eq!(gate.tests[0].inputs, [input("ARMED", false)]);
    assert_eq!(
        why.outline(),
        [
            "LAMP = FALSE, held: no assignment ran this scan",
            "  line 4: LAMP := TRUE skipped:",
            "    IF ARMED (line 4) closed",
            "      ARMED = FALSE",
            "        ARMED = FALSE (input)",
        ]
    );
}

#[test]
fn library_blocks_report_their_inputs_and_cycles_stop() {
    let src = prog(
        "go AT %IX0.0 : BOOL; t : TON; on : BOOL;",
        "t(IN := go AND NOT on, PT := T#5ms);\non := t.Q OR on;",
    );
    let (mut p, m) = build(&src);
    let t = scan(&mut p, 1);
    let why = explain(&m, &t, "on", 5);
    assert_eq!(
        why.outline(),
        [
            "ON = FALSE because line 4: ON := T.Q OR ON",
            "  T.Q = FALSE from T : TON (called line 3)",
            "    GO = TRUE (input)",
            "    ON = FALSE",
            "  ON = FALSE",
        ],
        "ON reading itself is not followed again"
    );
}

#[test]
fn values_on_wires_and_gates_open_or_closed() {
    let src = prog(
        "go AT %IX0.0 : BOOL; n : INT; big : BOOL;",
        "IF go THEN n := n + 5; ELSE n := 0; END_IF;\nbig := n > 3 AND go;\nFOR n := 1 TO 0 DO big := FALSE; END_FOR;\nn := 5;",
    );
    let (mut p, m) = build(&src);
    let d = &m.program().unwrap().diagram;
    let t = scan(&mut p, 1);
    let live = annotate(d, &t, "");
    assert_eq!(
        live.gates,
        [Gate::Open(1), Gate::Closed, Gate::Closed],
        "IF arm taken, ELSE not, FOR made no pass"
    );
    // `n > 3 AND go`: reads end-of-scan values, logic re-evaluated.
    let and = d
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Op { op: "AND".into() })
        .unwrap();
    assert_eq!(live.values[and.id.0 as usize], [Some(Value::Bool(true))]);
    let gt = d
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Op { op: ">".into() })
        .unwrap();
    assert_eq!(live.values[gt.id.0 as usize], [Some(Value::Bool(true))]);
    // Arithmetic is not re-evaluated.
    let add = d
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Op { op: "+".into() })
        .unwrap();
    assert_eq!(live.values[add.id.0 as usize], [None]);
    // Every wire out of a read carries the variable's value.
    for (k, w) in d.wires.iter().enumerate() {
        if let NodeKind::Read { path } = &d.node(w.from).kind {
            assert_eq!(live.wire(d, k), t.get(path), "{path}");
        }
    }
    // The ELSE's write did not run; the IF's did.
    let writes: Vec<bool> = d
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Write { .. }))
        .map(|n| live.ran[n.id.0 as usize])
        .collect();
    assert_eq!(writes, [true, false, true, false, true]);
    // Not reached: nothing ran.
    let t = scan(&mut p, 0);
    let live = annotate(d, &t, "");
    assert_eq!(live.gates[0], Gate::Closed);
    assert_eq!(live.gates[1], Gate::Open(1));
}

#[test]
fn a_function_block_body_is_viewed_through_one_instance() {
    let src = "FUNCTION_BLOCK motor
VAR_INPUT run : BOOL; END_VAR
VAR_OUTPUT on : BOOL; END_VAR
on := run;
END_FUNCTION_BLOCK
PROGRAM t
VAR a, b : motor; END_VAR
a(run := TRUE);
b(run := FALSE);
END_PROGRAM";
    let (mut p, m) = build(src);
    let d = &m.pou("motor").unwrap().diagram;
    let t = scan(&mut p, 0);
    assert_eq!(annotate(d, &t, "A.").values[0], [Some(Value::Bool(true))]);
    assert_eq!(annotate(d, &t, "B.").values[0], [Some(Value::Bool(false))]);
    assert_eq!(
        m.instances(),
        [("A".into(), "MOTOR".into()), ("B".into(), "MOTOR".into())]
    );
}
