//! An interlock as a table: an OR of ANDs drawn as rows of conditions.
//!
//! `run := (a AND b AND NOT c) OR (d AND e) OR f;` lowers to an OR block
//! fed by two AND blocks and a signal. Drawn as gates, which signal
//! belongs to which way of running is a matter of following wires. The
//! rule ([`tables`]) makes it one [`FoldKind::Table`] fold: a row per
//! term, a cell per signal in it, each cell the signal and whether the row
//! needs it on or off ([`FoldCell`]). The canvas lights the row that
//! holds, and when the output is off marks in each row the cell that
//! stops it.
//!
//! What is a table:
//!
//! - An OR of at least two terms, one of them an AND of two signals or
//!   more, each term an AND (an operand of the OR that is a block of its
//!   own) or a single signal.
//! - Every signal a plain BOOL read, optionally negated: a variable, an
//!   element or output of a function block (`m.run`, `bits[2]`), a
//!   variable an earlier statement wrote, or a comparison with a literal
//!   (`level > 80.0`, the comparison [`super::chips`] folds), which
//!   reads as one cell, named by its text.
//! - The AND and OR operators are BOOL by the declarations
//!   ([`Diagram::bools`]): on a WORD they are bitwise, which a cell cannot
//!   say.
//!
//! Left as it is, the whole expression: a term that is anything else (a
//! nested OR, a call, a comparison of two variables, arithmetic, a
//! negated AND, a literal), an AND read by anything but the OR, an OR that
//! is itself an operand of an AND, OR or XOR (it is part of a larger
//! expression, not the interlock), one term only, and terms that are all
//! single signals (a plain OR of signals needs no table). The dual, an
//! AND of ORs, is not a table either.
//!
//! The fold stands for the OR and its ANDs. Its pins are the distinct
//! signals, one each however many rows use it (the cells carry the bubbles,
//! so a signal used on in one row and off in another is still one pin;
//! [`super::view`]), and its output is the OR's. Its key is
//! `table:{output variable}:{n}`.

use std::collections::BTreeMap;

use super::chips::chip_of;
use super::idioms::{consumers, key, one_place};
use super::{Fold, FoldCell, FoldKind, FoldTable, source_name};
use crate::model::{Diagram, NodeId, NodeKind, Wire};

/// One fold per interlock of `d`. See the module docs.
#[must_use]
pub fn tables(d: &Diagram) -> Vec<Fold> {
    let mut seen = BTreeMap::new();
    let mut out = Vec::new();
    for n in &d.nodes {
        if !is_bool_op(d, n.id, "OR") {
            continue;
        }
        let Some((rows, ands)) = rows_of(d, n.id) else {
            continue;
        };
        let title = d
            .wires
            .iter()
            .filter(|w| w.from == n.id)
            .map(|w| &d.node(w.to).kind)
            .collect::<Vec<_>>();
        let title = match title[..] {
            [NodeKind::Write { path }] => path.clone(),
            _ => "permissive".to_string(),
        };
        let nodes = std::iter::once(n.id).chain(ands);
        let fold = Fold::over_nodes(key(&mut seen, "table", &title), FoldKind::Table, title, nodes)
            .with_table(FoldTable { rows, out: n.id });
        out.push(fold);
    }
    out
}

/// Whether `n` is a `sym` block the declarations show is on BOOLs.
fn is_bool_op(d: &Diagram, n: NodeId, sym: &str) -> bool {
    d.bools.contains(&n) && matches!(&d.node(n).kind, NodeKind::Op { op } if op == sym)
}

/// The rows of the OR `or` and the ANDs behind them, if it is a table.
fn rows_of(d: &Diagram, or: NodeId) -> Option<(Vec<Vec<FoldCell>>, Vec<NodeId>)> {
    let node = d.node(or);
    let terms: Vec<&Wire> = d.inputs(or).collect();
    if terms.len() < 2 || terms.len() != node.ins.len() {
        return None;
    }
    // Part of a larger expression: not the interlock.
    let combined = d.wires.iter().filter(|w| w.from == or).any(
        |w| matches!(&d.node(w.to).kind, NodeKind::Op { op } if matches!(op.as_str(), "AND" | "OR" | "XOR")),
    );
    if combined {
        return None;
    }
    let (mut rows, mut ands) = (Vec::new(), Vec::new());
    for t in terms {
        let and = t.from;
        if !t.negated && t.out == 0 && is_bool_op(d, and, "AND") {
            let ins: Vec<&Wire> = d.inputs(and).collect();
            if consumers(d, and) != 1 || ins.len() < 2 || ins.len() != d.node(and).ins.len() {
                return None;
            }
            if !one_place(d, &[or, and]) {
                return None;
            }
            rows.push(ins.into_iter().map(|w| cell(d, w)).collect::<Option<Vec<_>>>()?);
            ands.push(and);
        } else {
            rows.push(vec![cell(d, t)?]);
        }
    }
    rows.iter().any(|r| r.len() > 1).then_some((rows, ands))
}

/// The cell for the wire `w` into an AND or the OR: a plain signal or a
/// comparison with a literal.
fn cell(d: &Diagram, w: &Wire) -> Option<FoldCell> {
    let n = d.node(w.from);
    let label = match &n.kind {
        NodeKind::Read { .. } | NodeKind::Write { .. } | NodeKind::Access { .. } | NodeKind::Fb { .. } => {
            source_name(n, w.out)
        }
        NodeKind::Op { .. } => {
            let (konst, title) = chip_of(d, n.id)?;
            let sides: Vec<&Wire> = d.inputs(n.id).collect();
            let var = sides.iter().find(|s| s.from != konst)?;
            // The variable side is a plain signal too: `ABS(n) > 3` or
            // `n + 1 > 3` is arithmetic, not a condition on a signal.
            if !matches!(
                d.node(var.from).kind,
                NodeKind::Read { .. }
                    | NodeKind::Write { .. }
                    | NodeKind::Access { .. }
                    | NodeKind::Fb { .. }
            ) {
                return None;
            }
            let name = source_name(d.node(var.from), var.out);
            if sides[0].from == konst {
                format!("{title} {name}")
            } else {
                format!("{name} {title}")
            }
        }
        _ => return None,
    };
    Some(FoldCell {
        from: (w.from, w.out),
        negated: w.negated,
        label,
    })
}
