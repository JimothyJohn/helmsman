//! Scan-cycle simulation of a project: each scan evaluates every node once in
//! wire order (sources first). A wire that closes a loop reads the value its
//! source had at the end of the previous scan, as a PLC's would. Time only
//! moves when the caller says so, so scans are deterministic.

#![allow(clippy::many_single_char_names)]

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{CmpOp, Graph, Kind, MathOp, NodeId, Project, Ty, Value};

/// Instances deeper than this evaluate to zeros (a guard against a project
/// that nests a block inside itself; edits refuse that already).
const MAX_DEPTH: usize = 64;

/// Per-node memory: timers, counters, latches, edges.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mem {
    /// Elapsed ms for timers and the clock; count for counters.
    pub acc: f64,
    pub q: bool,
    /// Previous scan's input, for edge detection.
    pub prev: bool,
}

/// State of one graph instance: its nodes' outputs and memory, and the state
/// of each block instance inside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inst {
    pub outs: BTreeMap<(NodeId, u32), Value>,
    pub mem: BTreeMap<NodeId, Mem>,
    pub children: BTreeMap<NodeId, Inst>,
}

impl Inst {
    /// The instance reached by following block nodes from here.
    #[must_use]
    pub fn at(&self, path: &[NodeId]) -> Option<&Self> {
        path.iter().try_fold(self, |inst, id| inst.children.get(id))
    }

    /// Value on an output port (zero of `ty` before the first scan).
    #[must_use]
    pub fn out(&self, node: NodeId, port: u32, ty: Ty) -> Value {
        self.outs.get(&(node, port)).copied().unwrap_or(Value::zero(ty))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sim {
    pub root: Inst,
    /// Simulated ms since the last reset.
    pub time_ms: f64,
    pub scans: u64,
    /// Buttons held down, each as the instance path to it plus its own id.
    pub held: BTreeSet<Vec<NodeId>>,
}

impl Sim {
    /// Runs one scan, `dt_ms` after the previous one.
    pub fn scan(&mut self, project: &Project, dt_ms: f64) {
        let dt = dt_ms.max(0.0);
        self.time_ms += dt;
        self.scans += 1;
        let mut path = Vec::new();
        let mut cx = Cx {
            project,
            held: &self.held,
            dt,
            path: &mut path,
        };
        eval(&mut cx, &project.root, &mut self.root, &BTreeMap::new());
    }

    /// Forgets all state: timers, counters, latches, outputs.
    pub fn reset(&mut self) {
        let held = std::mem::take(&mut self.held);
        *self = Self {
            held,
            ..Self::default()
        };
    }
}

struct Cx<'a> {
    project: &'a Project,
    held: &'a BTreeSet<Vec<NodeId>>,
    dt: f64,
    path: &'a mut Vec<NodeId>,
}

/// Nodes in evaluation order: Kahn's algorithm over the wires, ties by id;
/// nodes on a loop follow in id order.
fn order(g: &Graph) -> Vec<NodeId> {
    let mut indeg: BTreeMap<NodeId, usize> = g.nodes.keys().map(|id| (*id, 0)).collect();
    let mut next: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    for w in &g.wires {
        if g.nodes.contains_key(&w.from)
            && w.from != w.to
            && let Some(d) = indeg.get_mut(&w.to)
        {
            *d += 1;
            next.entry(w.from).or_default().push(w.to);
        }
    }
    let mut ready: BTreeSet<NodeId> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut out = Vec::with_capacity(g.nodes.len());
    while let Some(id) = ready.pop_first() {
        out.push(id);
        for to in next.get(&id).into_iter().flatten() {
            let d = indeg.get_mut(to).expect("counted above");
            *d -= 1;
            if *d == 0 {
                ready.insert(*to);
            }
        }
    }
    if out.len() < g.nodes.len() {
        let done: BTreeSet<NodeId> = out.iter().copied().collect();
        out.extend(g.nodes.keys().filter(|id| !done.contains(id)));
    }
    out
}

/// Evaluates one graph instance; `pins` are its input pins' values. Returns
/// the output pins' values by pin id.
#[allow(clippy::too_many_lines)]
fn eval(cx: &mut Cx, g: &Graph, inst: &mut Inst, pins: &BTreeMap<NodeId, Value>) -> BTreeMap<NodeId, Value> {
    let mut result = BTreeMap::new();
    for id in order(g) {
        let node = &g.nodes[&id];
        let (ins, outs) = cx.project.ports(&node.kind);
        let input = |key: u32| -> Value {
            let ty = ins.iter().find(|p| p.key == key).map_or(Ty::Bool, |p| p.ty);
            g.wire_into(id, key)
                .map_or(Value::zero(ty), |w| inst.out(w.from, w.from_port, ty))
        };
        let b = |key: u32| input(key).as_bool();
        let n = |key: u32| input(key).as_num();
        let mut mem = inst.mem.get(&id).copied().unwrap_or_default();
        let dt = cx.dt;
        let set: Vec<(u32, Value)> = match &node.kind {
            Kind::Toggle { on } => vec![(0, Value::Bool(*on))],
            Kind::Button => {
                cx.path.push(id);
                let held = cx.held.contains(cx.path.as_slice());
                cx.path.pop();
                vec![(0, Value::Bool(held))]
            }
            Kind::Const { value } => vec![(0, Value::Num(*value))],
            Kind::Clock { period_ms } => {
                mem.acc += dt;
                let q = *period_ms > 0.0 && mem.acc.rem_euclid(*period_ms) < period_ms / 2.0;
                vec![(0, Value::Bool(q))]
            }
            Kind::Lamp | Kind::Display | Kind::Note { .. } => vec![],
            Kind::And { .. } => vec![(0, Value::Bool(ins.iter().all(|p| b(p.key))))],
            Kind::Or { .. } => vec![(0, Value::Bool(ins.iter().any(|p| b(p.key))))],
            Kind::Xor => vec![(0, Value::Bool(b(0) ^ b(1)))],
            Kind::Not => vec![(0, Value::Bool(!b(0)))],
            Kind::Sr => {
                mem.q = b(0) || (mem.q && !b(1));
                vec![(0, Value::Bool(mem.q))]
            }
            Kind::Rs => {
                mem.q = !b(1) && (b(0) || mem.q);
                vec![(0, Value::Bool(mem.q))]
            }
            Kind::RTrig | Kind::FTrig => {
                let clk = b(0);
                let q = if matches!(node.kind, Kind::RTrig) {
                    clk && !mem.prev
                } else {
                    !clk && mem.prev
                };
                mem.prev = clk;
                vec![(0, Value::Bool(q))]
            }
            Kind::Ton { preset_ms } => {
                if b(0) {
                    mem.acc = (mem.acc + dt).min(*preset_ms);
                    mem.q = mem.acc >= *preset_ms;
                } else {
                    mem.acc = 0.0;
                    mem.q = false;
                }
                vec![(0, Value::Bool(mem.q)), (1, Value::Num(mem.acc))]
            }
            Kind::Tof { preset_ms } => {
                if b(0) {
                    mem.q = true;
                    mem.acc = 0.0;
                } else if mem.q {
                    mem.acc = (mem.acc + dt).min(*preset_ms);
                    mem.q = mem.acc < *preset_ms;
                }
                vec![(0, Value::Bool(mem.q)), (1, Value::Num(mem.acc))]
            }
            Kind::Tp { preset_ms } => {
                let inp = b(0);
                if mem.q {
                    mem.acc = (mem.acc + dt).min(*preset_ms);
                    mem.q = mem.acc < *preset_ms;
                } else if inp && !mem.prev {
                    mem.q = true;
                    mem.acc = 0.0;
                } else if !inp {
                    mem.acc = 0.0;
                }
                mem.prev = inp;
                vec![(0, Value::Bool(mem.q)), (1, Value::Num(mem.acc))]
            }
            Kind::Tonr { preset_ms } => {
                if b(1) {
                    mem.acc = 0.0;
                } else if b(0) {
                    mem.acc = (mem.acc + dt).min(*preset_ms);
                }
                mem.q = !b(1) && mem.acc >= *preset_ms;
                vec![(0, Value::Bool(mem.q)), (1, Value::Num(mem.acc))]
            }
            Kind::Ctu { preset } => {
                let cu = b(0);
                if b(1) {
                    mem.acc = 0.0;
                } else if cu && !mem.prev {
                    mem.acc += 1.0;
                }
                mem.prev = cu;
                mem.q = mem.acc >= *preset;
                vec![(0, Value::Bool(mem.q)), (1, Value::Num(mem.acc))]
            }
            Kind::Math(op) => {
                let (a, c) = (n(0), n(1));
                let v = match op {
                    MathOp::Add => a + c,
                    MathOp::Sub => a - c,
                    MathOp::Mul => a * c,
                    MathOp::Div if c == 0.0 => 0.0,
                    MathOp::Div => a / c,
                    MathOp::Min => a.min(c),
                    MathOp::Max => a.max(c),
                };
                vec![(0, Value::Num(if v.is_finite() { v } else { 0.0 }))]
            }
            Kind::Compare(op) => {
                let (a, c) = (n(0), n(1));
                // Exact, as IEC EQ/NE on REAL are.
                #[allow(clippy::float_cmp)]
                let q = match op {
                    CmpOp::Gt => a > c,
                    CmpOp::Ge => a >= c,
                    CmpOp::Lt => a < c,
                    CmpOp::Le => a <= c,
                    CmpOp::Eq => a == c,
                    CmpOp::Ne => a != c,
                };
                vec![(0, Value::Bool(q))]
            }
            Kind::Select => vec![(0, Value::Num(if b(0) { n(2) } else { n(1) }))],
            Kind::PinIn { ty, .. } => vec![(0, pins.get(&id).copied().unwrap_or(Value::zero(*ty)))],
            Kind::PinOut { ty, .. } => {
                let v = g
                    .wire_into(id, 0)
                    .map_or(Value::zero(*ty), |w| inst.out(w.from, w.from_port, *ty));
                result.insert(id, v);
                vec![]
            }
            Kind::Block(block) => {
                let child_pins: BTreeMap<NodeId, Value> =
                    ins.iter().map(|p| (NodeId(p.key), input(p.key))).collect();
                let mut child = inst.children.remove(&id).unwrap_or_default();
                let got = match cx.project.blocks.get(block) {
                    Some(def) if cx.path.len() < MAX_DEPTH => {
                        cx.path.push(id);
                        let got = eval(cx, &def.graph, &mut child, &child_pins);
                        cx.path.pop();
                        got
                    }
                    _ => BTreeMap::new(),
                };
                inst.children.insert(id, child);
                outs.iter()
                    .map(|p| {
                        (
                            p.key,
                            got.get(&NodeId(p.key)).copied().unwrap_or(Value::zero(p.ty)),
                        )
                    })
                    .collect()
            }
        };
        inst.mem.insert(id, mem);
        for (key, v) in set {
            inst.outs.insert((id, key), v);
        }
    }
    // Forget state of nodes that were deleted since the last scan.
    inst.mem.retain(|id, _| g.nodes.contains_key(id));
    inst.outs.retain(|(id, _), _| g.nodes.contains_key(id));
    inst.children
        .retain(|id, _| matches!(g.nodes.get(id).map(|n| &n.kind), Some(Kind::Block(_))));
    result
}
