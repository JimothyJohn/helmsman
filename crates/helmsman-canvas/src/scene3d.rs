//! The 3D canvas's scene, built from `helmsman-diagram`: no GL, no egui.
//!
//! Levels by height. The middle level (y = [`MIDDLE_Y`]) holds the
//! program's POUs as nodes (the program, then each user function block
//! instance, as the Viewer's scopes list them), joined to the instance
//! that calls them. The bottom level (y = 0) is one board per POU: its
//! blocks and wires from the deterministic layout, the diagram's x along
//! the world's x and its y along z (toward the viewer). A region (an IF
//! arm, a loop body) is a thin slab and lifts what it holds, so nesting
//! gives depth within the level. The top level (the plant and its
//! devices) is a later card (`canvas-3d-devices`): the scene leaves the
//! space above [`MIDDLE_Y`] empty and holds nothing for it.

use helmsman_diagram::layout::{in_port, out_port};
use helmsman_diagram::{Diagram, Live, NodeId, NodeKind, RegionId};
use helmsman_iec::Value;

use crate::cam3d::{Aabb, Ray, V3, pick};

/// World units per diagram unit.
pub const UNIT: f32 = 1.0 / 40.0;
/// Height of the middle level above the bottom one.
pub const MIDDLE_Y: f32 = 9.0;
/// How tall a block stands on its board.
pub const BLOCK_H: f32 = 0.35;
/// How much a nesting region lifts what it holds.
pub const LIFT: f32 = 0.3;
/// Space between boards.
pub const GAP: f32 = 3.0;
/// A POU node's size (x, y, z).
pub const POU_SIZE: V3 = [4.5, 0.7, 2.2];

/// What is on one scope of the scene.
pub struct ScopeIn<'a> {
    pub label: String,
    pub diagram: &'a Diagram,
    /// The scan on it, if one is traced.
    pub live: Option<&'a Live>,
    /// The scope that calls this one (a function block instance's caller).
    pub parent: Option<usize>,
}

/// What can be picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    /// A POU node of the middle level, by scope.
    Pou(usize),
    /// A block of a scope's board.
    Block(usize, NodeId),
}

impl Item {
    #[must_use]
    pub const fn scope(self) -> usize {
        match self {
            Self::Pou(s) | Self::Block(s, _) => s,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Middle,
    Bottom,
}

/// What a solid is, for its colour (the renderer reads the theme).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Pou,
    Board,
    Read,
    Write,
    Op,
    Func,
    Fb,
    Const,
    Other,
    Region,
}

/// What a wire carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// No scan on it.
    Idle,
    /// A scan, and a FALSE or zero value (or none).
    Off,
    /// A scan, and TRUE or a value other than zero.
    On,
    /// A link between levels or between POUs.
    Link,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    pub item: Option<Item>,
    pub bounds: Aabb,
    pub role: Role,
    /// A traced scan marks it live: it glows.
    pub lit: bool,
    pub level: Level,
    pub scope: usize,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: V3,
    pub b: V3,
    pub flow: Flow,
    pub level: Level,
    pub scope: usize,
}

impl Seg {
    #[must_use]
    pub const fn lit(&self) -> bool {
        matches!(self.flow, Flow::On)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub solids: Vec<Solid>,
    pub segs: Vec<Seg>,
    /// Per scope: the bottom board with its blocks (what a dive frames).
    pub boards: Vec<Aabb>,
    /// Per scope: its POU node.
    pub pous: Vec<Aabb>,
}

impl Scene {
    /// Everything, for the overview's framing. An empty scene is a small
    /// box at the origin, so a camera can still frame it.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.boards
            .iter()
            .chain(&self.pous)
            .copied()
            .reduce(|a, b| a.union(&b))
            .unwrap_or(Aabb::new([-1.0, 0.0, -1.0], [1.0, 1.0, 1.0]))
    }

    /// The item under `ray`: with `dive` set, only the blocks of that
    /// scope's board (the level the camera is in); else the POU nodes and
    /// every block, the nearest first.
    #[must_use]
    pub fn pick(&self, ray: &Ray, dive: Option<usize>) -> Option<Item> {
        let cands: Vec<(Item, Aabb)> = self
            .solids
            .iter()
            .filter_map(|s| {
                let it = s.item?;
                match (dive, it) {
                    (Some(d), Item::Block(sc, _)) if sc == d => Some((it, s.bounds)),
                    (None, _) => Some((it, s.bounds)),
                    _ => None,
                }
            })
            .collect();
        pick(ray, cands.iter().map(|c| c.1)).map(|i| cands[i].0)
    }

    /// The solid of `item`.
    #[must_use]
    pub fn solid(&self, item: Item) -> Option<&Solid> {
        self.solids.iter().find(|s| s.item == Some(item))
    }
}

fn truthy(v: Value) -> bool {
    match v {
        Value::Bool(b) => b,
        Value::Int(i) => i != 0,
        Value::Real(r) => r != 0.0,
    }
}

const fn role_of(k: &NodeKind) -> Role {
    match k {
        NodeKind::Read { .. } => Role::Read,
        NodeKind::Write { .. } => Role::Write,
        NodeKind::Op { .. } => Role::Op,
        NodeKind::Func { .. } => Role::Func,
        NodeKind::Fb { .. } => Role::Fb,
        NodeKind::Const { .. } => Role::Const,
        _ => Role::Other,
    }
}

/// How many regions are around `r`, itself included (a cycle in the
/// parents cannot make it loop: it stops at the region count).
fn depth(d: &Diagram, r: Option<RegionId>) -> usize {
    d.chain(r).take(d.regions.len() + 1).count()
}

fn num(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// The scene of `scopes`.
#[must_use]
pub fn build(scopes: &[ScopeIn]) -> Scene {
    let mut sc = Scene {
        solids: Vec::new(),
        segs: Vec::new(),
        boards: Vec::new(),
        pous: Vec::new(),
    };
    // Boards side by side along x; a called scope one row further along z.
    let depths: Vec<usize> = (0..scopes.len())
        .map(|i| {
            let (mut n, mut at) = (0, scopes[i].parent);
            while let Some(p) = at.filter(|&p| p < i) {
                n += 1;
                at = scopes[p].parent;
            }
            n
        })
        .collect();
    let row = scopes
        .iter()
        .map(|s| num(s.diagram.size.1) * UNIT)
        .fold(0.0_f32, f32::max)
        + GAP;
    let mut x = 0.0_f32;
    for (i, s) in scopes.iter().enumerate() {
        let d = s.diagram;
        let (w, h) = (num(d.size.0) * UNIT, num(d.size.1) * UNIT);
        let (ox, oz) = (x, depths[i] as f32 * row);
        x += w.max(POU_SIZE[0]) + GAP;
        let at = |px: f32, py: f32| (ox + num(px) * UNIT, oz + num(py) * UNIT);
        let mut board = Aabb::new([ox, -0.12, oz], [ox + w.max(0.5), 0.0, oz + h.max(0.5)]);
        // Regions: slabs under their blocks, outermost first so a nested
        // one sits above its parent.
        for r in &d.regions {
            let lift = (depth(d, r.parent) as f32) * LIFT;
            let (x0, z0) = at(r.rect.x, r.rect.y);
            let (x1, z1) = at(r.rect.right(), r.rect.bottom());
            let b = Aabb::new([x0, lift - 0.04, z0], [x1.max(x0), lift, z1.max(z0)]);
            board = board.union(&b);
            sc.solids.push(Solid {
                item: None,
                bounds: b,
                role: Role::Region,
                lit: s
                    .live
                    .and_then(|l| l.gates.get(r.id.0 as usize))
                    .is_some_and(|g| g.open()),
                level: Level::Bottom,
                scope: i,
                label: r.label.clone(),
            });
        }
        // Blocks.
        let mut tops: Vec<f32> = Vec::with_capacity(d.nodes.len());
        for n in &d.nodes {
            let lift = depth(d, n.region) as f32 * LIFT;
            tops.push(lift + BLOCK_H);
            let (x0, z0) = at(n.rect.x, n.rect.y);
            let (x1, z1) = at(n.rect.right(), n.rect.bottom());
            let b = Aabb::new(
                [x0, lift, z0],
                [x1.max(x0 + 0.05), lift + BLOCK_H, z1.max(z0 + 0.05)],
            );
            board = board.union(&b);
            let lit = s.live.is_some_and(|l| {
                let i = n.id.0 as usize;
                l.ran.get(i).copied().unwrap_or(false)
                    && l.values
                        .get(i)
                        .is_some_and(|o| o.iter().flatten().any(|v| truthy(*v)))
            });
            sc.solids.push(Solid {
                item: Some(Item::Block(i, n.id)),
                bounds: b,
                role: role_of(&n.kind),
                lit,
                level: Level::Bottom,
                scope: i,
                label: n.label.clone(),
            });
        }
        // Wires, as elbows at the height of the blocks' middle.
        for (wi, w) in d.wires.iter().enumerate() {
            let (Some(from), Some(to)) = (d.nodes.get(w.from.0 as usize), d.nodes.get(w.to.0 as usize))
            else {
                continue;
            };
            let y = tops[w.from.0 as usize].min(tops[w.to.0 as usize]) - BLOCK_H / 2.0;
            let (ax, az) = at(out_port(from, w.out).0, out_port(from, w.out).1);
            let (bx, bz) = at(in_port(to, w.inp).0, in_port(to, w.inp).1);
            let mx = f32::midpoint(ax, bx);
            let flow = match s.live.and_then(|l| l.wire(d, wi)) {
                Some(v) if truthy(v) => Flow::On,
                Some(_) => Flow::Off,
                None => Flow::Idle,
            };
            let pts = [[ax, y, az], [mx, y, az], [mx, y, bz], [bx, y, bz]];
            for p in pts.windows(2) {
                if p[0].iter().zip(&p[1]).any(|(a, b)| (a - b).abs() > 1e-6) {
                    sc.segs.push(Seg {
                        a: p[0],
                        b: p[1],
                        flow,
                        level: Level::Bottom,
                        scope: i,
                    });
                }
            }
        }
        sc.solids.push(Solid {
            item: None,
            bounds: Aabb::new(
                [board.min[0], -0.12, board.min[2]],
                [board.max[0], 0.0, board.max[2]],
            ),
            role: Role::Board,
            lit: false,
            level: Level::Bottom,
            scope: i,
            label: String::new(),
        });
        // Its POU node, over the board's middle.
        let c = board.center();
        let p = Aabb::new(
            [c[0] - POU_SIZE[0] / 2.0, MIDDLE_Y, c[2] - POU_SIZE[2] / 2.0],
            [
                c[0] + POU_SIZE[0] / 2.0,
                MIDDLE_Y + POU_SIZE[1],
                c[2] + POU_SIZE[2] / 2.0,
            ],
        );
        let lit = s.live.is_some_and(|l| l.ran.iter().any(|r| *r));
        sc.solids.push(Solid {
            item: Some(Item::Pou(i)),
            bounds: p,
            role: Role::Pou,
            lit,
            level: Level::Middle,
            scope: i,
            label: s.label.clone(),
        });
        // The stem down to the board.
        sc.segs.push(Seg {
            a: [c[0], MIDDLE_Y, c[2]],
            b: [c[0], board.max[1].max(0.0), c[2]],
            flow: Flow::Link,
            level: Level::Middle,
            scope: i,
        });
        sc.boards.push(board);
        sc.pous.push(p);
    }
    // The calls between POUs.
    for (i, s) in scopes.iter().enumerate() {
        if let Some(p) = s.parent.filter(|&p| p < i) {
            let (a, b) = (sc.pous[p].center(), sc.pous[i].center());
            sc.segs.push(Seg {
                a,
                b,
                flow: Flow::Link,
                level: Level::Middle,
                scope: i,
            });
        }
    }
    sc
}
