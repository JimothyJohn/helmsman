//! For any program the checker accepts: lowering succeeds without
//! panicking, every element maps to a statement of its POU, the layout
//! keeps its invariants and is deterministic, and a traced scan annotates
//! and explains every variable without panicking.
//!
//! The statement generator is the one from `helmsman-iec/tests/trace.rs`
//! (integration tests cannot share code across crates), extended with FB
//! output reads, `=>` outputs, computed indices, nested FB calls and jumps.

mod common;

use std::fmt::Write as _;
use std::time::Duration;

use common::{SIZES, check};
use helmsman_diagram::{annotate, explain, lower};
use helmsman_iec::{Image, compile};
use proptest::prelude::*;

fn stmt() -> impl Strategy<Value = String> {
    let var = prop_oneof![Just("a"), Just("b"), Just("c")];
    let expr = (var.clone(), 0..7u8, -9i32..10, var.clone()).prop_map(|(v, op, k, w)| match op {
        0 => format!("{v} + {k}"),
        1 => format!("{v} * {k} - {w}"),
        2 => format!("{v} / ({w} MOD 4)"),
        3 => format!("BYTE_TO_INT(inp) - {k}"),
        4 => format!("arr[{v} MOD 4] + arr[1]"),
        5 => format!("LIMIT(0, {v}, {k})"),
        _ => format!("{v} MOD 7"),
    });
    let cond = (var.clone(), 0..6u8, -5i32..6).prop_map(|(v, op, k)| match op {
        0 => format!("{v} > {k}"),
        1 => format!("{v} = {k}"),
        2 => "x".to_string(),
        3 => "f.q AND NOT y".to_string(),
        4 => format!("tm.Q OR {v} <= {k}"),
        _ => format!("y AND {v} < {k}"),
    });
    let leaf = prop_oneof![
        (var.clone(), expr.clone()).prop_map(|(v, e)| format!("{v} := {e};")),
        cond.clone().prop_map(|c| format!("x := {c};")),
        Just("y := NOT y;".to_string()),
        Just("f(in := x);".to_string()),
        Just("f(in := y, q => x);".to_string()),
        Just("tm(IN := x, PT := T#5ms, ET => et);".to_string()),
        (var.clone(), 0..4u8).prop_map(|(v, k)| format!("arr[{v} MOD 4] := {k};")),
        Just("arr[2] := arr[a MOD 4];".to_string()),
    ];
    leaf.prop_recursive(3, 24, 4, move |inner| {
        let block = proptest::collection::vec(inner, 1..4).prop_map(|v| v.join("\n"));
        prop_oneof![
            (cond.clone(), block.clone(), block.clone())
                .prop_map(|(c, t, e)| format!("IF {c} THEN\n{t}\nELSE\n{e}\nEND_IF;")),
            (cond.clone(), cond.clone(), block.clone(), block.clone())
                .prop_map(|(c, d, t, e)| format!("IF {c} THEN\n{t}\nELSIF {d} THEN\n{e}\nEND_IF;")),
            (var.clone(), block.clone(), block.clone()).prop_map(|(v, x, y)| format!(
                "CASE {v} OF\n0, 1: {x}\n2..5: {y}\nELSE\nc := c + 1;\nEND_CASE;"
            )),
            (0..4i32, block.clone()).prop_map(|(n, b)| format!(
                "FOR i := 0 TO {n} DO\n{b}\nIF i = a THEN EXIT; END_IF;\nEND_FOR;"
            )),
            (block.clone(), cond.clone()).prop_map(|(b, c)| format!(
                "n := 0;\nWHILE n < 3 AND NOT ({c}) DO\nn := n + 1;\n{b}\nIF n = 2 THEN CONTINUE; END_IF;\nEND_WHILE;"
            )),
            (block, cond.clone()).prop_map(|(b, c)| format!("REPEAT\n{b}\nUNTIL {c}\nEND_REPEAT;")),
        ]
    })
}

fn program(body: &[String], early_return: bool) -> String {
    let mut s = String::from(
        "FUNCTION_BLOCK pulse
VAR_INPUT in : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
VAR r : R_TRIG; t : TOF; END_VAR
r(CLK := in);
IF r.Q THEN
  t(IN := TRUE, PT := T#20ms);
ELSE
  t(IN := FALSE, PT := T#20ms);
END_IF;
q := t.Q;
END_FUNCTION_BLOCK
PROGRAM t
VAR inp AT %IB0 : BYTE; a, b, c, i, n : INT; x, y : BOOL; f : pulse; tm : TON; et : TIME;
arr : ARRAY[0..3] OF INT; o AT %QW0 : INT; END_VAR
",
    );
    for st in body {
        writeln!(s, "{st}").unwrap();
    }
    if early_return {
        s.push_str("IF y THEN RETURN; END_IF;\n");
    }
    s.push_str("o := a + b + c;\nEND_PROGRAM\n");
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn lowering_any_accepted_program_is_total_and_faithful(
        body in proptest::collection::vec(stmt(), 1..6),
        early_return in any::<bool>(),
        inputs in proptest::collection::vec(any::<u8>(), 1..4),
    ) {
        let src = program(&body, early_return);
        let Ok(mut p) = compile(&src, SIZES) else {
            return Err(TestCaseError::fail(format!("does not compile:\n{src}")));
        };
        let map = p.trace_map();
        let m = lower(&src, &map).map_err(|e| TestCaseError::fail(format!("{e}\n{src}")))?;
        prop_assert_eq!(&m, &lower(&src, &map).unwrap(), "deterministic");
        for pou in &m.pous {
            let d = &pou.diagram;
            let ours: Vec<_> = map.in_pou(&pou.name).collect();
            let listed: Vec<_> = d.stmts.iter().map(|s| s.id).collect();
            prop_assert_eq!(&listed, &ours, "every statement once, in order\n{}", src);
            prop_assert!(d.nodes.iter().all(|n| ours.contains(&n.stmt)));
            prop_assert!(d.regions.iter().all(|r| ours.contains(&r.stmt)));
            for w in &d.wires {
                prop_assert!(w.out < d.node(w.from).outs.len(), "{:?}", w);
                prop_assert!(w.inp < d.node(w.to).ins.len(), "{:?}", w);
                prop_assert!(w.from < w.to, "producers come first: {:?}", w);
            }
            check(d);
        }
        p.set_budget(5_000);
        let mut img = Image::new(SIZES);
        for (k, &inp) in inputs.iter().enumerate() {
            img.i[0] = inp;
            let (_, t) = p.scan_traced(&mut img, Duration::from_millis(u64::try_from(k).unwrap() * 7));
            for pou in &m.pous {
                let live = annotate(&pou.diagram, &t, if pou.name == "PULSE" { "F." } else { "" });
                prop_assert_eq!(live.values.len(), pou.diagram.nodes.len());
            }
            for (path, _) in map.vars() {
                let why = explain(&m, &t, path, 4);
                prop_assert_eq!(why.path.as_str(), path);
                prop_assert!(!why.outline().is_empty());
            }
        }
    }
}
