//! A diagram with one traced scan on it: a value per node output (so per
//! wire), and per region whether its gate let it run.
//!
//! Variables read and written show their value when the scan ended (the
//! trace's values). Logic and comparisons in between are re-evaluated from
//! those, so a gate shows the value its condition has now: what the scan
//! saw unless the scan itself changed an input after testing it.
//! Arithmetic, functions and computed indices are not re-evaluated (their
//! overflow and rounding are the interpreter's business): no value.
//!
//! A POU other than the program (a function block) is viewed through one
//! instance: `prefix` is its path with a trailing dot (`M.`). Its statement
//! counts are totals over every instance (see [`helmsman_iec::trace`]).

use helmsman_iec::Value;
use helmsman_iec::trace::Trace;

use crate::model::{Diagram, NodeKind, Region, RegionKind};

/// What a region's gate did this scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Its IF/CASE/loop statement did not run.
    NotReached,
    /// The statement ran; this arm was not taken (or the loop made no pass).
    Closed,
    /// Taken this many times (arm hits, or loop passes).
    Open(u32),
}

impl Gate {
    #[must_use]
    pub const fn open(self) -> bool {
        matches!(self, Self::Open(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// Per node, per output port.
    pub values: Vec<Vec<Option<Value>>>,
    /// Per node: whether its statement ran this scan.
    pub ran: Vec<bool>,
    /// Per region.
    pub gates: Vec<Gate>,
}

impl Live {
    /// The value on wire `w` of the diagram.
    #[must_use]
    pub fn wire(&self, d: &Diagram, w: usize) -> Option<Value> {
        let w = d.wires.get(w)?;
        self.values.get(w.from.0 as usize)?.get(w.out).copied().flatten()
    }
}

/// How `r`'s gate did in `t`.
#[must_use]
pub fn gate(t: &Trace, r: &Region) -> Gate {
    if !t.ran(r.stmt) {
        return Gate::NotReached;
    }
    let hits = match r.kind {
        RegionKind::For | RegionKind::While | RegionKind::Repeat => t.passes(r.stmt).unwrap_or(0),
        _ => t.arm_hits(r.stmt).get(r.arm).copied().unwrap_or(0),
    };
    if hits > 0 { Gate::Open(hits) } else { Gate::Closed }
}

/// `d` (the POU at `prefix`) with `t` on it.
#[must_use]
pub fn annotate(d: &Diagram, t: &Trace, prefix: &str) -> Live {
    let get = |path: &str| t.get(&format!("{prefix}{path}"));
    let mut srcs: Vec<[Option<(usize, usize)>; 2]> = vec![[None; 2]; d.nodes.len()];
    for w in &d.wires {
        if let Some(slot) = srcs[w.to.0 as usize].get_mut(w.inp) {
            *slot = Some((w.from.0 as usize, w.out));
        }
    }
    let mut values: Vec<Vec<Option<Value>>> = Vec::with_capacity(d.nodes.len());
    for n in &d.nodes {
        // Inputs come from nodes created earlier, so already valued.
        let input = |k: usize| -> Option<Value> {
            let (from, out) = srcs[n.id.0 as usize][k]?;
            values.get(from)?.get(out).copied().flatten()
        };
        let out: Vec<Option<Value>> = match &n.kind {
            NodeKind::Read { path } | NodeKind::Write { path } => vec![get(path)],
            NodeKind::For { var } => vec![get(var)],
            NodeKind::Const { value } => vec![*value],
            NodeKind::Fb { instance, .. } => n.outs.iter().map(|o| get(&format!("{instance}.{o}"))).collect(),
            NodeKind::Op { op } => vec![eval(op, input(0), input(1))],
            NodeKind::Func { .. } | NodeKind::Access { .. } | NodeKind::Jump => vec![None; n.outs.len()],
        };
        values.push(out);
    }
    Live {
        values,
        ran: d.nodes.iter().map(|n| t.ran(n.stmt)).collect(),
        gates: d.regions.iter().map(|r| gate(t, r)).collect(),
    }
}

fn num(v: Value) -> Option<f64> {
    match v {
        #[allow(clippy::cast_precision_loss)]
        Value::Int(i) => Some(i as f64),
        Value::Real(r) => Some(r),
        Value::Bool(_) => None,
    }
}

/// Logic and comparisons only; see the module docs.
fn eval(op: &str, a: Option<Value>, b: Option<Value>) -> Option<Value> {
    use Value::{Bool, Int};
    let a = a?;
    if op == "NOT" {
        return match a {
            Bool(x) => Some(Bool(!x)),
            _ => None,
        };
    }
    let b = b?;
    Some(match (op, a, b) {
        ("AND", Bool(x), Bool(y)) => Bool(x && y),
        ("OR", Bool(x), Bool(y)) => Bool(x || y),
        ("XOR", Bool(x), Bool(y)) => Bool(x ^ y),
        ("=", Bool(x), Bool(y)) => Bool(x == y),
        ("<>", Bool(x), Bool(y)) => Bool(x != y),
        (_, Int(x), Int(y)) => Bool(match op {
            "=" => x == y,
            "<>" => x != y,
            "<" => x < y,
            "<=" => x <= y,
            ">" => x > y,
            ">=" => x >= y,
            _ => return None,
        }),
        _ => {
            let (x, y) = (num(a)?, num(b)?);
            Bool(match op {
                "=" => (x - y).abs() == 0.0,
                "<>" => (x - y).abs() != 0.0,
                "<" => x < y,
                "<=" => x <= y,
                ">" => x > y,
                ">=" => x >= y,
                _ => return None,
            })
        }
    })
}
