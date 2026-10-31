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
use helmsman_diagram::{Diagram, NodeId, NodeKind, Why, annotate, diff, drivers, explain, lower};
use helmsman_iec::{Image, Program, Ty, Value, compile};
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
            let prefix = if pou.name == "PULSE" { "F." } else { "" };
            for w in d.wires.iter().filter(|w| w.negated) {
                prop_assert!(
                    feeds_bool(&p, prefix, d, w.from, w.out),
                    "a negated wire carries a BOOL: {:?} from {:?}\n{}",
                    w,
                    d.node(w.from),
                    src
                );
            }
            for n in &d.nodes {
                if n.kind == (NodeKind::Op { op: "NOT".into() }) {
                    let w = d.inputs(n.id).next().unwrap();
                    prop_assert!(
                        !feeds_bool(&p, prefix, d, w.from, w.out),
                        "no NOT block on a BOOL: {:?}\n{}",
                        n,
                        src
                    );
                }
            }
            // A refit of an untouched layout keeps every box where it was.
            let mut r = d.clone();
            helmsman_diagram::layout::refit(&mut r);
            for (a, b) in d.regions.iter().zip(&r.regions) {
                let same = (a.rect.x - b.rect.x).abs() < 0.01
                    && (a.rect.y - b.rect.y).abs() < 0.01
                    && (a.rect.w - b.rect.w).abs() < 0.01
                    && (a.rect.h - b.rect.h).abs() < 0.01;
                prop_assert!(same, "{} {:?} -> {:?}\n{}", a.label, a.rect, b.rect, src);
            }
        }
        p.set_budget(5_000);
        let mut img = Image::new(SIZES);
        for (k, &inp) in inputs.iter().enumerate() {
            img.i[0] = inp;
            let (_, t) = p.scan_traced(&mut img, Duration::from_millis(u64::try_from(k).unwrap() * 7));
            for pou in &m.pous {
                let live = annotate(&pou.diagram, &t, if pou.name == "PULSE" { "F." } else { "" });
                prop_assert_eq!(live.values.len(), pou.diagram.nodes.len());
                for (k, w) in pou.diagram.wires.iter().enumerate() {
                    if w.negated && let Some(v) = live.wire(&pou.diagram, k) {
                        prop_assert!(matches!(v, Value::Bool(_)), "{:?} carries {:?}", w, v);
                    }
                }
            }
            let top = m.pous.iter().position(|q| q.name == "T").unwrap();
            for (path, _) in map.vars() {
                let why = explain(&m, &t, path, 4);
                prop_assert_eq!(why.path.as_str(), path);
                prop_assert!(!why.outline().is_empty());
                // The highlighted path holds the assignment explain names.
                let on = drivers(&m, &t, top, "", path);
                if let Why::Assigned { pou, stmt, .. } = &why.why
                    && pou == "T"
                {
                    let d = &m.pous[top].diagram;
                    prop_assert!(
                        d.nodes.iter().any(|n| n.stmt == *stmt
                            && matches!(n.kind, NodeKind::Write { .. })
                            && on.node(n.id)),
                        "{} {:?}\n{}",
                        path,
                        why.outline(),
                        src
                    );
                }
            }
        }
    }
}

/// Whether output `out` of node `n` (POU viewed at `prefix`) is BOOL, by
/// the compiled program's own types, not lowering's.
fn feeds_bool(p: &Program, prefix: &str, d: &Diagram, n: NodeId, out: usize) -> bool {
    let node = d.node(n);
    let ty = |path: &str| {
        // An element's type: any element's.
        let mut plain = String::new();
        let mut depth = 0;
        for c in path.chars() {
            match c {
                '[' => {
                    depth += 1;
                    if depth == 1 {
                        plain.push_str("[0");
                    }
                }
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        plain.push(']');
                    }
                }
                _ if depth == 0 => plain.push(c),
                _ => {}
            }
        }
        p.ty(&format!("{prefix}{plain}"))
    };
    match &node.kind {
        NodeKind::Read { path } | NodeKind::Write { path } | NodeKind::Access { path } => {
            ty(path) == Some(Ty::Bool)
        }
        NodeKind::Fb { instance, .. } => ty(&format!("{instance}.{}", node.outs[out])) == Some(Ty::Bool),
        NodeKind::Const { value } => matches!(value, Some(Value::Bool(_))),
        NodeKind::Op { op } => match op.as_str() {
            "=" | "<>" | "<" | "<=" | ">" | ">=" => true,
            "AND" | "OR" | "XOR" | "NOT" => d.inputs(n).all(|w| feeds_bool(p, prefix, d, w.from, w.out)),
            _ => false,
        },
        NodeKind::Func { .. } | NodeKind::For { .. } | NodeKind::Jump => false,
    }
}

/// Every signature of every POU of `m`, sorted: a multiset.
fn sigs(m: &helmsman_diagram::Module) -> Vec<diff::Sig> {
    let mut v: Vec<diff::Sig> = m
        .pous
        .iter()
        .flat_map(|p| diff::signatures(&p.name, &p.diagram))
        .collect();
    v.sort();
    v
}

fn lowered(src: &str) -> Result<helmsman_diagram::Module, TestCaseError> {
    let p = compile(src, SIZES).map_err(|e| TestCaseError::fail(format!("{e:?}\n{src}")))?;
    lower(src, &p.trace_map()).map_err(|e| TestCaseError::fail(format!("{e}\n{src}")))
}

fn line() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("a"), Just("b"), Just("c"), Just("END_IF;"), Just("")]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(150))]

    /// A source diff is a merge of both sources: its kept and removed lines
    /// are the old source in order, its kept and added lines the new one,
    /// with consecutive line numbers; nothing differs between a source and
    /// itself; and it keeps as many lines as the longest common
    /// subsequence (checked against a plain recursive one).
    #[test]
    fn a_line_diff_reproduces_both_sides_and_keeps_the_most(
        old in proptest::collection::vec(line(), 0..10),
        new in proptest::collection::vec(line(), 0..10),
        crlf in any::<bool>(),
    ) {
        let sep = if crlf { "\r\n" } else { "\n" };
        let (a, b) = (old.join(sep), new.join("\n"));
        let d = diff::lines(&a, &b);
        prop_assert_eq!(&d, &diff::lines(&a, &b), "deterministic");
        let mut back_old = Vec::new();
        let mut back_new = Vec::new();
        for l in &d {
            match l {
                diff::Line::Same { old, new, text } => {
                    prop_assert_eq!(*old, back_old.len() + 1);
                    prop_assert_eq!(*new, back_new.len() + 1);
                    back_old.push(text.as_str());
                    back_new.push(text.as_str());
                }
                diff::Line::Removed { old, text } => {
                    prop_assert_eq!(*old, back_old.len() + 1);
                    back_old.push(text.as_str());
                }
                diff::Line::Added { new, text } => {
                    prop_assert_eq!(*new, back_new.len() + 1);
                    back_new.push(text.as_str());
                }
            }
        }
        let al: Vec<&str> = a.lines().collect();
        let bl: Vec<&str> = b.lines().collect();
        prop_assert_eq!(&back_old, &al);
        prop_assert_eq!(&back_new, &bl);
        prop_assert!(diff::lines(&a, &a).iter().all(|l| !l.is_change()));
        let kept = d.iter().filter(|l| !l.is_change()).count();
        prop_assert_eq!(kept, naive_lcs(&al, &bl));
    }

    /// Diagram diffs: a module against itself differs nowhere; against any
    /// other, the old blocks less the removed and changed ones plus the
    /// added and the changed ones' new versions are exactly the new
    /// blocks, every id is a real block of its POU, and the same pair
    /// always gives the same diff.
    #[test]
    fn a_diagram_diff_is_empty_on_itself_and_rebuilds_the_target(
        a in proptest::collection::vec(stmt(), 1..5),
        b in proptest::collection::vec(stmt(), 0..4),
        keep in 0usize..5,
        early_return in any::<bool>(),
    ) {
        // Share a prefix, so edits look like edits.
        let k = keep.min(a.len());
        let mut b2: Vec<String> = a[..k].to_vec();
        b2.extend(b);
        if b2.is_empty() {
            b2.push("y := NOT y;".into());
        }
        let (sa, sb) = (program(&a, early_return), program(&b2, false));
        let (ma, mb) = (lowered(&sa)?, lowered(&sb)?);
        let same = diff::modules(&ma, &ma);
        prop_assert!(same.is_empty(), "{:?}", same);
        let d = diff::modules(&ma, &mb);
        prop_assert_eq!(&d, &diff::modules(&ma, &mb), "deterministic");
        let mut rebuilt = sigs(&ma);
        for c in &d.nodes {
            if let Some((id, s)) = &c.old {
                let pou = ma.pou(&c.pou).unwrap();
                prop_assert_eq!(&diff::signatures(&pou.name, &pou.diagram)[id.0 as usize], s);
                let at = rebuilt.iter().position(|x| x == s);
                prop_assert!(at.is_some(), "a removed or changed block was in the old set: {:?}", s);
                rebuilt.remove(at.unwrap());
            }
        }
        for c in &d.nodes {
            if let Some((id, s)) = &c.new {
                let pou = mb.pou(&c.pou).unwrap();
                prop_assert_eq!(&diff::signatures(&pou.name, &pou.diagram)[id.0 as usize], s);
                rebuilt.push(s.clone());
            }
            match c.change {
                diff::Change::Added => prop_assert!(c.old.is_none() && c.new.is_some()),
                diff::Change::Removed => prop_assert!(c.old.is_some() && c.new.is_none()),
                diff::Change::Changed => {
                    let (o, n) = (&c.old.as_ref().unwrap().1, &c.new.as_ref().unwrap().1);
                    prop_assert_eq!(&o.key, &n.key);
                    prop_assert!(o != n);
                }
            }
        }
        rebuilt.sort();
        prop_assert_eq!(rebuilt, sigs(&mb));
        if sa == sb {
            prop_assert!(d.is_empty());
        }
    }
}

fn naive_lcs(a: &[&str], b: &[&str]) -> usize {
    match (a.split_first(), b.split_first()) {
        (Some((x, ar)), Some((y, br))) if x == y => 1 + naive_lcs(ar, br),
        (Some((_, ar)), Some((_, br))) => naive_lcs(ar, b).max(naive_lcs(a, br)),
        _ => 0,
    }
}

#[test]
fn a_changed_condition_is_one_changed_write_not_a_new_block_set() {
    let old = program(&["x := a > 1;".into(), "y := NOT y;".into()], false);
    let new = program(
        &["x := a > 2;".into(), "y := NOT y;".into(), "c := c + 1;".into()],
        false,
    );
    let (mo, mn) = (lowered(&old).unwrap(), lowered(&new).unwrap());
    let d = diff::modules(&mo, &mn);
    let t = |c: &diff::NodeChange| {
        let s = c.new.as_ref().or(c.old.as_ref()).unwrap();
        format!("{:?} {} | {}", c.change, s.1.key.what, s.1.text)
    };
    let got: Vec<String> = d.nodes.iter().map(t).collect();
    assert_eq!(d.counts(), (3, 0, 3), "{got:#?}");
    assert!(
        got.contains(&"Changed op + | A + B + C".to_string()),
        "the read of C now comes from the new write: {got:#?}"
    );
    assert!(
        got.contains(&"Changed write X := | X := A > 2".to_string()),
        "{got:#?}"
    );
    assert!(
        got.contains(&"Added write C := | C := C + 1".to_string()),
        "{got:#?}"
    );
    assert_eq!(d.pous(), ["T"]);
    // Back the other way: the new statement's blocks are removed, the
    // changed ones change back; removed blocks are ghosted below.
    let back = diff::modules(&mn, &mo);
    assert_eq!(back.counts(), (0, 3, 3));
    let pou = mn.pou("T").unwrap();
    let removed = back.removed_nodes("T");
    let (rects, _) = diff::ghosts(&pou.diagram, &removed, 500.0, 40.0);
    assert_eq!(rects.len(), 3);
    let top = rects.iter().map(|(_, r)| r.y).fold(f32::INFINITY, f32::min);
    assert!(
        (top - 540.0).abs() < 1e-3,
        "ghosts start below the diagram: {top}"
    );
}

#[test]
fn a_changed_if_condition_changes_the_region_and_keeps_its_body() {
    let old = program(&["IF x THEN\nb := 1;\nEND_IF;".into()], false);
    let new = program(&["IF y THEN\nb := 1;\nEND_IF;".into()], false);
    let (mo, mn) = (lowered(&old).unwrap(), lowered(&new).unwrap());
    let d = diff::modules(&mo, &mn);
    assert_eq!(d.regions.len(), 1, "{d:#?}");
    assert_eq!(d.regions[0].change, diff::Change::Changed);
    assert!(
        d.nodes.iter().all(|c| !c
            .new
            .as_ref()
            .is_some_and(|(_, s)| s.key.what.starts_with("write B"))),
        "the body is unchanged: {d:#?}"
    );
}
