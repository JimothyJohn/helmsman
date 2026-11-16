//! For any program the checker accepts: lowering succeeds without
//! panicking, every element maps to a statement of its POU, the layout
//! keeps its invariants and is deterministic, and a traced scan annotates
//! and explains every variable without panicking.
//!
//! The statement generator is the one from `helmsman-iec/tests/trace.rs`
//! (integration tests cannot share code across crates), extended with FB
//! output reads, `=>` outputs, computed indices, nested FB calls and jumps.

mod common;
#[path = "../../helmsman-iec/tests/spans_check/mod.rs"]
mod spans_check;

use std::time::Duration;

use common::{SIZES, check};
use helmsman_diagram::edit::{self, Bubble, Edit, Literal, Pin, Refusal, Source};
use helmsman_diagram::{
    Diagram, Module, NodeId, NodeKind, Span, Why, annotate, diff, drivers, explain, lower,
};
use helmsman_iec::ast::{BinOp, Pos, Stmt, StmtKind};
use helmsman_iec::{Image, Program, Ty, Value, compile};
use proptest::prelude::*;

use common::programs::{program, stmt};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Every statement and expression of a generated program, bare and
    /// with a comment between every pair of tokens, reads back from the
    /// text between its start and its end (helmsman-iec's `end` positions).
    #[test]
    fn every_statement_and_expression_reads_back_from_its_span(
        body in proptest::collection::vec(stmt(), 1..6),
        early_return in any::<bool>(),
    ) {
        let bare = program(&body, early_return);
        let noisy_body: Vec<String> = body.iter().map(|s| s.replace(' ', " (* c *) ")).collect();
        let noisy = program(&noisy_body, early_return);
        for src in [bare, noisy] {
            let n = spans_check::check(&src).map_err(|e| TestCaseError::fail(format!("{e}\n{src}")))?;
            prop_assert!(n > 0);
        }
    }

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
        NodeKind::Func { .. } | NodeKind::For { .. } | NodeKind::Jump | NodeKind::Fold { .. } => false,
    }
}

/// Every edit the diagram offers: a bubble on every pin and gate, a new
/// value for every literal, every operator swapped for each of its kin.
fn offered(m: &Module) -> Vec<Edit> {
    let mut out = Vec::new();
    for p in &m.pous {
        let d = &p.diagram;
        for n in &d.nodes {
            for input in 0..n.ins.len() {
                let pin = Pin {
                    pou: p.name.clone(),
                    node: n.id,
                    input,
                };
                out.push(Edit::ToggleNegation(Bubble::Pin(pin.clone())));
                let Some(w) = d.wires.iter().find(|w| w.to == n.id && w.inp == input) else {
                    continue;
                };
                let src = d.node(w.from);
                let value = match &src.kind {
                    NodeKind::Const {
                        value: Some(Value::Bool(b)),
                    } => Literal::Bool(!b),
                    NodeKind::Const {
                        value: Some(Value::Real(r)),
                    } => Literal::Real(r + 1.5),
                    NodeKind::Const {
                        value: Some(Value::Int(i)),
                    } if src.text.starts_with("T#") => Literal::Time(i + 1_000_000_000),
                    NodeKind::Const {
                        value: Some(Value::Int(i)),
                    } => Literal::Int(i + 1),
                    _ => continue,
                };
                out.push(Edit::SetConst { pin, value });
            }
            if let NodeKind::Op { op } = &n.kind {
                let family: &[BinOp] = match op.as_str() {
                    "AND" | "OR" => &[BinOp::And, BinOp::Or],
                    "=" | "<>" | "<" | "<=" | ">" | ">=" => {
                        &[BinOp::Eq, BinOp::Ne, BinOp::Lt, BinOp::Le, BinOp::Gt, BinOp::Ge]
                    }
                    "+" | "-" | "*" | "/" => &[BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div],
                    _ => &[],
                };
                // A run of one operator is one block with an input each:
                // it can only become an operator that also takes any number.
                let fits = |to: BinOp| n.ins.len() == 2 || helmsman_diagram::lower::chains(to);
                for &to in family.iter().filter(|b| b.symbol() != op && fits(**b)) {
                    out.push(Edit::SwapOp {
                        pou: p.name.clone(),
                        node: n.id,
                        to,
                    });
                }
            }
        }
        for r in &d.regions {
            out.push(Edit::ToggleNegation(Bubble::Gate {
                pou: p.name.clone(),
                region: r.id,
            }));
        }
    }
    out
}

/// The statements of the source's POUs as source ranges.
fn walk_stmts(body: &[Stmt], pou: &str, off: &dyn Fn(Pos) -> usize, out: &mut Vec<(String, Span)>) {
    for s in body {
        out.push((
            pou.to_string(),
            Span {
                start: off(s.pos),
                end: off(s.end),
            },
        ));
        match &s.kind {
            StmtKind::If(conds, other) => {
                for (_, b) in conds {
                    walk_stmts(b, pou, off, out);
                }
                walk_stmts(other, pou, off, out);
            }
            StmtKind::Case(_, arms, other) => {
                for (_, b) in arms {
                    walk_stmts(b, pou, off, out);
                }
                walk_stmts(other, pou, off, out);
            }
            StmtKind::For { body, .. } | StmtKind::While(_, body) | StmtKind::Repeat(body, _) => {
                walk_stmts(body, pou, off, out);
            }
            _ => {}
        }
    }
}

fn stmt_spans(src: &str) -> Vec<(String, Span)> {
    let mut starts = vec![0usize];
    starts.extend(
        src.bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i + 1),
    );
    let off = |p: Pos| starts[p.line as usize - 1] + p.col as usize - 1;
    let unit = helmsman_iec::ast::parse(src).unwrap();
    let mut out = Vec::new();
    for p in &unit.pous {
        walk_stmts(&p.body, &p.name, &off, &mut out);
    }
    out
}

/// The edits that change what feeds a block, remove a statement or rename
/// a variable: every pin tried against a few sources, every function block
/// call against its inputs, every statement deleted, every variable renamed.
fn offered_more(src: &str, m: &Module) -> Vec<Edit> {
    let mut out = Vec::new();
    for p in &m.pous {
        let d = &p.diagram;
        let writes: Vec<NodeId> = d
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Write { .. }))
            .map(|n| n.id)
            .collect();
        for n in &d.nodes {
            for input in 0..n.ins.len() {
                let pin = Pin {
                    pou: p.name.clone(),
                    node: n.id,
                    input,
                };
                let mut sources = vec![
                    Source::Var("x".into()),
                    Source::Var("a".into()),
                    Source::Const(Literal::Bool(true)),
                    Source::Const(Literal::Int(3)),
                    Source::Const(Literal::Time(2_000_000_000)),
                ];
                sources.extend(writes.iter().take(2).map(|w| Source::Write(*w)));
                sources.extend(writes.iter().rev().take(1).map(|w| Source::Write(*w)));
                for to in sources {
                    out.push(Edit::Rewire { pin: pin.clone(), to });
                }
            }
            if let NodeKind::Fb { .. } = &n.kind {
                for (arg, to) in [
                    ("PT", Source::Const(Literal::Time(7_000_000_000))),
                    ("IN", Source::Var("y".into())),
                    ("in", Source::Var("x".into())),
                    ("NOPE", Source::Var("x".into())),
                ] {
                    out.push(Edit::SetFbArg {
                        pou: p.name.clone(),
                        node: n.id,
                        arg: arg.into(),
                        to,
                    });
                }
            }
        }
        let mut names: Vec<&String> = p.vars.keys().collect();
        names.sort();
        for v in names {
            for to in ["renamed_var", "x", "IF"] {
                out.push(Edit::Rename {
                    pou: p.name.clone(),
                    from: v.clone(),
                    to: to.into(),
                });
            }
        }
    }
    for (pou, span) in stmt_spans(src) {
        out.push(Edit::DeleteStmt { pou, span });
    }
    // A range that is not a statement.
    out.push(Edit::DeleteStmt {
        pou: "T".into(),
        span: Span {
            start: 3,
            end: src.len() - 3,
        },
    });
    out
}

/// Whether refusing `e` with `r` is an honest answer for a program that
/// compiles: the diagram offers every bubble and every operator, so a
/// refusal must be about the thing (no bubble on a constant, a number is
/// not a condition, the types do not fit), never about the edit's own
/// bookkeeping.
fn honest(e: &Edit, r: &Refusal) -> bool {
    match r {
        Refusal::NoBubble | Refusal::NotBoolean | Refusal::NoText | Refusal::NoCondition => matches!(
            e,
            Edit::ToggleNegation(_) | Edit::Rewire { .. } | Edit::SetFbArg { .. }
        ),
        // `NOT TRUE` is a literal folded from an expression.
        Refusal::NotAValue(_) => matches!(e, Edit::SetConst { .. }),
        // A NOT on a number, or an operator the types do not take.
        Refusal::WouldNotCompile(_) => true,
        // Pins fed by blocks, sources of another type, writes not yet made,
        // a port that is not there: each about the thing asked for.
        Refusal::NotALeaf(_)
        | Refusal::TypeMismatch { .. }
        | Refusal::NotWrittenYet(_)
        | Refusal::NoSuchVariable { .. }
        | Refusal::NotAnInput { .. }
        | Refusal::NothingToChange => matches!(
            e,
            Edit::Rewire { .. }
                | Edit::SetFbArg { .. }
                | Edit::Rename { .. }
                | Edit::ToggleNegation(_)
                | Edit::SetConst { .. }
        ),
        Refusal::NameTaken(_) | Refusal::KeywordAsName(_) => matches!(e, Edit::Rename { .. }),
        // The range offered that is not one statement.
        Refusal::NotAStatement => matches!(e, Edit::DeleteStmt { .. }),
        _ => false,
    }
}

/// The source of the block changed: ids whose signature differs.
fn changed_ids(a: &Module, b: &Module, pou: &str) -> Vec<NodeId> {
    let (pa, pb) = (a.pou(pou).unwrap(), b.pou(pou).unwrap());
    let (sa, sb) = (
        diff::signatures(&pa.name, &pa.diagram),
        diff::signatures(&pb.name, &pb.diagram),
    );
    (0..sa.len())
        .filter(|&i| sa[i] != sb[i])
        .map(|i| NodeId(u32::try_from(i).unwrap()))
        .collect()
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// Every edit the diagram offers, on a program bare and with a
    /// comment between every pair of tokens: it is made or refused for a
    /// reason about the thing edited; when made, the bytes outside the
    /// patch are the original's, the result compiles, the inverse patch
    /// restores the original bytes exactly, no comment is lost, and the
    /// diagram differs in the addressed block (and the blocks holding its
    /// text) alone.
    #[test]
    fn every_offered_edit_is_exact_checked_and_undoable(
        body in proptest::collection::vec(stmt(), 1..4),
        early_return in any::<bool>(),
        noisy in any::<bool>(),
    ) {
        let body: Vec<String> = if noisy {
            body.iter().map(|s| s.replace(' ', " (* c *) ")).collect()
        } else {
            body
        };
        let src = program(&body, early_return);
        let m = lowered(&src)?;
        let comments = src.matches("(* c *)").count();
        let mut made = 0;
        for e in offered(&m) {
            match edit::apply(&src, &m, e.clone()) {
                Err(r) => prop_assert!(honest(&e, &r), "{e:?} refused: {r}\n{src}"),
                Ok(patch) => {
                    made += 1;
                    prop_assert!(
                        patch.start <= patch.end && patch.end <= src.len()
                            && src.is_char_boundary(patch.start) && src.is_char_boundary(patch.end),
                        "{patch:?}"
                    );
                    let out = patch.apply_to(&src);
                    prop_assert_eq!(&out[..patch.start], &src[..patch.start]);
                    prop_assert_eq!(&out[patch.start + patch.text.len()..], &src[patch.end..]);
                    prop_assert_eq!(out.matches("(* c *)").count(), comments, "{:?}\n{}", patch, out);
                    prop_assert_eq!(&patch.inverse(&src).apply_to(&out), &src);
                    let m2 = lowered(&out)?;
                    let (pou, target, swapped) = match &e {
                        Edit::SetConst { pin, .. } | Edit::ToggleNegation(Bubble::Pin(pin)) => {
                            (pin.pou.clone(), Some(pin.node), matches!(e, Edit::SetConst { .. }))
                        }
                        Edit::SwapOp { pou, node, .. } => (pou.clone(), Some(*node), true),
                        Edit::ToggleNegation(Bubble::Gate { pou, .. }) => (pou.clone(), None, false),
                        Edit::Rewire { .. } | Edit::SetFbArg { .. } | Edit::DeleteStmt { .. } | Edit::Rename { .. } => {
                            unreachable!("offered by offered_more")
                        }
                    };
                    let stmt_of = |id: NodeId| m.pou(&pou).unwrap().diagram.node(id).stmt;
                    let changed = changed_ids(&m, &m2, &pou);
                    for id in &changed {
                        prop_assert_eq!(Some(stmt_of(*id)), target.map(stmt_of), "{:?} changed {:?}\n{}", e, id, out);
                    }
                    prop_assert_eq!(target.is_some(), !changed.is_empty());
                    let d = diff::modules(&m, &m2);
                    let (added, removed, _) = d.counts();
                    prop_assert_eq!(added, removed, "{:?}", d);
                    prop_assert!(!swapped || added >= 1, "{:?}", d);
                    prop_assert!(d.pous_added.is_empty() && d.pous_removed.is_empty());
                    // The same toggle again puts the bubble back (`NOT NOT x`
                    // loses a NOT each time, so the bytes may differ).
                    if let Edit::ToggleNegation(_) = &e {
                        let again = edit::apply(&out, &m2, e.clone()).map_err(|r| TestCaseError::fail(format!("{r}\n{out}")))?;
                        let m3 = lowered(&again.apply_to(&out))?;
                        let bubbles = |m: &Module| -> Vec<bool> {
                            m.pous.iter().flat_map(|p| p.diagram.wires.iter().map(|w| w.negated)
                                .chain(p.diagram.regions.iter().map(|r| r.negated))).collect()
                        };
                        prop_assert_eq!(bubbles(&m3), bubbles(&m), "{:?}\n{}", e, out);
                    }
                }
            }
        }
        prop_assert!(made > 0, "something is always editable\n{src}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    /// The edits that rewire, set an argument, delete or rename, offered
    /// on every pin, call, statement and variable of generated programs
    /// (bare and with a comment between every pair of tokens): when one is
    /// made, the bytes outside its patch are the original's, the result
    /// compiles and lowers, the inverse restores the original exactly, and
    /// the shape is what the edit says (a rename keeps every block and
    /// wire, a delete takes only statements, a rewire keeps the statement
    /// list); when it is refused, the reason is about the thing asked for.
    #[test]
    fn every_offered_rewire_argument_delete_and_rename_is_exact_checked_and_undoable(
        body in proptest::collection::vec(stmt(), 1..4),
        early_return in any::<bool>(),
        noisy in any::<bool>(),
    ) {
        let body: Vec<String> = if noisy {
            body.iter().map(|s| s.replace(' ', " (* c *) ")).collect()
        } else {
            body
        };
        let src = program(&body, early_return);
        let m = lowered(&src)?;
        let mut made = [0usize; 4];
        for e in offered_more(&src, &m) {
            match edit::apply(&src, &m, e.clone()) {
                Err(r) => prop_assert!(honest(&e, &r), "{e:?} refused: {r}\n{src}"),
                Ok(patch) => {
                    prop_assert!(
                        patch.start <= patch.end && patch.end <= src.len()
                            && src.is_char_boundary(patch.start) && src.is_char_boundary(patch.end),
                        "{patch:?}"
                    );
                    let out = patch.apply_to(&src);
                    prop_assert_eq!(&out[..patch.start], &src[..patch.start]);
                    prop_assert_eq!(&out[patch.start + patch.text.len()..], &src[patch.end..]);
                    prop_assert_eq!(&patch.inverse(&src).apply_to(&out), &src);
                    let m2 = lowered(&out)?;
                    prop_assert_eq!(m.pous.len(), m2.pous.len());
                    let (a, b) = (&m.program().unwrap().diagram, &m2.program().unwrap().diagram);
                    match &e {
                        Edit::Rename { .. } => {
                            made[0] += 1;
                            prop_assert_eq!(a.nodes.len(), b.nodes.len());
                            prop_assert_eq!(a.wires.len(), b.wires.len());
                            prop_assert_eq!(a.regions.len(), b.regions.len());
                            prop_assert_eq!(a.stmts.len(), b.stmts.len());
                        }
                        Edit::DeleteStmt { .. } => {
                            made[1] += 1;
                            prop_assert!(out.len() < src.len());
                        }
                        Edit::Rewire { .. } => {
                            made[2] += 1;
                            prop_assert_eq!(a.stmts.len(), b.stmts.len());
                            prop_assert_eq!(a.regions.len(), b.regions.len());
                        }
                        _ => {
                            made[3] += 1;
                            prop_assert_eq!(a.stmts.len(), b.stmts.len());
                        }
                    }
                }
            }
        }
        prop_assert!(made.iter().all(|&n| n > 0), "each kind is made at least once: {made:?}\n{src}");
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
