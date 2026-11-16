//! A set of blocks shown as one.
//!
//! A [`Fold`] names blocks of a [`Diagram`] that the reader would rather
//! see as one thing: a comparison against a constant, a latch, a whole IF
//! arm, a run of statements under one comment. [`view`] gives the diagram
//! with every fold applied: each folded set is one [`NodeKind::Fold`]
//! block whose pins are exactly the wires that crossed its edge, laid out
//! like any other diagram. Nothing below changes: the lowering, the trace
//! mapping and the edit spans are those of the diagram the view was made
//! from, and a [`View`] maps its blocks, ports and regions back to it
//! ([`View::origin`], [`View::live`]).
//!
//! Two shapes of fold:
//!
//! - **Within a statement** ([`Fold::over_nodes`]): blocks of one
//!   statement in one region, convex (no wire leaves the set and comes
//!   back in, which would make the folded block feed itself).
//! - **Whole statements** ([`Fold::over_stmts`]): consecutive statements
//!   of one block (the top of the POU, or one arm), with everything nested
//!   in them. The variables they read are its named inputs, the variables
//!   they write its outputs.
//!
//! A fold that is not one of these, or that shares a block with a fold
//! before it in the list, is left out ([`View::skipped`] says which):
//! the view is then the diagram as it was at that place, never a wrong one.
//! With no folds the view is the diagram itself, block for block.

pub mod chips;
pub mod structure;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

use helmsman_iec::Value;
use helmsman_iec::trace::StmtId;

use crate::explain::Drivers;
use crate::layout;
use crate::live::{Gate, Live};
use crate::model::{Diagram, Node, NodeId, NodeKind, Rect, Region, RegionId, Stmt, Wire};

/// What a fold stands for: how the canvas draws and names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FoldKind {
    /// A comparison against a constant: `level > 80.0`.
    Chip,
    /// One variable between two bounds: `lo <= x <= hi`.
    Range,
    /// A seal-in: set, reset, the output held between.
    Latch,
    /// A rising or falling edge of one signal.
    Edge,
    /// One of two values by a condition.
    Select,
    /// An IF that only sets or clears.
    SetReset,
    /// The statements of one IF, CASE or loop arm.
    Arm,
    /// A run of statements under the author's comment.
    Section,
    /// Everything that drives one output.
    Output,
    /// A CASE on a state variable.
    State,
    /// An OR of ANDs as rows of conditions.
    Table,
}

/// A set of blocks to show as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    /// Its name across versions of the source, as far as the rule that
    /// made it can give one: what the canvas keeps "folded" or "open" by.
    pub key: String,
    pub kind: FoldKind,
    /// What the block shows in its header.
    pub title: String,
    /// The blocks it stands for.
    pub nodes: BTreeSet<NodeId>,
    /// The whole statements it covers, indices into [`Diagram::stmts`]:
    /// consecutive statements of one block. Empty for a fold within a
    /// statement.
    pub stmts: Vec<usize>,
    /// Names for its input pins by the output that feeds them (`set`,
    /// `reset`); any other pin is named after its source.
    pub names: BTreeMap<(NodeId, usize), String>,
}

impl Fold {
    /// A fold of blocks within one statement.
    #[must_use]
    pub fn over_nodes(
        key: impl Into<String>,
        kind: FoldKind,
        title: impl Into<String>,
        nodes: impl IntoIterator<Item = NodeId>,
    ) -> Self {
        Self {
            key: key.into(),
            kind,
            title: title.into(),
            nodes: nodes.into_iter().collect(),
            stmts: Vec::new(),
            names: BTreeMap::new(),
        }
    }

    /// A fold of the statements `stmts` of `d` (indices into
    /// [`Diagram::stmts`]) and everything nested in them.
    #[must_use]
    pub fn over_stmts(
        d: &Diagram,
        key: impl Into<String>,
        kind: FoldKind,
        title: impl Into<String>,
        stmts: &[usize],
    ) -> Self {
        let (inside, _) = nested(d, stmts);
        let ids: HashSet<StmtId> = inside
            .iter()
            .filter_map(|&s| d.stmts.get(s))
            .map(|s| s.id)
            .collect();
        Self {
            key: key.into(),
            kind,
            title: title.into(),
            nodes: d
                .nodes
                .iter()
                .filter(|n| ids.contains(&n.stmt))
                .map(|n| n.id)
                .collect(),
            stmts: stmts.to_vec(),
            names: BTreeMap::new(),
        }
    }
}

/// The statements `stmts` and every statement nested in their arms, and
/// the regions of those arms.
fn nested(d: &Diagram, stmts: &[usize]) -> (BTreeSet<usize>, BTreeSet<RegionId>) {
    let (mut inside, mut regions) = (BTreeSet::new(), BTreeSet::new());
    let mut todo: Vec<usize> = stmts.to_vec();
    while let Some(s) = todo.pop() {
        let Some(st) = d.stmts.get(s) else { continue };
        if !inside.insert(s) {
            continue;
        }
        for &r in &st.arms {
            if let Some(region) = d.regions.get(r.0 as usize) {
                regions.insert(r);
                todo.extend(region.body.iter().copied());
            }
        }
    }
    (inside, regions)
}

/// The folds a diagram opens with: the ones that are plain convention
/// and hide nothing a reader would look for (see [`chips`]).
#[must_use]
pub fn auto(d: &Diagram) -> Vec<Fold> {
    chips::chips(d)
}

/// Why a fold was left out of a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    /// No blocks, or a block or statement the diagram does not have.
    Empty,
    /// It shares a block with a fold before it.
    Overlaps,
    /// Its blocks are not all of one statement in one region.
    Spread,
    /// A wire leaves it and comes back in.
    NotConvex,
    /// Its statements are not consecutive statements of one block, or
    /// its blocks are not exactly theirs.
    NotARun,
}

/// What a block of a view stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The block of the diagram it was made from.
    Node(NodeId),
    /// A fold: an index into [`View::folds`].
    Fold(usize),
}

/// An input pin of a folded block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A wire from outside: the output feeding it, and whether it enters
    /// through a negation bubble.
    Wire { from: (NodeId, usize), negated: bool },
    /// A variable the folded statements read.
    Read(String),
}

/// A diagram with folds applied; see the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// The diagram to draw, laid out.
    pub diagram: Diagram,
    /// The folds applied, as [`NodeKind::Fold::fold`] indexes them.
    pub folds: Vec<Fold>,
    /// The folds left out, by their place in the list given, and why.
    pub skipped: Vec<(usize, Skipped)>,
    /// Per block of [`View::diagram`].
    pub origin: Vec<Origin>,
    /// Per region of [`View::diagram`]: the region it was.
    pub region_origin: Vec<RegionId>,
    /// Per applied fold, per input pin.
    pub ins: Vec<Vec<Input>>,
    /// Per applied fold, per output pin: the output it carries.
    pub outs: Vec<Vec<(NodeId, usize)>>,
    /// Per wire of [`View::diagram`]: the wires of the diagram it shows
    /// (several when two blocks of one fold read the same output).
    pub wire_origin: Vec<Vec<usize>>,
    /// Per block of the diagram the view was made from: the block showing
    /// it.
    shown: Vec<NodeId>,
    /// Per region of that diagram: the region showing it, if any.
    region_shown: Vec<Option<RegionId>>,
}

impl View {
    /// The block of the view showing block `n` of the diagram: itself, or
    /// the fold it is in.
    #[must_use]
    pub fn shown(&self, n: NodeId) -> Option<NodeId> {
        self.shown.get(n.0 as usize).copied()
    }

    /// The region of the view that is region `r` of the diagram; `None`
    /// when a fold hides it.
    #[must_use]
    pub fn region_shown(&self, r: RegionId) -> Option<RegionId> {
        self.region_shown.get(r.0 as usize).copied().flatten()
    }

    /// The block of the diagram behind block `n` of the view; `None` for a
    /// fold.
    #[must_use]
    pub fn original(&self, n: NodeId) -> Option<NodeId> {
        match self.origin.get(n.0 as usize)? {
            Origin::Node(o) => Some(*o),
            Origin::Fold(_) => None,
        }
    }

    /// The fold block `n` of the view is, if it is one.
    #[must_use]
    pub fn fold_at(&self, n: NodeId) -> Option<&Fold> {
        match self.origin.get(n.0 as usize)? {
            Origin::Fold(f) => self.folds.get(*f),
            Origin::Node(_) => None,
        }
    }

    /// Output `out` of block `n` of the diagram, as a port of the view.
    /// `None` when a fold hides it (nothing outside reads it).
    #[must_use]
    pub fn port(&self, n: NodeId, out: usize) -> Option<(NodeId, usize)> {
        let shown = self.shown(n)?;
        match self.origin[shown.0 as usize] {
            Origin::Node(_) => Some((shown, out)),
            Origin::Fold(f) => self.outs[f]
                .iter()
                .position(|&p| p == (n, out))
                .map(|k| (shown, k)),
        }
    }

    /// A highlighted path of the diagram the view was made from, on the
    /// view: a folded block is on the path if any of its blocks is, a
    /// wire if any wire it shows is.
    #[must_use]
    pub fn drivers(&self, on: &Drivers) -> Drivers {
        Drivers {
            nodes: self
                .origin
                .iter()
                .map(|o| match *o {
                    Origin::Node(n) => on.node(n),
                    Origin::Fold(f) => self.folds[f].nodes.iter().any(|&n| on.node(n)),
                })
                .collect(),
            wires: self
                .wire_origin
                .iter()
                .map(|from| from.iter().any(|&k| on.wire(k)))
                .collect(),
            regions: self.region_origin.iter().map(|&r| on.region(r)).collect(),
        }
    }

    /// `live` (of the diagram the view was made from) on the view: a
    /// folded block's outputs carry the values of the outputs they stand
    /// for, and it ran if any of its blocks did.
    #[must_use]
    pub fn live(&self, live: &Live) -> Live {
        let value = |n: NodeId, out: usize| -> Option<Value> {
            live.values.get(n.0 as usize)?.get(out).copied().flatten()
        };
        let ran = |n: NodeId| live.ran.get(n.0 as usize).copied().unwrap_or(false);
        let mut values = Vec::with_capacity(self.origin.len());
        let mut runs = Vec::with_capacity(self.origin.len());
        for o in &self.origin {
            match *o {
                Origin::Node(n) => {
                    values.push(live.values.get(n.0 as usize).cloned().unwrap_or_default());
                    runs.push(ran(n));
                }
                Origin::Fold(f) => {
                    values.push(self.outs[f].iter().map(|&(n, out)| value(n, out)).collect());
                    runs.push(self.folds[f].nodes.iter().any(|&n| ran(n)));
                }
            }
        }
        Live {
            values,
            ran: runs,
            gates: self
                .region_origin
                .iter()
                .map(|r| live.gates.get(r.0 as usize).copied().unwrap_or(Gate::NotReached))
                .collect(),
        }
    }
}

/// What a wire from output `out` of `n` is called on a folded block's pin.
fn source_name(n: &Node, out: usize) -> String {
    match &n.kind {
        NodeKind::Read { path } | NodeKind::Write { path } | NodeKind::Access { path } => path.clone(),
        NodeKind::Fb { instance, .. } => n
            .outs
            .get(out)
            .map_or_else(|| instance.clone(), |o| format!("{instance}.{o}")),
        NodeKind::For { var } => var.clone(),
        _ => n.text.clone(),
    }
}

/// Whether `f` can be applied to `d`, given the blocks `claimed` by folds
/// before it.
fn check(
    d: &Diagram,
    f: &Fold,
    claimed: &[Option<usize>],
    stmt_at: &HashMap<StmtId, usize>,
) -> Result<(), Skipped> {
    if f.nodes.is_empty() || f.nodes.iter().any(|n| n.0 as usize >= d.nodes.len()) {
        return Err(Skipped::Empty);
    }
    if f.nodes.iter().any(|n| claimed[n.0 as usize].is_some()) {
        return Err(Skipped::Overlaps);
    }
    if f.stmts.is_empty() {
        let first = d.node(*f.nodes.first().ok_or(Skipped::Empty)?);
        if f.nodes.iter().any(|&n| {
            let n = d.node(n);
            (n.stmt, n.region) != (first.stmt, first.region)
        }) {
            return Err(Skipped::Spread);
        }
        // Convex: nothing reached from the set, outside it, feeds it.
        let mut out: HashSet<NodeId> = HashSet::new();
        let mut todo: Vec<NodeId> = f.nodes.iter().copied().collect();
        while let Some(n) = todo.pop() {
            for w in d.wires.iter().filter(|w| w.from == n) {
                if f.nodes.contains(&w.to) {
                    if out.contains(&n) {
                        return Err(Skipped::NotConvex);
                    }
                } else if out.insert(w.to) {
                    todo.push(w.to);
                }
            }
        }
        // A block of the set that gates a region the set's statement
        // holds is read outside it: fine, it becomes an output.
        return Ok(());
    }
    if f.stmts.iter().any(|&s| s >= d.stmts.len()) {
        return Err(Skipped::Empty);
    }
    // Consecutive statements of one block.
    let region = d.stmts[f.stmts[0]].region;
    let block: &[usize] = region.map_or(&d.top, |r| &d.region(r).body);
    let Some(at) = block.iter().position(|&s| s == f.stmts[0]) else {
        return Err(Skipped::NotARun);
    };
    if block.get(at..at + f.stmts.len()) != Some(&f.stmts[..]) {
        return Err(Skipped::NotARun);
    }
    let (inside, _) = nested(d, &f.stmts);
    let theirs = |n: &Node| stmt_at.get(&n.stmt).is_some_and(|s| inside.contains(s));
    if d.nodes.iter().any(|n| theirs(n) != f.nodes.contains(&n.id)) {
        return Err(Skipped::NotARun);
    }
    Ok(())
}

/// `d` with `folds` applied, laid out. See the module docs.
#[must_use]
pub fn view(d: &Diagram, folds: &[Fold]) -> View {
    let stmt_at: HashMap<StmtId, usize> = d.stmts.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    // Which folds apply, and the fold each block is in.
    let mut claimed: Vec<Option<usize>> = vec![None; d.nodes.len()];
    let mut applied: Vec<Fold> = Vec::new();
    let mut skipped = Vec::new();
    for (i, f) in folds.iter().enumerate() {
        match check(d, f, &claimed, &stmt_at) {
            Ok(()) => {
                for n in &f.nodes {
                    claimed[n.0 as usize] = Some(applied.len());
                }
                applied.push(f.clone());
            }
            Err(why) => skipped.push((i, why)),
        }
    }
    // What each whole-statement fold hides.
    let mut gone_stmts: Vec<Option<usize>> = vec![None; d.stmts.len()];
    let mut gone_regions: Vec<bool> = vec![false; d.regions.len()];
    for (k, f) in applied.iter().enumerate() {
        let (inside, regions) = nested(d, &f.stmts);
        for s in inside {
            gone_stmts[s] = Some(k);
        }
        for r in regions {
            gone_regions[r.0 as usize] = true;
        }
    }

    // The pins of each fold: the wires that cross its edge, in wire
    // order, then (for whole statements) the variables read and written.
    let mut ins: Vec<Vec<Input>> = vec![Vec::new(); applied.len()];
    let mut outs: Vec<Vec<(NodeId, usize)>> = vec![Vec::new(); applied.len()];
    let fold_of = |n: NodeId| claimed[n.0 as usize];
    for w in &d.wires {
        let (a, b) = (fold_of(w.from), fold_of(w.to));
        if a == b && a.is_some() {
            continue;
        }
        if let Some(f) = b {
            let pin = Input::Wire {
                from: (w.from, w.out),
                negated: w.negated,
            };
            if !ins[f].contains(&pin) {
                ins[f].push(pin);
            }
        }
        if let Some(f) = a
            && !outs[f].contains(&(w.from, w.out))
        {
            outs[f].push((w.from, w.out));
        }
    }
    for r in &d.regions {
        if gone_regions[r.id.0 as usize] {
            continue;
        }
        if let Some((g, port)) = r.gate
            && let Some(f) = fold_of(g)
            && !outs[f].contains(&(g, port))
        {
            outs[f].push((g, port));
        }
    }
    for (k, f) in applied.iter().enumerate() {
        if f.stmts.is_empty() {
            continue;
        }
        let mut last_write: Vec<(String, NodeId)> = Vec::new();
        for &n in &f.nodes {
            match &d.node(n).kind {
                NodeKind::Read { path } => {
                    let pin = Input::Read(path.clone());
                    if !ins[k].contains(&pin) {
                        ins[k].push(pin);
                    }
                }
                NodeKind::Write { path } => {
                    if let Some(slot) = last_write.iter_mut().find(|(p, _)| p == path) {
                        slot.1 = n;
                    } else {
                        last_write.push((path.clone(), n));
                    }
                }
                _ => {}
            }
        }
        for (path, n) in last_write {
            // One pin per variable: the last write, unless an earlier
            // write of it is already a pin (something outside reads it).
            let named = outs[k]
                .iter()
                .any(|&(o, _)| matches!(&d.node(o).kind, NodeKind::Write { path: p } if *p == path));
            if !named {
                outs[k].push((n, 0));
            }
        }
    }

    // The view's blocks in an order where every producer comes first
    // (what layout and live values rely on): the diagram's own order,
    // a fold standing where its first block was, moved back only as far
    // as what feeds it requires.
    let units = d.nodes.len() + applied.len();
    let unit = |n: NodeId| fold_of(n).map_or(n.0 as usize, |f| d.nodes.len() + f);
    let mut first: Vec<usize> = (0..units).collect();
    for (k, f) in applied.iter().enumerate() {
        first[d.nodes.len() + k] = f.nodes.first().map_or(0, |n| n.0 as usize);
    }
    let mut needs: Vec<HashSet<usize>> = vec![HashSet::new(); units];
    let mut feeds: Vec<Vec<usize>> = vec![Vec::new(); units];
    for w in &d.wires {
        let (a, b) = (unit(w.from), unit(w.to));
        if a != b && needs[b].insert(a) {
            feeds[a].push(b);
        }
    }
    let live_unit = |u: usize| u >= d.nodes.len() || claimed[u].is_none();
    let mut ready: BinaryHeap<Reverse<(usize, usize)>> = (0..units)
        .filter(|&u| live_unit(u) && needs[u].is_empty())
        .map(|u| Reverse((first[u], u)))
        .collect();
    let mut waiting: Vec<usize> = needs.iter().map(HashSet::len).collect();
    let mut order: Vec<usize> = Vec::with_capacity(units);
    while let Some(Reverse((_, u))) = ready.pop() {
        order.push(u);
        for &v in &feeds[u] {
            waiting[v] -= 1;
            if waiting[v] == 0 {
                ready.push(Reverse((first[v], v)));
            }
        }
    }

    // Regions and statements that stay, renumbered in order.
    let mut region_shown: Vec<Option<RegionId>> = vec![None; d.regions.len()];
    let mut region_origin = Vec::new();
    for r in &d.regions {
        if !gone_regions[r.id.0 as usize] {
            #[allow(clippy::cast_possible_truncation)]
            let id = RegionId(region_origin.len() as u32);
            region_shown[r.id.0 as usize] = Some(id);
            region_origin.push(r.id);
        }
    }
    let region_to = |r: Option<RegionId>| r.and_then(|r| region_shown[r.0 as usize]);
    // A statement stays unless a fold hides it; the first statement of a
    // fold stays as the fold's own.
    let keeps = |s: usize| match gone_stmts[s] {
        None => true,
        Some(k) => applied[k].stmts.first() == Some(&s),
    };
    let mut next = 0;
    let stmt_shown: Vec<Option<usize>> = (0..d.stmts.len())
        .map(|s| {
            keeps(s).then(|| {
                next += 1;
                next - 1
            })
        })
        .collect();

    // Blocks.
    let mut shown: Vec<NodeId> = vec![NodeId(0); d.nodes.len()];
    let mut origin = Vec::with_capacity(order.len());
    let mut nodes: Vec<Node> = Vec::with_capacity(order.len());
    for &u in &order {
        #[allow(clippy::cast_possible_truncation)]
        let id = NodeId(nodes.len() as u32);
        if u < d.nodes.len() {
            let n = &d.nodes[u];
            shown[u] = id;
            origin.push(Origin::Node(n.id));
            nodes.push(Node {
                id,
                region: region_to(n.region),
                rect: Rect::default(),
                ..n.clone()
            });
            continue;
        }
        let k = u - d.nodes.len();
        let f = &applied[k];
        for n in &f.nodes {
            shown[n.0 as usize] = id;
        }
        // Where it sits: its first statement's place, or its blocks'.
        let anchor = d.node(*f.nodes.first().unwrap_or(&NodeId(0)));
        let (stmt, pos, region) = f
            .stmts
            .first()
            .map_or((anchor.stmt, anchor.pos, anchor.region), |&s| {
                let st = &d.stmts[s];
                (st.id, st.pos, st.region)
            });
        origin.push(Origin::Fold(k));
        #[allow(clippy::cast_possible_truncation)]
        nodes.push(Node {
            id,
            kind: NodeKind::Fold {
                fold: k as u32,
                kind: f.kind,
            },
            label: f.title.clone(),
            text: f.title.clone(),
            ins: ins[k]
                .iter()
                .map(|i| match i {
                    Input::Wire { from, .. } => f
                        .names
                        .get(from)
                        .cloned()
                        .unwrap_or_else(|| source_name(d.node(from.0), from.1)),
                    Input::Read(path) => path.clone(),
                })
                .collect(),
            outs: outs[k]
                .iter()
                .map(|&(n, out)| match &d.node(n).kind {
                    NodeKind::Write { path } => path.clone(),
                    NodeKind::Fb { .. } => source_name(d.node(n), out),
                    _ => String::new(),
                })
                .collect(),
            stmt,
            pos,
            region: region_to(region),
            rect: Rect::default(),
        });
    }

    // Ports and wires.
    let port = |n: NodeId, out: usize| -> Option<(NodeId, usize)> {
        let to = shown[n.0 as usize];
        match fold_of(n) {
            None => Some((to, out)),
            Some(f) => outs[f].iter().position(|&p| p == (n, out)).map(|k| (to, k)),
        }
    };
    let mut wires: Vec<Wire> = Vec::new();
    let mut wire_origin: Vec<Vec<usize>> = Vec::new();
    let mut seen: HashMap<(NodeId, usize, NodeId, usize), usize> = HashMap::new();
    for (index, w) in d.wires.iter().enumerate() {
        let (a, b) = (fold_of(w.from), fold_of(w.to));
        if a == b && a.is_some() {
            continue;
        }
        let Some((from, out)) = port(w.from, w.out) else {
            continue;
        };
        let (to, inp) = match b {
            None => (shown[w.to.0 as usize], w.inp),
            Some(f) => {
                let pin = Input::Wire {
                    from: (w.from, w.out),
                    negated: w.negated,
                };
                let Some(k) = ins[f].iter().position(|i| *i == pin) else {
                    continue;
                };
                (shown[w.to.0 as usize], k)
            }
        };
        match seen.get(&(from, out, to, inp)) {
            Some(&k) => wire_origin[k].push(index),
            None => {
                seen.insert((from, out, to, inp), wires.len());
                wire_origin.push(vec![index]);
                wires.push(Wire {
                    from,
                    out,
                    to,
                    inp,
                    negated: w.negated,
                });
            }
        }
    }

    // Statements and regions.
    let map_stmts = |body: &[usize]| -> Vec<usize> { body.iter().filter_map(|&s| stmt_shown[s]).collect() };
    let mut stmts: Vec<Stmt> = Vec::new();
    for (s, st) in d.stmts.iter().enumerate() {
        if !keeps(s) {
            continue;
        }
        let folded = gone_stmts[s].is_some();
        let mut roots: Vec<NodeId> = Vec::new();
        let from: Vec<NodeId> = match gone_stmts[s] {
            Some(k) => applied[k].nodes.first().copied().into_iter().collect(),
            None => st.roots.clone(),
        };
        for r in from {
            let to = shown[r.0 as usize];
            if !roots.contains(&to) {
                roots.push(to);
            }
        }
        stmts.push(Stmt {
            id: st.id,
            pos: st.pos,
            region: region_to(st.region),
            roots,
            arms: if folded {
                Vec::new()
            } else {
                st.arms
                    .iter()
                    .filter_map(|&r| region_shown[r.0 as usize])
                    .collect()
            },
        });
    }
    let regions: Vec<Region> = region_origin
        .iter()
        .enumerate()
        .map(|(k, &r)| {
            let r = d.region(r);
            #[allow(clippy::cast_possible_truncation)]
            Region {
                id: RegionId(k as u32),
                parent: region_to(r.parent),
                gate: r.gate.and_then(|(g, p)| port(g, p)),
                body: map_stmts(&r.body),
                rect: Rect::default(),
                ..r.clone()
            }
        })
        .collect();

    // Where the text is, for the blocks and regions that are themselves.
    let own = |n: NodeId| fold_of(n).is_none().then(|| shown[n.0 as usize]);
    let mut diagram = Diagram {
        pou: d.pou.clone(),
        nodes,
        wires,
        regions,
        stmts,
        top: map_stmts(&d.top),
        size: (0.0, 0.0),
        pins: d
            .pins
            .iter()
            .filter_map(|(&(n, k), &s)| Some(((own(n)?, k), s)))
            .collect(),
        gates: d
            .gates
            .iter()
            .filter_map(|(&r, &s)| Some((region_shown[r.0 as usize]?, s)))
            .collect(),
        ops: d.ops.iter().filter_map(|(&n, &s)| Some((own(n)?, s))).collect(),
    };
    layout::layout(&mut diagram);
    View {
        diagram,
        folds: applied,
        skipped,
        origin,
        region_origin,
        ins,
        outs,
        wire_origin,
        shown,
        region_shown,
    }
}
