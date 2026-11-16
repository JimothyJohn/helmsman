//! The generated programs the property tests run over (`props.rs`,
//! `fold.rs`).

use std::fmt::Write as _;

use proptest::prelude::*;

pub fn stmt() -> impl Strategy<Value = String> {
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
    let cond = (var.clone(), 0..7u8, -5i32..6).prop_map(|(v, op, k)| match op {
        0 => format!("{v} > {k}"),
        1 => format!("{v} = {k}"),
        2 => "x".to_string(),
        3 => "f.q AND NOT y".to_string(),
        4 => "NOT tm.Q AND NOT NOT x".to_string(),
        5 => format!("tm.Q OR {v} <= {k}"),
        _ => format!("y AND {v} < {k}"),
    });
    let leaf = prop_oneof![
        (var.clone(), expr.clone()).prop_map(|(v, e)| format!("{v} := {e};")),
        cond.clone().prop_map(|c| format!("x := {c};")),
        Just("y := NOT y;".to_string()),
        Just("x := NOT bs[a MOD 4] OR NOT bs[1] OR NOT TRUE;".to_string()),
        Just("wd := NOT wd AND 16#FF;".to_string()),
        cond.clone().prop_map(|c| format!("y := NOT ({c});")),
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

pub fn program(body: &[String], early_return: bool) -> String {
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
arr : ARRAY[0..3] OF INT; o AT %QW0 : INT; bs : ARRAY[0..3] OF BOOL; wd : WORD; END_VAR
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
