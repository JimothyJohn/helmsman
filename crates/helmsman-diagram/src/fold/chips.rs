//! A comparison against a constant as one block.
//!
//! `level > 80.0` lowers to three blocks: the variable, the constant and
//! the comparison. The constant says nothing on its own, so the two fold
//! into one block titled `> 80.0`, fed by the variable: `LEVEL` into
//! `> 80.0`. The variable keeps its own block (it is where an input is
//! set and watched). A constant on the left reads the same way round the
//! source does: `80.0 <`.

use super::{Fold, FoldKind};
use crate::model::{Diagram, NodeId, NodeKind};

const COMPARISONS: [&str; 6] = ["=", "<>", "<", "<=", ">", ">="];

/// One fold per comparison of `d` with a literal on exactly one side.
#[must_use]
pub fn chips(d: &Diagram) -> Vec<Fold> {
    let mut out = Vec::new();
    for n in &d.nodes {
        let NodeKind::Op { op } = &n.kind else { continue };
        if !COMPARISONS.contains(&op.as_str()) || n.ins.len() != 2 {
            continue;
        }
        let sides: Vec<_> = d.inputs(n.id).collect();
        let [a, b] = sides[..] else { continue };
        if a.negated || b.negated {
            continue;
        }
        let literal = |id: NodeId| matches!(d.node(id).kind, NodeKind::Const { .. });
        let (konst, title) = match (literal(a.from), literal(b.from)) {
            (false, true) => (b.from, format!("{op} {}", d.node(b.from).label)),
            (true, false) => (a.from, format!("{} {op}", d.node(a.from).label)),
            _ => continue,
        };
        // A literal is its own block, read here only; anything else
        // (a named constant is a variable) is left as it is.
        if d.wires.iter().filter(|w| w.from == konst).count() != 1 {
            continue;
        }
        let k = out.len();
        out.push(Fold::over_nodes(
            format!("chip:{}:{k}", n.text),
            FoldKind::Chip,
            title,
            [konst, n.id],
        ));
    }
    out
}
