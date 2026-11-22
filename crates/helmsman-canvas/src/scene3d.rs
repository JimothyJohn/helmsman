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
pub const UNIT: f32 = 1.0 / 26.0;
/// Height of the middle level above the bottom one: the levels stand well
/// apart, so orbiting shows a stack and not a plate with a tag over it.
pub const MIDDLE_Y: f32 = 11.0;
/// How tall a block stands on its board.
pub const BLOCK_H: f32 = 0.8;
/// How thick a region's slab is, and how much each nesting level lifts it.
pub const SLAB_H: f32 = 0.4;
pub const LIFT: f32 = 1.5;
/// Space between boards.
pub const GAP: f32 = 7.0;
/// A POU node's size (x, y, z).
pub const POU_SIZE: V3 = [9.0, 1.1, 4.5];
/// Height of a wire over its block's foot.
pub const WIRE_Y: f32 = 0.45;

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
    /// A thin strut of the stack between a POU node and its board.
    Strut,
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
    /// A number that changes when what the block carries does (a flare
    /// at the change, `anim3d`).
    pub sig: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: V3,
    pub b: V3,
    pub flow: Flow,
    pub level: Level,
    pub scope: usize,
    /// How far along its wire (world units) this piece starts: the
    /// travelling pulses run on across the elbows.
    pub phase0: f32,
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
    /// Per scope: where its diagram's (0, 0) stands on the board (x, z).
    pub origins: Vec<(f32, f32)>,
    /// Per scope, per node (by id): the block's box.
    pub nodes: Vec<Vec<Aabb>>,
    /// Per scope, per region (by id): the slab's box.
    pub regions: Vec<Vec<Aabb>>,
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

    /// The corners of what is in the scene (of one scope's board, or of
    /// every board and POU node): what a framing fits to the view.
    #[must_use]
    pub fn frame_points(&self, scope: Option<usize>) -> Vec<V3> {
        let mut v = Vec::new();
        let all = scope.is_none();
        for (i, b) in self.boards.iter().enumerate() {
            if all || scope == Some(i) {
                v.extend(b.corners());
            }
        }
        if all {
            for p in &self.pous {
                v.extend(p.corners());
            }
        }
        v
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

/// A number for what a node's outputs hold: equal while they are equal.
fn signature(values: &[Option<Value>]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in values {
        let x = match v {
            None => 0,
            Some(Value::Bool(b)) => 1 + u64::from(*b),
            Some(Value::Int(i)) => (*i as u64).wrapping_mul(31).wrapping_add(7),
            Some(Value::Real(r)) => r.to_bits().wrapping_add(13),
        };
        h = (h ^ x).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The scene of `scopes`.
#[must_use]
pub fn build(scopes: &[ScopeIn]) -> Scene {
    let mut sc = Scene {
        solids: Vec::new(),
        segs: Vec::new(),
        boards: Vec::new(),
        pous: Vec::new(),
        origins: Vec::new(),
        nodes: Vec::new(),
        regions: Vec::new(),
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
        // Regions: slabs under their blocks; a nested one rides higher.
        let mut slabs = Vec::with_capacity(d.regions.len());
        for r in &d.regions {
            let top = depth(d, Some(r.id)) as f32 * LIFT;
            let (x0, z0) = at(r.rect.x, r.rect.y);
            let (x1, z1) = at(r.rect.right(), r.rect.bottom());
            let b = Aabb::new([x0, top - SLAB_H, z0], [x1.max(x0), top, z1.max(z0)]);
            board = board.union(&b);
            slabs.push(b);
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
                sig: 0,
            });
        }
        // Blocks, standing on the slab of their region.
        let mut lifts: Vec<f32> = Vec::with_capacity(d.nodes.len());
        let mut boxes: Vec<Aabb> = Vec::with_capacity(d.nodes.len());
        for n in &d.nodes {
            let lift = depth(d, n.region) as f32 * LIFT;
            lifts.push(lift);
            let (x0, z0) = at(n.rect.x, n.rect.y);
            let (x1, z1) = at(n.rect.right(), n.rect.bottom());
            let b = Aabb::new(
                [x0, lift, z0],
                [x1.max(x0 + 0.05), lift + BLOCK_H, z1.max(z0 + 0.05)],
            );
            board = board.union(&b);
            boxes.push(b);
            let vals = s.live.and_then(|l| l.values.get(n.id.0 as usize));
            let lit = s.live.is_some_and(|l| {
                l.ran.get(n.id.0 as usize).copied().unwrap_or(false)
                    && vals.is_some_and(|o| o.iter().flatten().any(|v| truthy(*v)))
            });
            sc.solids.push(Solid {
                item: Some(Item::Block(i, n.id)),
                bounds: b,
                role: role_of(&n.kind),
                lit,
                level: Level::Bottom,
                scope: i,
                label: n.label.clone(),
                sig: vals.map_or(0, |v| signature(v)),
            });
        }
        // Wires: elbows at the height of their blocks' feet, stepping
        // between slabs where they differ.
        for (wi, w) in d.wires.iter().enumerate() {
            let (Some(from), Some(to)) = (d.nodes.get(w.from.0 as usize), d.nodes.get(w.to.0 as usize))
            else {
                continue;
            };
            let (ya, yb) = (lifts[w.from.0 as usize] + WIRE_Y, lifts[w.to.0 as usize] + WIRE_Y);
            let (px, py) = out_port(from, w.out);
            let (qx, qy) = in_port(to, w.inp);
            let (ax, az) = at(px, py);
            let (bx, bz) = at(qx, qy);
            let mx = f32::midpoint(ax, bx);
            let flow = match s.live.and_then(|l| l.wire(d, wi)) {
                Some(v) if truthy(v) => Flow::On,
                Some(_) => Flow::Off,
                None => Flow::Idle,
            };
            let pts = [
                [ax, ya, az],
                [mx, ya, az],
                [mx, ya, bz],
                [mx, yb, bz],
                [bx, yb, bz],
            ];
            let mut run = 0.0_f32;
            for p in pts.windows(2) {
                let len =
                    crate::cam3d::dot(crate::cam3d::sub(p[1], p[0]), crate::cam3d::sub(p[1], p[0])).sqrt();
                if len > 1e-5 {
                    sc.segs.push(Seg {
                        a: p[0],
                        b: p[1],
                        flow,
                        level: Level::Bottom,
                        scope: i,
                        phase0: run,
                    });
                    run += len;
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
            sig: 0,
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
            sig: 0,
        });
        // The way down: a thick link from the node's middle to the board
        // and a strut at each corner of the board.
        let floor = board.max[1].max(0.0);
        sc.segs.push(Seg {
            a: [c[0], MIDDLE_Y, c[2]],
            b: [c[0], floor, c[2]],
            flow: Flow::Link,
            level: Level::Middle,
            scope: i,
            phase0: 0.0,
        });
        for (cx, cz, px, pz) in [
            (board.min[0], board.min[2], p.min[0], p.min[2]),
            (board.max[0], board.min[2], p.max[0], p.min[2]),
            (board.min[0], board.max[2], p.min[0], p.max[2]),
            (board.max[0], board.max[2], p.max[0], p.max[2]),
        ] {
            sc.segs.push(Seg {
                a: [px, MIDDLE_Y, pz],
                b: [cx, floor, cz],
                flow: Flow::Strut,
                level: Level::Middle,
                scope: i,
                phase0: 0.0,
            });
        }
        sc.boards.push(board);
        sc.pous.push(p);
        sc.origins.push((ox, oz));
        sc.nodes.push(boxes);
        sc.regions.push(slabs);
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
                phase0: 0.0,
            });
        }
    }
    sc
}
