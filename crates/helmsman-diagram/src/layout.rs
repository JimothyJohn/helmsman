//! Deterministic left-to-right layout.
//!
//! Statements stack top to bottom in source order, each in its own band;
//! within a band its nodes are layered by longest path from the band's
//! sources (reads and literals in column 0, the assignment in the last
//! column) and stacked as a tidy tree from the band's roots. An IF or CASE
//! puts each arm's condition on the left of the arm's region box, the box
//! holding the arm's statements as a nested block; loops alike.
//!
//! Positions depend only on the statement and those before it in its
//! block, so editing a statement moves nothing above it, and the same
//! source always lays out the same. Sizes come from label lengths, never
//! from a font, so the library needs no UI.

use std::collections::HashSet;

use crate::model::{Diagram, NodeId, Rect, RegionId};

/// Width of one character of a label, in diagram units.
pub const CHAR_W: f32 = 7.0;
pub const HEADER: f32 = 20.0;
pub const PORT_ROW: f32 = 16.0;
const COL_GAP: f32 = 36.0;
const ROW_GAP: f32 = 10.0;
const STMT_GAP: f32 = 18.0;
const PAD: f32 = 12.0;
/// Room for a region's label above its statements.
pub const LABEL_H: f32 = 22.0;
const MIN_BOX_W: f32 = 120.0;

fn size(label: &str, ins: &[String], outs: &[String]) -> (f32, f32) {
    let port_chars =
        ins.iter().map(String::len).max().unwrap_or(0) + outs.iter().map(String::len).max().unwrap_or(0);
    let chars = label.chars().count().max(port_chars + 2) as f32;
    let rows = ins.len().max(outs.len()).max(1) as f32;
    ((chars * CHAR_W + 20.0).max(56.0), HEADER + rows * PORT_ROW + 4.0)
}

/// Lays out every node and region of `d` in place.
pub fn layout(d: &mut Diagram) {
    for n in &mut d.nodes {
        let (w, h) = size(&n.label, &n.ins, &n.outs);
        n.rect = Rect { x: 0.0, y: 0.0, w, h };
    }
    let mut l = Layout {
        d,
        placed: HashSet::new(),
    };
    let top = l.d.top.clone();
    let (w, h) = l.block(&top, 0.0, 0.0);
    d.size = (w, h);
}

/// The nodes inside region `r`, nested regions' too: what moves with it.
#[must_use]
pub fn inside(d: &Diagram, r: RegionId) -> Vec<NodeId> {
    let within = |mut at: Option<RegionId>| {
        while let Some(q) = at {
            if q == r {
                return true;
            }
            at = d.region(q).parent;
        }
        false
    };
    d.nodes
        .iter()
        .filter(|n| within(n.region))
        .map(|n| n.id)
        .collect()
}

/// Each region's box around what is in it, after nodes moved (the inner
/// regions first), and the diagram's size around everything. On a fresh
/// layout it changes nothing.
pub fn refit(d: &mut Diagram) {
    let depth = |d: &Diagram, r: &crate::model::Region| {
        let (mut k, mut at) = (0, r.parent);
        while let Some(q) = at {
            k += 1;
            at = d.region(q).parent;
        }
        k
    };
    let mut order: Vec<(usize, usize)> = d.regions.iter().map(|r| (depth(d, r), r.id.0 as usize)).collect();
    order.sort_unstable_by(|a, b| b.cmp(a));
    for (_, i) in order {
        let id = d.regions[i].id;
        let inner = d
            .nodes
            .iter()
            .filter(|n| n.region == Some(id))
            .map(|n| n.rect)
            .chain(d.regions.iter().filter(|q| q.parent == Some(id)).map(|q| q.rect));
        let Some((x0, y0, x1, y1)) = bounds(inner) else {
            continue;
        };
        let label_w = d.regions[i].label.chars().count() as f32 * CHAR_W + 2.0 * PAD;
        d.regions[i].rect = Rect {
            x: x0 - PAD,
            y: y0 - LABEL_H,
            w: (x1 - x0 + 2.0 * PAD).max(MIN_BOX_W).max(label_w),
            h: y1 - y0 + LABEL_H + PAD,
        };
    }
    let all = d
        .nodes
        .iter()
        .map(|n| n.rect)
        .chain(d.regions.iter().map(|r| r.rect));
    if let Some((_, _, x1, y1)) = bounds(all) {
        d.size = (x1.max(0.0), y1.max(0.0));
    }
}

fn bounds(rects: impl Iterator<Item = Rect>) -> Option<(f32, f32, f32, f32)> {
    rects.fold(None, |acc, r| {
        Some(match acc {
            None => (r.x, r.y, r.right(), r.bottom()),
            Some((a, b, c, e)) => (a.min(r.x), b.min(r.y), c.max(r.right()), e.max(r.bottom())),
        })
    })
}

struct Layout<'a> {
    d: &'a mut Diagram,
    placed: HashSet<NodeId>,
}

impl Layout<'_> {
    /// Stacks statements from `(x0, y0)`; returns the block's size.
    fn block(&mut self, stmts: &[usize], x0: f32, y0: f32) -> (f32, f32) {
        let (mut w, mut y) = (0.0f32, y0);
        for (k, &s) in stmts.iter().enumerate() {
            if k > 0 {
                y += STMT_GAP;
            }
            let (sw, sh) = self.stmt(s, x0, y);
            w = w.max(sw);
            y += sh;
        }
        (w, y - y0)
    }

    fn stmt(&mut self, s: usize, x0: f32, y0: f32) -> (f32, f32) {
        let st = self.d.stmts[s].clone();
        if st.arms.is_empty() {
            let (members, w, h) = self.group(&st.roots, st.id, st.region);
            self.shift(&members, x0, y0);
            return (w, h);
        }
        // Gates first, at the origin, to learn their sizes.
        let mut gates: Vec<Option<(Vec<NodeId>, f32, f32)>> = Vec::new();
        for &r in &st.arms {
            let g = self.d.region(r).gate.map(|(n, _)| n);
            gates.push(match g {
                Some(n) if !self.placed.contains(&n) => Some(self.group(&[n], st.id, st.region)),
                _ => None,
            });
        }
        let gate_w = gates.iter().flatten().map(|g| g.1).fold(0.0f32, f32::max);
        let bx = if gate_w > 0.0 { x0 + gate_w + COL_GAP } else { x0 };
        let (mut y, mut w) = (y0, 0.0f32);
        for (k, (&r, gate)) in st.arms.iter().zip(gates).enumerate() {
            if k > 0 {
                y += ROW_GAP;
            }
            let body = self.d.region(r).body.clone();
            let (iw, ih) = self.block(&body, bx + PAD, y + LABEL_H);
            let label_w = self.d.region(r).label.chars().count() as f32 * CHAR_W + 2.0 * PAD;
            let rect = Rect {
                x: bx,
                y,
                w: (iw + 2.0 * PAD).max(MIN_BOX_W).max(label_w),
                h: LABEL_H + ih + PAD,
            };
            self.d.regions[r.0 as usize].rect = rect;
            let mut band = rect.h;
            if let Some((members, gw, gh)) = gate {
                // Right-aligned against the box, level with its label.
                self.shift(&members, bx - COL_GAP - gw, y + 2.0);
                band = band.max(gh + 2.0);
            }
            w = w.max(rect.right() - x0);
            y += band;
        }
        (w, y - y0)
    }

    /// The nodes of one statement part reachable backwards from `roots`
    /// without leaving statement `stmt` and region `region`, laid out with
    /// their top-left at the origin. Returns them and the group's size.
    fn group(
        &mut self,
        roots: &[NodeId],
        stmt: helmsman_iec::trace::StmtId,
        region: Option<RegionId>,
    ) -> (Vec<NodeId>, f32, f32) {
        // Members, by backward search.
        let mut members: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack: Vec<NodeId> = roots.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            if self.placed.contains(&n) || !seen.insert(n) {
                continue;
            }
            members.push(n);
            for w in self.d.inputs(n).collect::<Vec<_>>().into_iter().rev() {
                let src = self.d.node(w.from);
                if src.stmt == stmt && src.region == region {
                    stack.push(w.from);
                }
            }
        }
        members.sort_unstable();
        let is_member = |n: NodeId| members.binary_search(&n).is_ok();
        let preds = |d: &Diagram, n: NodeId| -> Vec<NodeId> {
            let mut p: Vec<NodeId> = d.inputs(n).map(|w| w.from).filter(|&f| is_member(f)).collect();
            p.dedup();
            p
        };
        // Columns: longest path from the group's sources. Ids are a
        // topological order (a producer is always created first).
        let mut col = vec![0usize; members.len()];
        for (i, &n) in members.iter().enumerate() {
            col[i] = preds(self.d, n)
                .iter()
                .filter_map(|p| members.binary_search(p).ok())
                .map(|j| col[j] + 1)
                .max()
                .unwrap_or(0);
        }
        // Statement ends sit in the last column.
        let last = col.iter().copied().max().unwrap_or(0);
        let feeds: HashSet<NodeId> = members.iter().flat_map(|&n| preds(self.d, n)).collect();
        for (i, n) in members.iter().enumerate() {
            if roots.contains(n) && !feeds.contains(n) && !preds(self.d, *n).is_empty() {
                col[i] = last;
            }
        }
        let cols = last + 1;
        let mut col_w = vec![0.0f32; cols];
        for (i, &n) in members.iter().enumerate() {
            col_w[col[i]] = col_w[col[i]].max(self.d.node(n).rect.w);
        }
        let mut col_x = vec![0.0f32; cols];
        for c in 1..cols {
            col_x[c] = col_x[c - 1] + col_w[c - 1] + COL_GAP;
        }
        // Rows: a tidy tree from the roots, leaves stacked in visit order,
        // each parent centred on its inputs, pushed down if its column is
        // taken there.
        let mut col_bottom = vec![0.0f32; cols];
        let mut cursor = 0.0f32;
        let mut done: HashSet<NodeId> = HashSet::new();
        let mut order: Vec<NodeId> = roots.iter().copied().filter(|n| is_member(*n)).collect();
        order.extend(members.iter().copied().filter(|n| !roots.contains(n)));
        for r in order {
            self.visit(r, &members, &col, &col_x, &mut col_bottom, &mut cursor, &mut done);
        }
        let w = col_x[cols - 1] + col_w[cols - 1];
        let h = members
            .iter()
            .map(|&n| self.d.node(n).rect.bottom())
            .fold(0.0f32, f32::max);
        for &n in &members {
            self.placed.insert(n);
        }
        (members, w, h)
    }

    #[allow(clippy::too_many_arguments)]
    fn visit(
        &mut self,
        n: NodeId,
        members: &[NodeId],
        col: &[usize],
        col_x: &[f32],
        col_bottom: &mut [f32],
        cursor: &mut f32,
        done: &mut HashSet<NodeId>,
    ) {
        let Ok(i) = members.binary_search(&n) else {
            return;
        };
        if !done.insert(n) {
            return;
        }
        let mut preds: Vec<NodeId> = self
            .d
            .inputs(n)
            .map(|w| w.from)
            .filter(|f| members.binary_search(f).is_ok())
            .collect();
        preds.dedup();
        for &p in &preds {
            self.visit(p, members, col, col_x, col_bottom, cursor, done);
        }
        let c = col[i];
        let h = self.d.node(n).rect.h;
        let want = if preds.is_empty() {
            *cursor
        } else {
            let centres: f32 = preds
                .iter()
                .map(|&p| {
                    let r = self.d.node(p).rect;
                    r.y + r.h / 2.0
                })
                .sum();
            centres / preds.len() as f32 - h / 2.0
        };
        let y = want.max(col_bottom[c]).max(0.0);
        col_bottom[c] = y + h + ROW_GAP;
        *cursor = cursor.max(y + h + ROW_GAP);
        let r = &mut self.d.nodes[n.0 as usize].rect;
        r.x = col_x[c];
        r.y = y;
    }

    fn shift(&mut self, members: &[NodeId], dx: f32, dy: f32) {
        for &n in members {
            let r = &mut self.d.nodes[n.0 as usize].rect;
            r.x += dx;
            r.y += dy;
        }
    }
}

/// Where input `k` of a node with box `r` meets its left edge.
#[must_use]
pub fn in_port(r: &Rect, k: usize) -> (f32, f32) {
    (r.x, port_y(r, k))
}

/// Where output `k` of a node with box `r` meets its right edge.
#[must_use]
pub fn out_port(r: &Rect, k: usize) -> (f32, f32) {
    (r.right(), port_y(r, k))
}

fn port_y(r: &Rect, k: usize) -> f32 {
    r.y + HEADER + PORT_ROW * (k as f32 + 0.5) + 2.0
}
