//! Control idioms recognised on the diagram: the seal-in latch, the edge
//! detector, the two-way select, the set/reset IF and the range test.
//!
//! Each rule is a function from a [`Diagram`] to the [`Fold`]s it finds.
//! None guesses: a shape that differs by one wire, one extra input, a
//! negation bubble on the other pin or another variable is left as it is.
//! Every fold returned applies on its own ([`super::view`] skips none of
//! them). Rules do not look at each other, so two of them can claim the
//! same blocks (a range's comparisons are also [`super::chips`]'
//! comparisons); the caller orders them, the first claim winning.
//!
//! A key is the rule, the variable and the number of folds of the same
//! rule and variable before it in the diagram (`latch:M:0`), so it
//! survives edits elsewhere in the source.

use std::collections::BTreeMap;

use helmsman_iec::Value;

use super::{Fold, FoldKind};
use crate::model::{Diagram, NodeId, NodeKind, RegionKind, Wire};

/// The variable a block reads or writes (`M`, `T.ET`), if that is what it is.
fn variable(d: &Diagram, n: NodeId) -> Option<&str> {
    match &d.node(n).kind {
        NodeKind::Read { path } | NodeKind::Write { path } => Some(path),
        _ => None,
    }
}

/// How many places read output of `n`: wires and region gates.
fn consumers(d: &Diagram, n: NodeId) -> usize {
    d.wires.iter().filter(|w| w.from == n).count()
        + d.regions
            .iter()
            .filter(|r| r.gate.is_some_and(|(g, _)| g == n))
            .count()
}

/// The blocks of one binary operator `op` with exactly two input wires.
fn binary<'a>(d: &'a Diagram, n: NodeId, op: &str) -> Option<[&'a Wire; 2]> {
    let node = d.node(n);
    if !matches!(&node.kind, NodeKind::Op { op: o } if o == op) || node.ins.len() != 2 {
        return None;
    }
    let ws: Vec<&Wire> = d.inputs(n).collect();
    <[&Wire; 2]>::try_from(ws).ok()
}

/// The nth fold of `rule` over `var` so far, as the key's tail.
pub(super) fn key(seen: &mut BTreeMap<String, usize>, rule: &str, var: &str) -> String {
    let k = format!("{rule}:{var}");
    let n = seen.entry(k.clone()).or_insert(0);
    *n += 1;
    format!("{k}:{}", *n - 1)
}

/// Whether all of `nodes` are one statement in one region.
fn one_place(d: &Diagram, nodes: &[NodeId]) -> bool {
    let first = d.node(nodes[0]);
    nodes
        .iter()
        .all(|&n| (d.node(n).stmt, d.node(n).region) == (first.stmt, first.region))
}

/// The seal-in `m := (set OR m) AND NOT reset;`, in any operand order.
///
/// Exactly a two-input OR feeding a two-input AND (nothing else reads the
/// OR), the AND's other input entering through a negation bubble, and the
/// AND feeding only the write of the variable that the OR also reads (a
/// read, or the wire from an earlier write of it). A third input on
/// either gate, a bubble on the set or on the OR's wire, a feedback of a
/// different variable, or the same signal as set and reset, leave the
/// blocks alone. The fold is the OR and the AND, titled with the
/// variable, with pins `set` and `reset` named (the feedback keeps its
/// variable's name).
#[must_use]
pub fn latches(d: &Diagram) -> Vec<Fold> {
    let mut out = Vec::new();
    let mut seen = BTreeMap::new();
    for and in d.nodes.iter().map(|n| n.id) {
        let Some(ands) = binary(d, and, "AND") else {
            continue;
        };
        // The one write the AND feeds, un-negated.
        let [sink] = d.wires.iter().filter(|w| w.from == and).collect::<Vec<_>>()[..] else {
            continue;
        };
        let Some(var) = variable(d, sink.to).filter(|_| {
            !sink.negated && matches!(d.node(sink.to).kind, NodeKind::Write { .. }) && consumers(d, and) == 1
        }) else {
            continue;
        };
        for (or_w, reset) in [(ands[0], ands[1]), (ands[1], ands[0])] {
            if or_w.negated || !reset.negated {
                continue;
            }
            let Some(ors) = binary(d, or_w.from, "OR") else {
                continue;
            };
            if ors[0].negated || ors[1].negated || consumers(d, or_w.from) != 1 {
                continue;
            }
            let is_fb = |w: &Wire| variable(d, w.from) == Some(var);
            let (fb, set) = match (is_fb(ors[0]), is_fb(ors[1])) {
                (true, false) => (ors[0], ors[1]),
                (false, true) => (ors[1], ors[0]),
                _ => continue,
            };
            let (s, r) = ((set.from, set.out), (reset.from, reset.out));
            if s == r
                || s == (fb.from, fb.out)
                || r == (fb.from, fb.out)
                || variable(d, reset.from) == Some(var)
            {
                continue;
            }
            // The variable read back only to hold itself is part of the
            // latch; one wired from an earlier write, or read elsewhere in
            // the statement too, stays an input by its name.
            let own = matches!(d.node(fb.from).kind, NodeKind::Read { .. }) && consumers(d, fb.from) == 1;
            let mut nodes = vec![or_w.from, and];
            if own {
                nodes.push(fb.from);
            }
            if !one_place(d, &nodes) {
                continue;
            }
            let mut f = Fold::over_nodes(key(&mut seen, "latch", var), FoldKind::Latch, var, nodes);
            f.names.insert(s, "set".into());
            f.names.insert(r, "reset".into());
            out.push(f);
            break;
        }
    }
    out
}

/// The statements of the block holding statement `s` (the top of the POU
/// or one arm), and where `s` is in them.
fn block_of(d: &Diagram, s: usize) -> (&[usize], usize) {
    let block: &[usize] = d.stmts[s].region.map_or(&d.top, |r| &d.region(r).body);
    let at = block.iter().position(|&x| x == s).unwrap_or(0);
    (block, at)
}

/// The statement index of the statement `n` was lowered from.
fn stmt_index(d: &Diagram, n: NodeId) -> Option<usize> {
    d.stmts.iter().position(|s| s.id == d.node(n).stmt)
}

/// A statement that is exactly `var := value;` to a plain variable: the
/// write and the wire into it.
fn assignment(d: &Diagram, s: usize) -> Option<(NodeId, &str, &Wire)> {
    let st = d.stmts.get(s)?;
    let [root] = st.roots[..] else { return None };
    if !st.arms.is_empty() {
        return None;
    }
    let NodeKind::Write { path } = &d.node(root).kind else {
        return None;
    };
    if path.contains('[') || d.node(root).ins.len() != 1 || d.node(root).stmt != st.id {
        return None;
    }
    let [w] = d.inputs(root).collect::<Vec<_>>()[..] else {
        return None;
    };
    (!w.negated).then_some((root, path.as_str(), w))
}

/// The writes of `path`.
fn writes<'a>(d: &'a Diagram, path: &'a str) -> impl Iterator<Item = &'a crate::model::Node> {
    d.nodes
        .iter()
        .filter(move |n| matches!(&n.kind, NodeKind::Write { path: p } if p == path))
}

/// Whether, after statement `at` in its block, `prev := sig;` is the only
/// assignment of `prev` in the diagram and `sig` is not written from `at`
/// up to it.
fn follows(d: &Diagram, at: usize, prev: &str, sig: &str) -> bool {
    let (block, pos) = block_of(d, at);
    if writes(d, prev).count() != 1 {
        return false;
    }
    let Some(&pair) = block[pos + 1..].iter().find(|&&s| {
        assignment(d, s).is_some_and(|(_, path, w)| path == prev && variable(d, w.from) == Some(sig))
    }) else {
        return false;
    };
    !writes(d, sig).any(|n| stmt_index(d, n.id).is_some_and(|s| (at..pair).contains(&s)))
}

/// A rising edge `p := x AND NOT x_prev;` or a falling one
/// `p := NOT x AND x_prev;`, only where a later statement of the same
/// block is `x_prev := x;` (the only assignment of `x_prev` anywhere, and
/// `x` not written between). The fold is the two-input AND with one
/// plain and one negated input from two different variables, titled
/// `rising X` or `falling X`. Without that statement, or with the
/// pairing the other way round as well, nothing is folded.
#[must_use]
pub fn edges(d: &Diagram) -> Vec<Fold> {
    let mut out = Vec::new();
    let mut seen = BTreeMap::new();
    for and in d.nodes.iter().map(|n| n.id) {
        let Some(ws) = binary(d, and, "AND") else { continue };
        let [plain, bubble] = match (ws[0].negated, ws[1].negated) {
            (false, true) => [ws[0], ws[1]],
            (true, false) => [ws[1], ws[0]],
            _ => continue,
        };
        let (Some(p), Some(b), Some(at)) = (
            variable(d, plain.from),
            variable(d, bubble.from),
            stmt_index(d, and),
        ) else {
            continue;
        };
        if p == b {
            continue;
        }
        // `x AND NOT x_prev` with `x_prev := x` after; or `x_prev AND NOT
        // x` with `x_prev := x` after, the falling form.
        let rising = follows(d, at, b, p);
        let falling = follows(d, at, p, b);
        let (word, signal) = match (rising, falling) {
            (true, false) => ("rising", p),
            (false, true) => ("falling", b),
            _ => continue,
        };
        out.push(Fold::over_nodes(
            key(&mut seen, word, signal),
            FoldKind::Edge,
            format!("{word} {signal}"),
            [and],
        ));
    }
    out
}

/// What a value source is called in a title: a variable by name, a
/// literal as written, anything else by its text.
fn shown(d: &Diagram, n: NodeId) -> String {
    let node = d.node(n);
    match &node.kind {
        NodeKind::Read { path } | NodeKind::Write { path } => path.clone(),
        NodeKind::Const { .. } => node.label.clone(),
        _ => node.text.clone(),
    }
}

/// `IF c THEN y := a; ELSE y := b; END_IF;` with exactly one assignment in
/// each arm, both to the same plain variable.
///
/// Folds the whole IF statement, titled `Y := A or B` (the two values as
/// the program shows them, `A` for the THEN arm; `Y := one of two` when
/// that would pass 40 characters). An ELSIF, a second statement in an
/// arm, a nested IF, an arm assigning another variable or an element of an
/// array is not matched.
#[must_use]
pub fn selects(d: &Diagram) -> Vec<Fold> {
    let mut out = Vec::new();
    let mut seen = BTreeMap::new();
    for (i, st) in d.stmts.iter().enumerate() {
        let [then, other] = st.arms[..] else { continue };
        if (d.region(then).kind, d.region(other).kind) != (RegionKind::If, RegionKind::Else) {
            continue;
        }
        let (&[a], &[b]) = (&d.region(then).body[..], &d.region(other).body[..]) else {
            continue;
        };
        let (Some((_, pa, wa)), Some((_, pb, wb))) = (assignment(d, a), assignment(d, b)) else {
            continue;
        };
        if pa != pb {
            continue;
        }
        let title = format!("{pa} := {} or {}", shown(d, wa.from), shown(d, wb.from));
        let title = if title.chars().count() > 40 {
            format!("{pa} := one of two")
        } else {
            title
        };
        out.push(Fold::over_stmts(
            d,
            key(&mut seen, "select", pa),
            FoldKind::Select,
            title,
            &[i],
        ));
    }
    out
}

/// An IF (with ELSIF arms, without ELSE) in which every arm is exactly one
/// assignment of the literal TRUE or FALSE to the same plain variable.
///
/// Folds the whole IF statement, titled `set Y` when every arm assigns
/// TRUE, `reset Y` when every arm assigns FALSE and `set/reset Y` for a
/// mix. An ELSE arm is not matched (with literals in both arms it is a
/// [`selects`] shape); neither is an arm with another statement, another
/// variable, or a value that is not a literal.
#[must_use]
pub fn set_resets(d: &Diagram) -> Vec<Fold> {
    let mut out = Vec::new();
    let mut seen = BTreeMap::new();
    for (i, st) in d.stmts.iter().enumerate() {
        if st.arms.is_empty()
            || !st.arms.iter().enumerate().all(|(k, &r)| {
                let kind = d.region(r).kind;
                if k == 0 {
                    kind == RegionKind::If
                } else {
                    kind == RegionKind::Elsif
                }
            })
        {
            continue;
        }
        let mut var: Option<&str> = None;
        let mut values = Vec::new();
        for &r in &st.arms {
            let &[s] = &d.region(r).body[..] else { break };
            let Some((_, path, w)) = assignment(d, s) else {
                break;
            };
            let NodeKind::Const {
                value: Some(Value::Bool(v)),
            } = d.node(w.from).kind
            else {
                break;
            };
            if var.is_some_and(|p| p != path) {
                break;
            }
            var = Some(path);
            values.push(v);
        }
        let Some(var) = var.filter(|_| values.len() == st.arms.len()) else {
            continue;
        };
        let verb = if values.iter().all(|&v| v) {
            "set"
        } else if values.iter().all(|&v| !v) {
            "reset"
        } else {
            "set/reset"
        };
        out.push(Fold::over_stmts(
            d,
            key(&mut seen, "setreset", var),
            FoldKind::SetReset,
            format!("{verb} {var}"),
            &[i],
        ));
    }
    out
}

/// One comparison of a source against a literal, as a bound on the source.
struct Bound {
    cmp: NodeId,
    konst: NodeId,
    source: (NodeId, usize),
    /// The source is above (true) or below (false) the literal.
    lower: bool,
    strict: bool,
}

fn bound(d: &Diagram, n: NodeId) -> Option<Bound> {
    let NodeKind::Op { op } = &d.node(n).kind else {
        return None;
    };
    if !["<", "<=", ">", ">="].contains(&op.as_str()) {
        return None;
    }
    let ws: Vec<&Wire> = d.inputs(n).collect();
    let [a, b] = ws[..] else { return None };
    let is_const = |w: &Wire| matches!(d.node(w.from).kind, NodeKind::Const { .. });
    if a.negated || b.negated || d.node(n).ins.len() != 2 || consumers(d, n) != 1 {
        return None;
    }
    // `source op literal`, or the literal first, which flips the sense.
    let (source, konst, flip) = match (is_const(a), is_const(b)) {
        (false, true) => (a, b, false),
        (true, false) => (b, a, true),
        _ => return None,
    };
    if consumers(d, konst.from) != 1 {
        return None;
    }
    let above = op.starts_with('>');
    Some(Bound {
        cmp: n,
        konst: konst.from,
        source: (source.from, source.out),
        lower: above != flip,
        strict: op.len() == 1,
    })
}

/// `lo <= x AND x <= hi`: one variable between two literal bounds.
///
/// Both comparisons read the same source (one block, one output), each
/// against its own literal on either side, one a lower and one an upper
/// bound, strict or not; the AND has just those two inputs, no bubble,
/// and the comparisons and literals feed nothing else. The fold covers the
/// two comparisons, the two literals and the AND, with the source as its
/// one input pin. The title is `10 .. 20` when both ends are included,
/// otherwise each bound's operator is shown: `10 < .. <= 20` is
/// `10 < x AND x <= 20`. Two lower bounds, two upper bounds, two sources
/// or a non-literal bound are not matched.
#[must_use]
pub fn ranges(d: &Diagram) -> Vec<Fold> {
    let mut out = Vec::new();
    let mut seen = BTreeMap::new();
    for and in d.nodes.iter().map(|n| n.id) {
        let Some(ws) = binary(d, and, "AND") else { continue };
        if ws[0].negated || ws[1].negated || ws[0].from == ws[1].from {
            continue;
        }
        let (Some(x), Some(y)) = (bound(d, ws[0].from), bound(d, ws[1].from)) else {
            continue;
        };
        if x.source != y.source
            || x.lower == y.lower
            || matches!(d.node(x.source.0).kind, NodeKind::Const { .. })
        {
            continue;
        }
        let (lo, hi) = if x.lower { (&x, &y) } else { (&y, &x) };
        let nodes = [lo.cmp, lo.konst, hi.cmp, hi.konst, and];
        if !one_place(d, &nodes) || nodes.iter().collect::<std::collections::BTreeSet<_>>().len() != 5 {
            continue;
        }
        let (a, b) = (&d.node(lo.konst).label, &d.node(hi.konst).label);
        let title = if lo.strict || hi.strict {
            let op = |s: bool| if s { "<" } else { "<=" };
            format!("{a} {} .. {} {b}", op(lo.strict), op(hi.strict))
        } else {
            format!("{a} .. {b}")
        };
        let name = shown(d, lo.source.0);
        out.push(Fold::over_nodes(
            key(&mut seen, "range", &name),
            FoldKind::Range,
            title,
            nodes,
        ));
    }
    out
}
