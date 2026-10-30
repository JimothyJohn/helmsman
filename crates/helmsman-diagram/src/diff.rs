//! What a proposal changes, for review: the source line by line
//! ([`lines`]), and the diagram block by block ([`modules`]). Both are
//! deterministic (same inputs, same answer, no hashing order) and pure,
//! so the canvas draws them and the tests check them natively.
//!
//! **Blocks.** Two lowerings share no ids, so a block is known across
//! versions by what it is and what it computes ([`Sig`]): its POU, the
//! chain of region kinds and arms around it (not their conditions, so a
//! changed IF condition changes the region, not every block inside it),
//! its kind and header, the source text it computes, and what feeds each
//! input. Pairing is in two passes: blocks whose whole signature is equal
//! pair first (unchanged), in source order; of the rest, blocks with the
//! same identity ([`Key`]: POU, scope, kind and header) pair as `changed`
//! (`lamp := a` became `lamp := b`), those that also compute the same text
//! (only their wiring moved) first; what is left was removed (old) or
//! added (new). Regions pair the same way on their kind and label. So the
//! old blocks less the removed and changed ones, plus the added and the
//! changed ones' new versions, are exactly the new blocks (as multisets of
//! signatures): the property `tests/diff.rs` checks.

use std::collections::{BTreeMap, VecDeque};

use crate::lower::Module;
use crate::model::{Diagram, NodeId, NodeKind, Rect, RegionId, RegionKind};

/// One line of a source diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// In both; 1-based line numbers in old and new.
    Same { old: usize, new: usize, text: String },
    /// Only in the old source.
    Removed { old: usize, text: String },
    /// Only in the new source.
    Added { new: usize, text: String },
}

impl Line {
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Same { text, .. } | Self::Removed { text, .. } | Self::Added { text, .. } => text,
        }
    }

    #[must_use]
    pub const fn is_change(&self) -> bool {
        !matches!(self, Self::Same { .. })
    }
}

/// Largest middle (after the common head and tail are trimmed) the exact
/// diff takes, in old lines times new lines; beyond it the middle is
/// shown as all removed then all added (still correct, just coarse).
pub const MAX_CELLS: usize = 4_000_000;

/// The line diff of `old` to `new`: a longest common subsequence of lines
/// (ties broken toward removals first, so the output is fixed), with the
/// common head and tail trimmed first. Line endings (`\n`, `\r\n`) are
/// not part of a line.
#[must_use]
pub fn lines(old: &str, new: &str) -> Vec<Line> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut out = Vec::with_capacity(a.len().max(b.len()));
    for (k, text) in a[..head].iter().enumerate() {
        out.push(Line::Same {
            old: k + 1,
            new: k + 1,
            text: (*text).to_string(),
        });
    }
    middle(am, bm, head, &mut out);
    for k in 0..tail {
        let (i, j) = (a.len() - tail + k, b.len() - tail + k);
        out.push(Line::Same {
            old: i + 1,
            new: j + 1,
            text: a[i].to_string(),
        });
    }
    out
}

fn middle(a: &[&str], b: &[&str], at: usize, out: &mut Vec<Line>) {
    let (n, m) = (a.len(), b.len());
    let removed = |i: usize| Line::Removed {
        old: at + i + 1,
        text: a[i].to_string(),
    };
    let added = |j: usize| Line::Added {
        new: at + j + 1,
        text: b[j].to_string(),
    };
    if n.saturating_mul(m) > MAX_CELLS {
        out.extend((0..n).map(removed));
        out.extend((0..m).map(added));
        return;
    }
    // lcs[i][j]: the LCS length of a[i..] and b[j..].
    let w = m + 1;
    let mut lcs = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * w + j] = if a[i] == b[j] {
                lcs[(i + 1) * w + j + 1] + 1
            } else {
                lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push(Line::Same {
                old: at + i + 1,
                new: at + j + 1,
                text: a[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if j == m || (i < n && lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
            out.push(removed(i));
            i += 1;
        } else {
            out.push(added(j));
            j += 1;
        }
    }
}

/// A block's identity across versions: where it is and what it is, not
/// what it computes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The POU, upper-cased.
    pub pou: String,
    /// The regions around it, outermost first, as kind and arm
    /// (`IF/0`, `CASE/2`, `FOR/0`).
    pub scope: Vec<String>,
    /// Kind and header (`write LAMP`, `op AND`, `fb T : TON`).
    pub what: String,
}

/// A block's whole signature: its [`Key`], the source it computes, and
/// what feeds each of its inputs (the feeding block's kind and header,
/// and which of its outputs).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sig {
    pub key: Key,
    pub text: String,
    pub ins: Vec<String>,
}

/// How a block or region differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    /// Only in the new version.
    Added,
    /// Only in the old version.
    Removed,
    /// In both, computing something else (or gated by something else).
    Changed,
}

/// One block that differs. `old` and `new` index the POU's diagram in
/// each version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeChange {
    pub pou: String,
    pub change: Change,
    pub old: Option<(NodeId, Sig)>,
    pub new: Option<(NodeId, Sig)>,
}

/// One region (IF/CASE arm, loop body) that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionChange {
    pub pou: String,
    pub change: Change,
    pub old: Option<(RegionId, String)>,
    pub new: Option<(RegionId, String)>,
}

/// What differs between two lowerings of a module, in a fixed order:
/// changed and added blocks in the new version's order, then removed
/// ones in the old version's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModuleDiff {
    pub nodes: Vec<NodeChange>,
    pub regions: Vec<RegionChange>,
    /// POUs only in the new version, and only in the old.
    pub pous_added: Vec<String>,
    pub pous_removed: Vec<String>,
}

impl ModuleDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.regions.is_empty()
            && self.pous_added.is_empty()
            && self.pous_removed.is_empty()
    }

    /// The blocks of `pou` in the new version that are `change`.
    #[must_use]
    pub fn new_nodes(&self, pou: &str, change: Change) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|c| c.pou == pou && c.change == change)
            .filter_map(|c| c.new.as_ref().map(|(id, _)| *id))
            .collect()
    }

    /// The blocks of `pou` that were removed, as ids in the old version.
    #[must_use]
    pub fn removed_nodes(&self, pou: &str) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|c| c.pou == pou && c.change == Change::Removed)
            .filter_map(|c| c.old.as_ref().map(|(id, _)| *id))
            .collect()
    }

    /// The regions of `pou` in the new version that were added or changed.
    #[must_use]
    pub fn new_regions(&self, pou: &str) -> Vec<(RegionId, Change)> {
        self.regions
            .iter()
            .filter(|c| c.pou == pou)
            .filter_map(|c| c.new.as_ref().map(|(id, _)| (*id, c.change)))
            .collect()
    }

    /// POUs (upper-cased) with any difference, in first-seen order.
    #[must_use]
    pub fn pous(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let names = self
            .nodes
            .iter()
            .map(|c| &c.pou)
            .chain(self.regions.iter().map(|c| &c.pou))
            .chain(&self.pous_added)
            .chain(&self.pous_removed);
        for n in names {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
        out
    }

    /// Counts of added, removed and changed blocks.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        let n = |k| self.nodes.iter().filter(|c| c.change == k).count();
        (n(Change::Added), n(Change::Removed), n(Change::Changed))
    }
}

const fn region_kind(k: RegionKind) -> &'static str {
    match k {
        RegionKind::If => "IF",
        RegionKind::Elsif => "ELSIF",
        RegionKind::Else => "ELSE",
        RegionKind::Case => "CASE",
        RegionKind::CaseElse => "CASE-ELSE",
        RegionKind::For => "FOR",
        RegionKind::While => "WHILE",
        RegionKind::Repeat => "REPEAT",
    }
}

fn what(k: &NodeKind, label: &str) -> String {
    let tag = match k {
        NodeKind::Read { .. } => "read",
        NodeKind::Const { .. } => "const",
        NodeKind::Op { .. } => "op",
        NodeKind::Func { .. } => "func",
        NodeKind::Fb { .. } => "fb",
        NodeKind::Access { .. } => "access",
        NodeKind::Write { .. } => "write",
        NodeKind::For { .. } => "for",
        NodeKind::Jump => "jump",
    };
    format!("{tag} {label}")
}

fn scope(d: &Diagram, region: Option<RegionId>) -> Vec<String> {
    let mut s: Vec<String> = d
        .chain(region)
        .map(|r| format!("{}/{}", region_kind(r.kind), r.arm))
        .collect();
    s.reverse();
    s
}

/// Every block of `d` (POU `pou`) with its signature, in id order.
#[must_use]
pub fn signatures(pou: &str, d: &Diagram) -> Vec<Sig> {
    d.nodes
        .iter()
        .map(|n| {
            let mut ins = vec![String::new(); n.ins.len()];
            for w in d.inputs(n.id) {
                let from = d.node(w.from);
                let s = format!("{}.{}", what(&from.kind, &from.label), w.out);
                match ins.get_mut(w.inp) {
                    Some(slot) => *slot = s,
                    None => ins.push(s),
                }
            }
            Sig {
                key: Key {
                    pou: pou.to_string(),
                    scope: scope(d, n.region),
                    what: what(&n.kind, &n.label),
                },
                text: n.text.clone(),
                ins,
            }
        })
        .collect()
}

/// Where an item sits: its POU and index there.
type Slot = (String, usize);

/// A region's identity: its POU, the scope around it, its kind and arm.
type RegionKey = (String, Vec<String>, String);

/// A region's identity and full signature: (POU, scope around it, kind
/// and arm) and its label.
fn region_sigs(pou: &str, d: &Diagram) -> Vec<(RegionKey, String)> {
    d.regions
        .iter()
        .map(|r| {
            (
                (
                    pou.to_string(),
                    scope(d, r.parent),
                    format!("{}/{}", region_kind(r.kind), r.arm),
                ),
                r.label.clone(),
            )
        })
        .collect()
}

/// How items pair up: unchanged (old, new), changed (old, new), removed
/// (old), added (new). Indices into the inputs.
#[derive(Debug, Default)]
struct Pairing {
    same: Vec<(usize, usize)>,
    changed: Vec<(usize, usize)>,
    removed: Vec<usize>,
    added: Vec<usize>,
}

/// Pairs equal `full`s first (k-th old with k-th new of each value), then
/// the rest by equal `mid` (same identity and text: only the wiring
/// moved), then by equal `key`, the same way; what is left is removed or
/// added.
fn pair<K: Ord, M: Ord, F: Ord>(old: &[(K, M, F)], new: &[(K, M, F)]) -> Pairing {
    let mut p = Pairing::default();
    let mut old_left = vec![true; old.len()];
    let mut new_left: Vec<usize> = (0..new.len()).collect();
    for pass in 0u8..3 {
        let mut by: BTreeMap<Probe<'_, K, M, F>, VecDeque<usize>> = BTreeMap::new();
        for (i, item) in old.iter().enumerate() {
            if old_left[i] {
                by.entry(Probe::of(pass, item)).or_default().push_back(i);
            }
        }
        let mut still = Vec::new();
        for j in new_left {
            match by
                .get_mut(&Probe::of(pass, &new[j]))
                .and_then(VecDeque::pop_front)
            {
                Some(i) => {
                    old_left[i] = false;
                    if pass == 0 {
                        p.same.push((i, j));
                    } else {
                        p.changed.push((i, j));
                    }
                }
                None => still.push(j),
            }
        }
        new_left = still;
    }
    p.added = new_left;
    p.removed = (0..old.len()).filter(|&i| old_left[i]).collect();
    p
}

/// What one pass of [`pair`] compares.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Probe<'a, K, M, F> {
    Full(&'a F),
    Mid(&'a K, &'a M),
    Key(&'a K),
}

impl<'a, K, M, F> Probe<'a, K, M, F> {
    const fn of(pass: u8, item: &'a (K, M, F)) -> Self {
        match pass {
            0 => Self::Full(&item.2),
            1 => Self::Mid(&item.0, &item.1),
            _ => Self::Key(&item.0),
        }
    }
}

/// What differs between the lowerings `old` and `new` of a module. POUs
/// pair by name; a POU in only one of them counts every block of it as
/// added or removed.
#[must_use]
pub fn modules(old: &Module, new: &Module) -> ModuleDiff {
    let mut out = ModuleDiff::default();
    // Every block of every POU at once, so a block moved between POUs is
    // removed from one and added to the other.
    let mut old_nodes = Vec::new();
    let mut new_nodes = Vec::new();
    let mut old_regions = Vec::new();
    let mut new_regions = Vec::new();
    for p in &old.pous {
        for (k, s) in signatures(&p.name, &p.diagram).into_iter().enumerate() {
            old_nodes.push(((p.name.clone(), k), s));
        }
        for (k, s) in region_sigs(&p.name, &p.diagram).into_iter().enumerate() {
            old_regions.push(((p.name.clone(), k), s));
        }
        if new.pou(&p.name).is_none() {
            out.pous_removed.push(p.name.clone());
        }
    }
    for p in &new.pous {
        for (k, s) in signatures(&p.name, &p.diagram).into_iter().enumerate() {
            new_nodes.push(((p.name.clone(), k), s));
        }
        for (k, s) in region_sigs(&p.name, &p.diagram).into_iter().enumerate() {
            new_regions.push(((p.name.clone(), k), s));
        }
        if old.pou(&p.name).is_none() {
            out.pous_added.push(p.name.clone());
        }
    }
    let id = |k: usize| NodeId(u32::try_from(k).unwrap_or(u32::MAX));
    let rid = |k: usize| RegionId(u32::try_from(k).unwrap_or(u32::MAX));

    let keyed = |v: &[((String, usize), Sig)]| -> Vec<(Key, String, Sig)> {
        v.iter()
            .map(|(_, s)| (s.key.clone(), s.text.clone(), s.clone()))
            .collect()
    };
    let p = pair(&keyed(&old_nodes), &keyed(&new_nodes));
    let mut nodes: Vec<(usize, NodeChange)> = Vec::new();
    for &(i, j) in &p.changed {
        let ((_, a), sa) = &old_nodes[i];
        let ((pou, b), sb) = &new_nodes[j];
        nodes.push((
            j,
            NodeChange {
                pou: pou.clone(),
                change: Change::Changed,
                old: Some((id(*a), sa.clone())),
                new: Some((id(*b), sb.clone())),
            },
        ));
    }
    for &j in &p.added {
        let ((pou, b), sb) = &new_nodes[j];
        nodes.push((
            j,
            NodeChange {
                pou: pou.clone(),
                change: Change::Added,
                old: None,
                new: Some((id(*b), sb.clone())),
            },
        ));
    }
    nodes.sort_by_key(|(j, _)| *j);
    out.nodes = nodes.into_iter().map(|(_, c)| c).collect();
    for &i in &p.removed {
        let ((pou, a), sa) = &old_nodes[i];
        out.nodes.push(NodeChange {
            pou: pou.clone(),
            change: Change::Removed,
            old: Some((id(*a), sa.clone())),
            new: None,
        });
    }

    let rk = |v: &[(Slot, (RegionKey, String))]| {
        v.iter()
            .map(|(_, (k, l))| (k.clone(), l.clone(), (k.clone(), l.clone())))
            .collect::<Vec<_>>()
    };
    let p = pair(&rk(&old_regions), &rk(&new_regions));
    let mut regions: Vec<(usize, RegionChange)> = Vec::new();
    for &(i, j) in &p.changed {
        let ((_, a), (_, la)) = &old_regions[i];
        let ((pou, b), (_, lb)) = &new_regions[j];
        regions.push((
            j,
            RegionChange {
                pou: pou.clone(),
                change: Change::Changed,
                old: Some((rid(*a), la.clone())),
                new: Some((rid(*b), lb.clone())),
            },
        ));
    }
    for &j in &p.added {
        let ((pou, b), (_, lb)) = &new_regions[j];
        regions.push((
            j,
            RegionChange {
                pou: pou.clone(),
                change: Change::Added,
                old: None,
                new: Some((rid(*b), lb.clone())),
            },
        ));
    }
    regions.sort_by_key(|(j, _)| *j);
    out.regions = regions.into_iter().map(|(_, c)| c).collect();
    for &i in &p.removed {
        let ((pou, a), (_, la)) = &old_regions[i];
        out.regions.push(RegionChange {
            pou: pou.clone(),
            change: Change::Removed,
            old: Some((rid(*a), la.clone())),
            new: None,
        });
    }
    out
}

/// Where removed blocks of `old` are drawn beside the new diagram: their
/// old layout, kept together, moved down to start `gap` below `below`
/// (the new diagram's height). With the wires among them (old wire
/// indices whose both ends were removed).
#[must_use]
pub fn ghosts(old: &Diagram, removed: &[NodeId], below: f32, gap: f32) -> (Vec<(NodeId, Rect)>, Vec<usize>) {
    let top = removed
        .iter()
        .filter_map(|id| old.nodes.get(id.0 as usize))
        .map(|n| n.rect.y)
        .fold(f32::INFINITY, f32::min);
    if !top.is_finite() {
        return (Vec::new(), Vec::new());
    }
    let dy = below + gap - top;
    let rects = removed
        .iter()
        .filter_map(|id| old.nodes.get(id.0 as usize))
        .map(|n| {
            (
                n.id,
                Rect {
                    y: n.rect.y + dy,
                    ..n.rect
                },
            )
        })
        .collect();
    let wires = old
        .wires
        .iter()
        .enumerate()
        .filter(|(_, w)| removed.contains(&w.from) && removed.contains(&w.to))
        .map(|(k, _)| k)
        .collect();
    (rects, wires)
}
