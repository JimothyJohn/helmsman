//! The check for edits that change what feeds a block, or remove or rename
//! things: a program's diagram as one tree per statement root, compared
//! with the re-lowered one.
//!
//! Block numbers shift when a leaf is added or goes (a variable read
//! shared by two pins becomes two), so blocks are not matched by number.
//! A statement is instead unfolded from its roots (the writes, the call,
//! the conditions) into a tree: a block is its kind, its header and its
//! inputs; a leaf is a literal, or a variable read from memory or from an
//! earlier statement's write. Two diagrams are the same when their trees
//! are, with the edit's one allowed difference left open: a hole where a
//! pin's source was, or a block on the way up from that pin whose text
//! changes with it.

use std::collections::HashMap;

use helmsman_iec::trace::StmtId;

use crate::lower::Pou;
use crate::model::{Diagram, NodeId, NodeKind, RegionKind, Wire};

/// Where a variable's value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Origin {
    /// Read from the variable.
    Memory,
    /// Wired from a block of another statement (named by statement and
    /// write number, or by the instance a call wrote).
    Earlier(String),
    /// Whatever the new text gives: the old origin was removed.
    Any,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tree {
    /// A block: its kind's name, its full key (`None` on the old side where
    /// the edit rewrites the block's text), then its inputs (negation
    /// bubble, source).
    Node(&'static str, Option<String>, Vec<(bool, Tree)>),
    /// A literal, as its kind and value.
    Leaf(String),
    /// A variable.
    Var(String, Origin),
    /// The pin the edit addresses.
    Hole,
}

/// How the addressed block's inputs change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Hole {
    /// Input `i` takes another source.
    Replace(usize),
    /// A new last input.
    Append,
}

pub(super) struct StmtDigest {
    region: String,
    roots: Vec<Tree>,
    regions: Vec<String>,
}

pub(super) struct Digest {
    stmts: Vec<StmtDigest>,
}

fn variant(k: &NodeKind) -> &'static str {
    match k {
        NodeKind::Read { .. } => "read",
        NodeKind::Const { .. } => "const",
        NodeKind::Op { .. } => "op",
        NodeKind::Func { .. } => "func",
        NodeKind::Fb { .. } => "fb",
        NodeKind::Access { .. } => "access",
        NodeKind::Write { .. } => "write",
        NodeKind::For { .. } => "for",
        NodeKind::Jump => "jump",
    }
}

struct Build<'a> {
    d: &'a Diagram,
    ord: HashMap<StmtId, usize>,
    /// The n-th write of its statement, per node (0 for any other).
    write_no: Vec<usize>,
    remap: &'a dyn Fn(usize) -> Option<usize>,
    /// The block whose inputs change (old side only).
    target: Option<(NodeId, Hole)>,
    /// The statement whose region labels may change with its text.
    open_labels: Option<usize>,
}

impl Build<'_> {
    fn stmt_of(&self, n: NodeId) -> usize {
        self.ord[&self.d.node(n).stmt]
    }

    fn earlier(&self, n: NodeId) -> Origin {
        let node = self.d.node(n);
        let at = self.stmt_of(n);
        match (&node.kind, (self.remap)(at)) {
            (_, None) => Origin::Any,
            (NodeKind::Write { .. }, Some(o)) => {
                Origin::Earlier(format!("w{o}.{}", self.write_no[n.0 as usize]))
            }
            (NodeKind::For { .. }, Some(o)) => Origin::Earlier(format!("for{o}")),
            (NodeKind::Fb { instance, .. }, Some(_)) => Origin::Earlier(format!("fb {instance}")),
            (_, Some(o)) => Origin::Earlier(format!("x{o}.{}", n.0)),
        }
    }

    fn edge(&self, w: &Wire, cur: usize) -> (Tree, bool) {
        let from = self.d.node(w.from);
        let other = self.stmt_of(w.from) != cur;
        match &from.kind {
            NodeKind::Read { path } => (Tree::Var(path.clone(), Origin::Memory), false),
            NodeKind::Const { .. } => (Tree::Leaf(format!("{:?}", from.kind)), false),
            NodeKind::Write { path } if other => (Tree::Var(path.clone(), self.earlier(w.from)), false),
            NodeKind::For { var } if other => (Tree::Var(var.clone(), self.earlier(w.from)), false),
            NodeKind::Fb { instance, .. } => {
                let port = from.outs.get(w.out).cloned().unwrap_or_default();
                (
                    Tree::Var(format!("{instance}.{port}"), self.earlier(w.from)),
                    false,
                )
            }
            _ => self.node(w.from),
        }
    }

    /// The tree of `n`, and whether the hole is somewhere below it.
    fn node(&self, n: NodeId) -> (Tree, bool) {
        let node = self.d.node(n);
        let cur = self.stmt_of(n);
        let mut kids = Vec::new();
        let mut open = false;
        for w in self.d.inputs(n) {
            let (t, o) = self.edge(w, cur);
            open |= o;
            let hole = matches!(self.target, Some((t, Hole::Replace(i))) if t == n && i == w.inp);
            kids.push(if hole {
                (w.negated, Tree::Hole)
            } else {
                (w.negated, t)
            });
        }
        if let Some((t, Hole::Append)) = self.target
            && t == n
        {
            kids.push((false, Tree::Hole));
        }
        let here = matches!(self.target, Some((t, _)) if t == n);
        let open = open || here;
        let key = (!open).then(|| format!("{:?}|{}|{:?}", node.kind, node.label, node.ins));
        (Tree::Node(variant(&node.kind), key, kids), open)
    }

    fn region_key(&self, r: crate::model::RegionId) -> String {
        let reg = self.d.region(r);
        let at = (self.remap)(self.ord[&reg.stmt]);
        format!("{at:?}.{}", reg.arm)
    }

    fn digest(&self) -> Digest {
        let mut stmts = Vec::new();
        for (i, s) in self.d.stmts.iter().enumerate() {
            if (self.remap)(i).is_none() {
                continue;
            }
            let roots = s.roots.iter().map(|&r| self.node(r).0).collect();
            let regions = s
                .arms
                .iter()
                // An ELSE with nothing left in it is not a region.
                .filter(|&&r| {
                    let reg = self.d.region(r);
                    let emptied = !reg.body.is_empty() && reg.body.iter().all(|&b| (self.remap)(b).is_none());
                    !(emptied && matches!(reg.kind, RegionKind::Else | RegionKind::CaseElse))
                })
                .map(|&r| {
                    let reg = self.d.region(r);
                    let label = if self.open_labels == Some(i) {
                        ""
                    } else {
                        reg.label.as_str()
                    };
                    let parent = reg.parent.map_or_else(String::new, |p| self.region_key(p));
                    format!(
                        "{:?}|{}|{}|{}|{parent}|{label}",
                        reg.kind,
                        reg.arm,
                        reg.negated,
                        reg.gate.is_some()
                    )
                })
                .collect();
            stmts.push(StmtDigest {
                region: s.region.map_or_else(String::new, |r| self.region_key(r)),
                roots,
                regions,
            });
        }
        Digest { stmts }
    }
}

/// Everything that identifies a statement or a write across lowerings.
pub(super) struct Opts<'a> {
    pub remap: &'a dyn Fn(usize) -> Option<usize>,
    pub target: Option<(NodeId, Hole)>,
    pub open_labels: Option<usize>,
}

pub(super) fn digest(d: &Diagram, o: &Opts<'_>) -> Digest {
    let ord: HashMap<StmtId, usize> = d.stmts.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    let mut count: HashMap<usize, usize> = HashMap::new();
    let write_no = d
        .nodes
        .iter()
        .map(|n| {
            if matches!(n.kind, NodeKind::Write { .. }) {
                let c = count.entry(ord[&n.stmt]).or_default();
                *c += 1;
                *c - 1
            } else {
                0
            }
        })
        .collect();
    Build {
        d,
        ord,
        write_no,
        remap: o.remap,
        target: o.target,
        open_labels: o.open_labels,
    }
    .digest()
}

/// The statement number of `node` in `d`.
pub(super) fn stmt_ordinal(d: &Diagram, node: NodeId) -> usize {
    d.stmts
        .iter()
        .position(|s| s.id == d.node(node).stmt)
        .unwrap_or(0)
}

/// The identity a later statement's read of this write is wired by.
pub(super) fn write_origin(d: &Diagram, node: NodeId) -> Origin {
    let ord: HashMap<StmtId, usize> = d.stmts.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    let at = ord[&d.node(node).stmt];
    let no = d
        .nodes
        .iter()
        .filter(|n| n.stmt == d.node(node).stmt && matches!(n.kind, NodeKind::Write { .. }))
        .position(|n| n.id == node)
        .unwrap_or(0);
    Origin::Earlier(format!("w{at}.{no}"))
}

/// Whether `new` is `old` with its hole filled; the filling is returned.
/// A hole's bubble and source are `got`.
fn lift(old: &Tree, new: &Tree, got: &mut Option<(bool, Tree)>, loose: bool) -> bool {
    match (old, new) {
        (Tree::Node(v1, k1, c1), Tree::Node(v2, k2, c2)) => {
            if v1 != v2 || (k1.is_some() && k1 != k2) || c1.len() != c2.len() {
                return false;
            }
            for ((n1, t1), (n2, t2)) in c1.iter().zip(c2) {
                if *t1 == Tree::Hole {
                    if got.replace((*n2, t2.clone())).is_some() {
                        return false;
                    }
                } else if n1 != n2 || !lift(t1, t2, got, loose) {
                    return false;
                }
            }
            true
        }
        (Tree::Leaf(a), Tree::Leaf(b)) => a == b,
        (Tree::Var(p1, o1), Tree::Var(p2, o2)) => {
            p1 == p2 && (*o1 == Origin::Any || o1 == o2 || (loose && *o1 == Origin::Memory))
        }
        _ => false,
    }
}

/// Compares two digests; `Ok(Some(..))` is what filled the hole.
///
/// `loose`: a variable read from memory may be wired from an earlier write
/// now (a removed statement stops hiding it).
pub(super) fn compare(old: &Digest, new: &Digest, loose: bool) -> Result<Option<(bool, Tree)>, ()> {
    if old.stmts.len() != new.stmts.len() {
        return Err(());
    }
    let mut got = None;
    for (a, b) in old.stmts.iter().zip(&new.stmts) {
        if a.region != b.region || a.regions != b.regions || a.roots.len() != b.roots.len() {
            return Err(());
        }
        for (x, y) in a.roots.iter().zip(&b.roots) {
            if !lift(x, y, &mut got, loose) {
                return Err(());
            }
        }
    }
    Ok(got)
}

/// The POUs of two modules compared with no hole and no removal.
pub(super) fn same_pou(a: &Pou, b: &Pou) -> bool {
    let id = |i: usize| Some(i);
    let o = Opts {
        remap: &id,
        target: None,
        open_labels: None,
    };
    matches!(
        compare(&digest(&a.diagram, &o), &digest(&b.diagram, &o), false),
        Ok(None)
    )
}
