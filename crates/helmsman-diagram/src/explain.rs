//! "On because …": why a variable has the value it has after a scan, as
//! data that explain-mode phrases later.
//!
//! From the variable, find the last assignment to it that ran this scan
//! (source order). The answer is that assignment, the gates around it
//! (each IF/ELSIF/CASE arm or loop it sits in, with the conditions that had
//! to hold, or fail, for it, and their values), and the same question asked
//! of every variable its expression and those conditions read, to `depth`.
//! A function block output is followed into the block's own body (user
//! blocks) or reported as the block with its inputs (library blocks: TON,
//! CTU, ...). No assignment ran: the variable held its value, and each
//! assignment says which gate kept it from running.

use std::collections::{HashMap, HashSet};

use helmsman_iec::Value;
use helmsman_iec::trace::StmtId;

use crate::live::{self, Live, Scan};
use crate::lower::{Module, Pou};
use crate::model::{Diagram, NodeId, NodeKind, Region, RegionId, RegionKind};

#[derive(Debug, Clone, PartialEq)]
pub struct Because {
    /// The variable, as the program sees it (`LAMP`, `M.ON`).
    pub path: String,
    pub value: Option<Value>,
    pub why: Why,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Why {
    /// This assignment ran last this scan.
    Assigned {
        pou: String,
        stmt: StmtId,
        line: u32,
        /// `LAMP := M.ON AND NOT STOP`, `Q => DONE`.
        text: String,
        /// The gates it sat behind, outermost first; all open.
        gates: Vec<Step>,
        /// Every variable its expression read.
        inputs: Vec<Because>,
    },
    /// No assignment to it ran this scan: it kept its value. Each
    /// assignment, with the first gate on its way that was closed.
    Held { pou: String, blocked: Vec<Blocked> },
    /// The output of a library function block; its inputs as the last call
    /// that ran passed them.
    Block {
        instance: String,
        fb: String,
        call: Option<(StmtId, u32)>,
        inputs: Vec<Because>,
        /// The instance's ports as the scan left them (`T.IN`, `T.PT`,
        /// `T.ET`, `T.Q` for a TON), for the blocks [`ports`] knows.
        ports: Vec<(String, Option<Value>)>,
    },
    /// A function block's input: what the call passed it (the last call of
    /// this instance that ran), followed in the caller.
    Argument {
        instance: String,
        call: Option<(StmtId, u32)>,
        inputs: Vec<Because>,
    },
    /// Not assigned in the POU: an input, a located input, or a constant.
    Input,
    /// Not followed further (`depth` reached, or a cycle).
    Elided,
}

/// One gate on the way to a statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub stmt: StmtId,
    pub line: u32,
    pub kind: RegionKind,
    /// `IF RUN`, `ELSE`, `CASE 1, 2`, `FOR I := 0 TO 3`.
    pub label: String,
    /// Whether it let the statements through this scan.
    pub open: bool,
    /// What it tested.
    pub tests: Vec<Test>,
}

/// One condition a gate tests.
#[derive(Debug, Clone, PartialEq)]
pub struct Test {
    /// `RUN AND NOT FAULT`; for a CASE, the selector.
    pub text: String,
    pub value: Option<Value>,
    /// For an IF arm: whether the arm needs it TRUE (its own condition) or
    /// FALSE (an earlier arm's). A CASE selector or loop condition: true.
    pub wanted: bool,
    pub inputs: Vec<Because>,
}

/// An assignment that did not run.
#[derive(Debug, Clone, PartialEq)]
pub struct Blocked {
    pub stmt: StmtId,
    pub line: u32,
    pub text: String,
    /// The outermost closed gate around it; `None` if every gate was open
    /// (an EXIT, RETURN or fault ended the scan before it).
    pub gate: Option<Step>,
}

/// Why `path` (a variable of the program: `LAMP`, `M.ON`) has its value
/// after the scan `t`, following inputs `depth` levels deep.
#[must_use]
pub fn explain(m: &Module, t: &dyn Scan, path: &str, depth: usize) -> Because {
    let path: String = path
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let mut x = Explainer {
        m,
        t,
        live: HashMap::new(),
        stack: Vec::new(),
        callers: Vec::new(),
    };
    match m
        .pous
        .iter()
        .position(|p| p.kind == helmsman_iec::ast::PouKind::Program)
    {
        Some(p) => x.var(p, "", &path, depth),
        None => Because {
            value: t.get(&path),
            path,
            why: Why::Input,
        },
    }
}

struct Explainer<'a> {
    m: &'a Module,
    t: &'a dyn Scan,
    live: HashMap<(usize, String), Live>,
    stack: Vec<String>,
    /// The calls followed into function blocks: (caller POU, caller
    /// prefix, instance as the caller names it).
    callers: Vec<(usize, String, String)>,
}

impl Explainer<'_> {
    fn live(&mut self, pou: usize, prefix: &str) -> &Live {
        self.live
            .entry((pou, prefix.to_string()))
            .or_insert_with(|| live::annotate(&self.m.pous[pou].diagram, self.t, prefix))
    }

    fn line(&self, stmt: StmtId) -> u32 {
        self.t.line(stmt).unwrap_or(0)
    }

    /// `local` in POU `pou` viewed at `prefix`.
    fn var(&mut self, pou: usize, prefix: &str, local: &str, depth: usize) -> Because {
        let full = format!("{prefix}{local}");
        let value = self.t.get(&full);
        let p: &Pou = &self.m.pous[pou];
        let d = &p.diagram;
        let writes: Vec<NodeId> = d
            .nodes
            .iter()
            .filter(|n| matches!(&n.kind, NodeKind::Write { path } if path == local))
            .map(|n| n.id)
            .collect();
        // An instance's output, not assigned here: follow it into the block.
        if writes.is_empty()
            && let Some(dot) = local.find('.')
        {
            let (inst, port) = (&local[..dot], &local[dot + 1..]);
            let fb = p.vars.get(crate::lower::root_of(inst)).and_then(|v| v.fb.clone());
            if let Some(fb) = fb {
                if let Some(inner) = self.m.pous.iter().position(|q| q.name == fb) {
                    self.callers.push((pou, prefix.to_string(), inst.to_string()));
                    let b = self.var(inner, &format!("{prefix}{inst}."), port, depth);
                    self.callers.pop();
                    return b;
                }
                return self.block(pou, prefix, inst, &fb, full, value, depth);
            }
        }
        if depth == 0 || self.stack.contains(&full) {
            return Because {
                path: full,
                value,
                why: Why::Elided,
            };
        }
        if writes.is_empty() {
            let why = match p.vars.get(local) {
                Some(v) if v.section == helmsman_iec::ast::Section::Input => self.argument(local, depth),
                _ => None,
            };
            return Because {
                path: full,
                value,
                why: why.unwrap_or(Why::Input),
            };
        }
        self.stack.push(full.clone());
        let ran = last_ran(d, self.t, &writes);
        let why = match ran {
            Some(w) => {
                let n = d.node(w);
                let (stmt, region, text) = (n.stmt, n.region, n.text.clone());
                let gates = self.gates(pou, prefix, region, depth);
                let inputs = self.upstream(pou, prefix, w, depth);
                Why::Assigned {
                    pou: p.name.clone(),
                    stmt,
                    line: self.line(stmt),
                    text,
                    gates,
                    inputs,
                }
            }
            None => {
                let blocked = writes
                    .iter()
                    .map(|&w| {
                        let n = d.node(w);
                        let closed = closed_gate(d, self.t, n.region);
                        Blocked {
                            stmt: n.stmt,
                            line: self.line(n.stmt),
                            text: n.text.clone(),
                            gate: closed.map(|r| self.step(pou, prefix, r, depth)),
                        }
                    })
                    .collect();
                Why::Held {
                    pou: p.name.clone(),
                    blocked,
                }
            }
        };
        self.stack.pop();
        Because {
            path: full,
            value,
            why,
        }
    }

    /// The function block input `local` of the instance being followed, as
    /// its caller passed it.
    fn argument(&mut self, local: &str, depth: usize) -> Option<Why> {
        let (pou, prefix, inst) = self.callers.pop()?;
        let d = &self.m.pous[pou].diagram;
        let call = d
            .nodes
            .iter()
            .rev()
            .find(|n| {
                matches!(&n.kind, NodeKind::Fb { instance, .. } if *instance == inst) && self.t.ran(n.stmt)
            })
            .map(|n| n.id);
        let mut inputs = Vec::new();
        if let Some(c) = call
            && let Some(k) = d.node(c).ins.iter().position(|i| i == local)
            && let Some(w) = d.inputs(c).find(|w| w.inp == k)
        {
            let mut paths = Vec::new();
            collect(d, w.from, w.out, &mut paths);
            for p in paths {
                inputs.push(self.var(pou, &prefix, &p, depth - 1));
            }
        }
        let call = call.map(|c| {
            let s = self.m.pous[pou].diagram.node(c).stmt;
            (s, self.line(s))
        });
        self.callers.push((pou, prefix.clone(), inst.clone()));
        Some(Why::Argument {
            instance: format!("{prefix}{inst}"),
            call,
            inputs,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        pou: usize,
        prefix: &str,
        inst: &str,
        fb: &str,
        full: String,
        value: Option<Value>,
        depth: usize,
    ) -> Because {
        let d = &self.m.pous[pou].diagram;
        let call = d
            .nodes
            .iter()
            .rev()
            .find(|n| {
                matches!(&n.kind, NodeKind::Fb { instance, .. } if instance == inst) && self.t.ran(n.stmt)
            })
            .map(|n| n.id);
        let why = if depth == 0 || self.stack.contains(&full) {
            Why::Elided
        } else {
            self.stack.push(full.clone());
            let inputs = call.map_or_else(Vec::new, |c| self.upstream(pou, prefix, c, depth));
            self.stack.pop();
            let instance = format!("{prefix}{inst}");
            let ports = ports(fb)
                .iter()
                .map(|p| {
                    let path = format!("{instance}.{p}");
                    let v = self.t.get(&path);
                    (path, v)
                })
                .collect();
            Why::Block {
                instance,
                fb: fb.to_string(),
                ports,
                call: call.map(|c| {
                    let s = d.node(c).stmt;
                    (s, self.line(s))
                }),
                inputs,
            }
        };
        Because {
            path: full,
            value,
            why,
        }
    }

    /// The variables feeding node `n`, each explained one level shallower.
    fn upstream(&mut self, pou: usize, prefix: &str, n: NodeId, depth: usize) -> Vec<Because> {
        let paths = leaves(&self.m.pous[pou].diagram, n);
        paths
            .into_iter()
            .map(|p| self.var(pou, prefix, &p, depth - 1))
            .collect()
    }

    fn gates(&mut self, pou: usize, prefix: &str, region: Option<RegionId>, depth: usize) -> Vec<Step> {
        let chain: Vec<RegionId> = self.m.pous[pou].diagram.chain(region).map(|r| r.id).collect();
        chain
            .into_iter()
            .rev()
            .map(|r| self.step(pou, prefix, r, depth))
            .collect()
    }

    fn step(&mut self, pou: usize, prefix: &str, r: RegionId, depth: usize) -> Step {
        let d: &Diagram = &self.m.pous[pou].diagram;
        let region: &Region = d.region(r);
        let open = live::gate(self.t, region).open();
        // (the region whose gate is tested, wanted)
        let mut want: Vec<(RegionId, bool)> = Vec::new();
        match region.kind {
            RegionKind::If | RegionKind::Elsif | RegionKind::Else => {
                for s in &d.regions {
                    if s.stmt == region.stmt && s.arm < region.arm && s.gate.is_some() {
                        want.push((s.id, false));
                    }
                }
                if region.gate.is_some() {
                    want.push((r, true));
                }
            }
            RegionKind::Case | RegionKind::CaseElse | RegionKind::While => {
                if region.gate.is_some() {
                    want.push((r, true));
                }
            }
            RegionKind::For | RegionKind::Repeat => {}
        }
        let (stmt, label, kind) = (region.stmt, region.label.clone(), region.kind);
        let tests = want
            .into_iter()
            .map(|(gr, wanted)| {
                let m = self.m;
                let d = &m.pous[pou].diagram;
                let value = self.live(pou, prefix).gate_value(d, gr);
                let (text, g) = gate_text(d, gr);
                let inputs = if depth == 0 {
                    Vec::new()
                } else {
                    let mut ins = Vec::new();
                    for p in leaves_of(d, g) {
                        ins.push(self.var(pou, prefix, &p, depth - 1));
                    }
                    ins
                };
                Test {
                    text,
                    value,
                    wanted,
                    inputs,
                }
            })
            .collect();
        Step {
            stmt,
            line: self.line(stmt),
            kind,
            label,
            open,
            tests,
        }
    }
}

/// What region `r`'s gate tests, as text (`NOT JAM` for a negated gate),
/// and the node it reads. `r` has a gate.
fn gate_text(d: &Diagram, r: RegionId) -> (String, NodeId) {
    let region = d.region(r);
    let (g, port) = region.gate.unwrap_or((NodeId(0), 0));
    let n = d.node(g);
    // A read wired from an earlier write or call reads that variable.
    let base = match &n.kind {
        NodeKind::Write { path } => path.clone(),
        NodeKind::Fb { instance, .. } => n
            .outs
            .get(port)
            .map_or_else(|| n.text.clone(), |o| format!("{instance}.{o}")),
        _ => n.text.clone(),
    };
    let text = if region.negated {
        // NOT binds tighter than any binary operator.
        match &n.kind {
            NodeKind::Op { .. } if n.ins.len() == 2 => format!("NOT ({base})"),
            _ => format!("NOT {base}"),
        }
    } else {
        base
    };
    (text, g)
}

/// The ports of a standard library block, inputs first; empty for any
/// other block.
#[must_use]
pub fn ports(fb: &str) -> &'static [&'static str] {
    match fb {
        "TON" | "TOF" | "TP" => &["IN", "PT", "Q", "ET"],
        "CTU" => &["CU", "R", "PV", "Q", "CV"],
        "CTD" => &["CD", "LD", "PV", "Q", "CV"],
        "CTUD" => &["CU", "CD", "R", "LD", "PV", "QU", "QD", "CV"],
        "R_TRIG" | "F_TRIG" => &["CLK", "Q"],
        "SR" => &["S1", "R", "Q1"],
        "RS" => &["S", "R1", "Q1"],
        _ => &[],
    }
}

/// The variables a node's inputs read, through operators and functions,
/// in first-seen order.
fn leaves(d: &Diagram, n: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    for w in d.inputs(n) {
        collect(d, w.from, w.out, &mut out);
    }
    out
}

/// The variables a node's value comes from: itself if it is a variable.
fn leaves_of(d: &Diagram, n: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    collect(d, n, 0, &mut out);
    out
}

fn collect(d: &Diagram, n: NodeId, port: usize, out: &mut Vec<String>) {
    let node = d.node(n);
    let path = match &node.kind {
        NodeKind::Read { path } | NodeKind::Write { path } => Some(path.clone()),
        NodeKind::For { var } => Some(var.clone()),
        NodeKind::Fb { instance, .. } => node.outs.get(port).map(|o| format!("{instance}.{o}")),
        NodeKind::Const { .. } | NodeKind::Jump => None,
        NodeKind::Op { .. } | NodeKind::Func { .. } | NodeKind::Access { .. } => {
            for w in d.inputs(n) {
                collect(d, w.from, w.out, out);
            }
            None
        }
    };
    if let Some(p) = path
        && !p.starts_with('%')
        && !out.contains(&p)
    {
        out.push(p);
    }
}

fn show(v: Option<Value>) -> String {
    v.map_or_else(|| "?".to_string(), |v| v.to_string())
}

impl Because {
    /// An indented outline, one fact per line, for a panel or a log. Not
    /// the floor-language phrasing (that is explain-mode's), just the data.
    #[must_use]
    pub fn outline(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.lines(0, &mut out);
        out
    }

    fn lines(&self, indent: usize, out: &mut Vec<String>) {
        let pad = "  ".repeat(indent);
        let head = format!("{pad}{} = {}", self.path, show(self.value));
        match &self.why {
            Why::Assigned {
                line,
                text,
                gates,
                inputs,
                ..
            } => {
                out.push(format!("{head} because line {line}: {text}"));
                for g in gates {
                    g.lines(indent + 1, out);
                }
                for i in inputs {
                    i.lines(indent + 1, out);
                }
            }
            Why::Held { blocked, .. } => {
                out.push(format!("{head}, held: no assignment ran this scan"));
                for b in blocked {
                    match &b.gate {
                        Some(g) => {
                            out.push(format!("{pad}  line {}: {} skipped:", b.line, b.text));
                            g.lines(indent + 2, out);
                        }
                        None => out.push(format!("{pad}  line {}: {} not reached", b.line, b.text)),
                    }
                }
            }
            Why::Block {
                instance,
                fb,
                call,
                inputs,
                ..
            } => {
                match call {
                    Some((_, line)) => {
                        out.push(format!("{head} from {instance} : {fb} (called line {line})"));
                    }
                    None => out.push(format!("{head} from {instance} : {fb} (not called this scan)")),
                }
                for i in inputs {
                    i.lines(indent + 1, out);
                }
            }
            Why::Argument {
                instance,
                call,
                inputs,
            } => {
                match call {
                    Some((_, line)) => {
                        out.push(format!("{head} passed in by {instance}(...) on line {line}"));
                    }
                    None => out.push(format!(
                        "{head} passed in by {instance}(...), not called this scan"
                    )),
                }
                for i in inputs {
                    i.lines(indent + 1, out);
                }
            }
            Why::Input => out.push(format!("{head} (input)")),
            Why::Elided => out.push(head),
        }
    }
}

impl Step {
    fn lines(&self, indent: usize, out: &mut Vec<String>) {
        let pad = "  ".repeat(indent);
        let state = if self.open { "open" } else { "closed" };
        out.push(format!("{pad}{} (line {}) {state}", self.label, self.line));
        for t in &self.tests {
            let need = if t.wanted { "" } else { " (needs FALSE)" };
            out.push(format!("{pad}  {} = {}{need}", t.text, show(t.value)));
            for i in &t.inputs {
                i.lines(indent + 2, out);
            }
        }
    }
}

/// The assignment among `writes` that ran last this scan (source order).
fn last_ran(d: &Diagram, t: &dyn Scan, writes: &[NodeId]) -> Option<NodeId> {
    writes.iter().rev().copied().find(|&w| t.ran(d.node(w).stmt))
}

/// The outermost closed gate around `region`, if any.
fn closed_gate(d: &Diagram, t: &dyn Scan, region: Option<RegionId>) -> Option<RegionId> {
    let chain: Vec<RegionId> = d.chain(region).map(|r| r.id).collect();
    chain
        .into_iter()
        .rev()
        .find(|&r| !live::gate(t, d.region(r)).open())
}

/// What drives a variable's value now, on one diagram: the blocks, wires
/// and regions the "because" path draws at full strength. Indexed like
/// the diagram's nodes, wires and regions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drivers {
    pub nodes: Vec<bool>,
    pub wires: Vec<bool>,
    pub regions: Vec<bool>,
}

impl Drivers {
    #[must_use]
    pub fn node(&self, n: NodeId) -> bool {
        self.nodes.get(n.0 as usize).copied().unwrap_or(false)
    }

    #[must_use]
    pub fn wire(&self, k: usize) -> bool {
        self.wires.get(k).copied().unwrap_or(false)
    }

    #[must_use]
    pub fn region(&self, r: RegionId) -> bool {
        self.regions.get(r.0 as usize).copied().unwrap_or(false)
    }

    /// Nothing on this diagram drives it (the variable lives in another
    /// scope).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self
            .nodes
            .iter()
            .chain(&self.wires)
            .chain(&self.regions)
            .any(|&b| b)
    }
}

/// The blocks, wires and regions of POU `pou`'s diagram (viewed through
/// the instance at `prefix`, `""` for the program) that drive `path` (a
/// program path, as for [`explain()`]) in scan `t`: "why it is in this
/// state now", drawn rather than said.
///
/// The same rules as [`explain()`]: the last assignment to it that ran,
/// what that assignment read (through operators, functions, block calls
/// and the earlier assignments it is wired from), the gates around it (an
/// IF arm's own condition and the earlier arms' that had to fail, a CASE
/// selector, a loop head) and, for every variable read, the same again. A
/// variable no assignment set this scan is held by the gates that closed:
/// those, their conditions and the assignments they skipped. An
/// instance's output is driven by its last call. The variable's own reads
/// are on the path too. Every node and variable is visited once, so the
/// cost is linear in the diagram: cheap enough for every frame.
#[must_use]
pub fn drivers(m: &Module, t: &dyn Scan, pou: usize, prefix: &str, path: &str) -> Drivers {
    let Some(p) = m.pous.get(pou) else {
        return Drivers::default();
    };
    let d = &p.diagram;
    let mut out = Drivers {
        nodes: vec![false; d.nodes.len()],
        wires: vec![false; d.wires.len()],
        regions: vec![false; d.regions.len()],
    };
    let path: String = path
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let Some(local) = path.strip_prefix(&prefix.to_ascii_uppercase()) else {
        return out;
    };
    let mut into: Vec<Vec<usize>> = vec![Vec::new(); d.nodes.len()];
    for (k, w) in d.wires.iter().enumerate() {
        into[w.to.0 as usize].push(k);
    }
    let mut w = Walk {
        d,
        t,
        into,
        out: &mut out,
        seen: HashSet::new(),
        vars: vec![local.to_string()],
        nodes: Vec::new(),
    };
    for n in &d.nodes {
        if matches!(&n.kind, NodeKind::Read { path } if path == local) {
            w.nodes.push(n.id);
        }
    }
    w.run(p);
    out
}

struct Walk<'a, 'o> {
    d: &'a Diagram,
    t: &'a dyn Scan,
    /// Per node, the wires into it.
    into: Vec<Vec<usize>>,
    out: &'o mut Drivers,
    seen: HashSet<String>,
    vars: Vec<String>,
    nodes: Vec<NodeId>,
}

impl Walk<'_, '_> {
    fn run(&mut self, p: &Pou) {
        loop {
            if let Some(n) = self.nodes.pop() {
                self.node(n);
            } else if let Some(v) = self.vars.pop() {
                if self.seen.insert(v.clone()) {
                    self.var(p, &v);
                }
            } else {
                return;
            }
        }
    }

    fn var(&mut self, p: &Pou, v: &str) {
        let d = self.d;
        let writes: Vec<NodeId> = d
            .nodes
            .iter()
            .filter(|n| matches!(&n.kind, NodeKind::Write { path } if path == v))
            .map(|n| n.id)
            .collect();
        if writes.is_empty() {
            // An instance's output: its last call drives it.
            if let Some(dot) = v.rfind('.') {
                let inst = &v[..dot];
                if p.vars
                    .get(crate::lower::root_of(inst))
                    .is_some_and(|x| x.fb.is_some())
                {
                    let calls: Vec<NodeId> = d
                        .nodes
                        .iter()
                        .filter(|n| matches!(&n.kind, NodeKind::Fb { instance, .. } if instance == inst))
                        .map(|n| n.id)
                        .collect();
                    if let Some(c) = last_ran(d, self.t, &calls).or_else(|| calls.last().copied()) {
                        self.nodes.push(c);
                    }
                }
            }
            return;
        }
        if let Some(w) = last_ran(d, self.t, &writes) {
            self.nodes.push(w);
            return;
        }
        // Held: the gates that kept each assignment from running.
        for w in writes {
            self.out.nodes[w.0 as usize] = true;
            if let Some(closed) = closed_gate(d, self.t, d.node(w).region) {
                self.region(closed);
            }
        }
    }

    /// Node `n`, everything feeding it, and the gates it sits behind.
    fn node(&mut self, n: NodeId) {
        let k = n.0 as usize;
        if self.out.nodes[k] {
            return;
        }
        self.out.nodes[k] = true;
        for &w in &self.into[k] {
            self.out.wires[w] = true;
            self.nodes.push(self.d.wires[w].from);
        }
        let node = self.d.node(n);
        if let NodeKind::Read { path } = &node.kind {
            self.vars.push(path.clone());
        }
        if let Some(r) = node.region {
            self.region(r);
        }
    }

    /// Region `r`, the regions around it, and what each gate tests: its
    /// condition (an IF arm's, with the earlier arms' that had to fail),
    /// selector or loop head.
    fn region(&mut self, r: RegionId) {
        let mut at = Some(r);
        while let Some(r) = at {
            if self.out.regions[r.0 as usize] {
                return;
            }
            self.out.regions[r.0 as usize] = true;
            let region = self.d.region(r);
            if matches!(region.kind, RegionKind::Elsif | RegionKind::Else) {
                for s in &self.d.regions {
                    if s.stmt == region.stmt
                        && s.arm < region.arm
                        && let Some((g, _)) = s.gate
                    {
                        self.nodes.push(g);
                    }
                }
            }
            if let Some((g, _)) = region.gate {
                self.nodes.push(g);
            }
            at = region.parent;
        }
    }
}
