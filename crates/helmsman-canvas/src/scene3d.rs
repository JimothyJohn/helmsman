//! The 3D canvas's scene, built from `helmsman-diagram`: no GL, no egui.
//!
//! One board per POU (the program, then each user function block
//! instance, as the Viewer's scopes list them): its blocks and wires from
//! the deterministic layout, the diagram's x along the world's x and its y
//! along z (toward the viewer). A board that a block calls stands
//! [`DROP`] lower than its caller's and off to the side, so none hides
//! another, with a thin lit link from the calling block down to it. A
//! region (an IF arm, a loop body) is a slab that lifts what it holds, so
//! nesting gives depth within a board.
//!
//! The program's inputs and outputs, as the simulation declares them
//! ([`Point`]), are not plain blocks: a BOOL input read on the board is a
//! push button, a BOOL output written there a pilot lamp, a numeric input
//! a control and a numeric output a readout ([`classify`]).
//!
//! The top level (the plant and its devices) is a later card
//! (`canvas-3d-devices`).

use helmsman_diagram::layout::{in_port, out_port};
use helmsman_diagram::{Diagram, Live, NodeId, NodeKind, RegionId};
use helmsman_iec::{Ty, Value};

use crate::cam3d::{Aabb, Ray, V3, pick};
use crate::playground::{Dir, Point, Widget};

/// World units per diagram unit.
pub const UNIT: f32 = 1.0 / 26.0;
/// How far a board stands under the one that calls it.
pub const DROP: f32 = 8.0;
/// How tall a block stands on its board.
pub const BLOCK_H: f32 = 0.36;
/// How thick a region's slab is, and how much each nesting level lifts it.
pub const SLAB_H: f32 = 0.12;
pub const LIFT: f32 = 0.34;
/// Space between boards.
pub const GAP: f32 = 7.0;
/// Height of a wire over its block's foot.
pub const WIRE_Y: f32 = 0.03;
/// What is on one scope of the scene.
pub struct ScopeIn<'a> {
    pub label: String,
    pub diagram: &'a Diagram,
    /// The scan on it, if one is traced.
    pub live: Option<&'a Live>,
    /// The scope that calls this one (a function block instance's caller).
    pub parent: Option<usize>,
    /// The block of the caller's diagram that calls it.
    pub via: Option<NodeId>,
    /// The program's inputs and outputs, for the program's own scope (the
    /// rest of the scopes' variables are not the simulation's).
    pub io: &'a [Point],
}

/// What can be picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    /// A scope's board itself.
    Board(usize),
    /// A block of a scope's board.
    Block(usize, NodeId),
}

impl Item {
    #[must_use]
    pub const fn scope(self) -> usize {
        match self {
            Self::Board(s) | Self::Block(s, _) => s,
        }
    }
}

/// What a solid is, for its colour (the renderer reads the theme).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Board,
    Read,
    Write,
    Op,
    Func,
    Fb,
    Const,
    Other,
    Region,
    /// A BOOL input: a push button.
    Button,
    /// A BOOL output: a pilot lamp.
    Lamp,
    /// A numeric input: a control to drag.
    Numeric,
    /// A numeric output: a lit readout.
    Readout,
}

/// The colour a lamp's name says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hue {
    Red,
    Amber,
    Green,
    Blue,
    White,
    /// The name says none: the theme's live colour.
    Live,
}

/// The colour `name` says: a whole word (split at `_`, `-`, space and a
/// lower to upper case change) of red, yellow or amber, green, blue, white.
#[must_use]
pub fn hue_of(name: &str) -> Hue {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || c == '-' || c == ' ' || c == '.' {
            words.push(std::mem::take(&mut cur));
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase();
        cur.push(c.to_ascii_lowercase());
    }
    words.push(cur);
    for w in &words {
        match w.as_str() {
            "red" => return Hue::Red,
            "yellow" | "amber" | "orange" => return Hue::Amber,
            "green" => return Hue::Green,
            "blue" => return Hue::Blue,
            "white" => return Hue::White,
            _ => {}
        }
    }
    Hue::Live
}

/// What a push button, a lamp or a control of the board is called and how
/// it acts.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub scope: usize,
    pub node: NodeId,
    pub name: String,
    pub widget: Widget,
    /// The top of its block: where the pointer finds it.
    pub top: Aabb,
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
    /// A link between boards.
    Link,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Solid {
    pub item: Option<Item>,
    pub bounds: Aabb,
    pub role: Role,
    /// A traced scan marks it live: it glows (a lamp: its output is TRUE;
    /// a button: it is pressed).
    pub lit: bool,
    pub scope: usize,
    pub label: String,
    /// A number that changes when what the block carries does (a flare
    /// at the change, `anim3d`).
    pub sig: u64,
    /// A lamp's colour.
    pub hue: Hue,
    /// How far a numeric control or readout is along its range, 0 to 1.
    pub fill: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: V3,
    pub b: V3,
    pub flow: Flow,
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
    /// Per scope: the board with its blocks (what a dive frames).
    pub boards: Vec<Aabb>,
    /// Per scope: where its diagram's (0, 0) stands on the board (x, z).
    pub origins: Vec<(f32, f32)>,
    /// Per scope, per node (by id): the block's box.
    pub nodes: Vec<Vec<Aabb>>,
    /// Per scope, per region (by id): the slab's box.
    pub regions: Vec<Vec<Aabb>>,
    /// The buttons and controls to press.
    pub controls: Vec<Control>,
}

impl Scene {
    /// Everything, for the overview's framing. An empty scene is a small
    /// box at the origin, so a camera can still frame it.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.boards
            .iter()
            .copied()
            .reduce(|a, b| a.union(&b))
            .unwrap_or(Aabb::new([-1.0, 0.0, -1.0], [1.0, 1.0, 1.0]))
    }

    /// The corners of what is in the scene (of one scope's board, or of
    /// every board): what a framing fits to the view.
    #[must_use]
    pub fn frame_points(&self, scope: Option<usize>) -> Vec<V3> {
        let mut v = Vec::new();
        for (i, b) in self.boards.iter().enumerate() {
            if scope.is_none() || scope == Some(i) {
                v.extend(b.corners());
            }
        }
        v
    }

    /// The item under `ray`: with `dive` set, only the blocks of that
    /// scope's board (the level the camera is in); else the boards and
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

    /// The height of scope `i`'s floor.
    #[must_use]
    pub fn floor(&self, i: usize) -> f32 {
        self.solid(Item::Board(i)).map_or(0.0, |s| s.bounds.max[1])
    }

    /// The solid of `item`.
    #[must_use]
    pub fn solid(&self, item: Item) -> Option<&Solid> {
        self.solids.iter().find(|s| s.item == Some(item))
    }
}

/// How far along its range a point's value is, 0 to 1: a bounded integer
/// by its range, anything else squashed (a REAL or a TIME has none).
#[must_use]
pub fn fill_of(p: &Point) -> f32 {
    let v = match p.value {
        Value::Bool(b) => return f32::from(u8::from(b)),
        Value::Int(i) => i as f64,
        Value::Real(r) => r,
    };
    let f = match p.widget {
        Some(Widget::Int { min, max }) if max > min => (v - min as f64) / (max - min) as f64,
        _ => v.abs() / (1.0 + v.abs()),
    };
    if f.is_finite() {
        f.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

/// The program variable a node's path names, when it names one whole.
fn whole(path: &str) -> Option<&str> {
    (!path.contains(['.', '[']) && !path.is_empty()).then_some(path)
}

/// What a block is to the simulation, by the program's declarations: a
/// read of an input is a control, a write to an output an indicator, and
/// anything else (an internal variable, a member, an array element, a read
/// of an output) stays a block. `io` is empty for a scope that is not the
/// program's.
#[must_use]
pub fn classify<'a>(kind: &NodeKind, io: &'a [Point]) -> Option<(Role, &'a Point)> {
    let (path, want, is_in) = match kind {
        NodeKind::Read { path } => (path, Dir::In, true),
        NodeKind::Write { path } => (path, Dir::Out, false),
        _ => return None,
    };
    let name = whole(path)?;
    let p = io
        .iter()
        .find(|p| p.dir == want && p.name.eq_ignore_ascii_case(name))?;
    let numeric = p.ty != Ty::Bool;
    Some(match (is_in, numeric) {
        (true, false) => (Role::Button, p),
        (true, true) => (Role::Numeric, p),
        (false, false) => (Role::Lamp, p),
        (false, true) => (Role::Readout, p),
    })
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
        origins: Vec::new(),
        nodes: Vec::new(),
        regions: Vec::new(),
        controls: Vec::new(),
    };
    // Boards side by side along x; a called one lower and one row further
    // along z.
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
        let by = -(depths[i] as f32) * DROP;
        let (ox, oz) = (x, depths[i] as f32 * row);
        x += w.max(8.0) + GAP;
        let at = |px: f32, py: f32| (ox + num(px) * UNIT, oz + num(py) * UNIT);
        let mut board = Aabb::new([ox, by - 0.12, oz], [ox + w.max(0.5), by, oz + h.max(0.5)]);
        // Regions: slabs under their blocks; a nested one rides higher.
        let mut slabs = Vec::with_capacity(d.regions.len());
        for r in &d.regions {
            let top = by + depth(d, Some(r.id)) as f32 * LIFT;
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
                scope: i,
                label: r.label.clone(),
                sig: 0,
                hue: Hue::Live,
                fill: 0.0,
            });
        }
        // Blocks, standing on the slab of their region.
        let mut lifts: Vec<f32> = Vec::with_capacity(d.nodes.len());
        let mut boxes: Vec<Aabb> = Vec::with_capacity(d.nodes.len());
        for n in &d.nodes {
            let lift = by + depth(d, n.region) as f32 * LIFT;
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
            let mut lit = s.live.is_some_and(|l| {
                l.ran.get(n.id.0 as usize).copied().unwrap_or(false)
                    && vals.is_some_and(|o| o.iter().flatten().any(|v| truthy(*v)))
            });
            let mut role = role_of(&n.kind);
            let mut sig = vals.map_or(0, |v| signature(v));
            let mut hue = Hue::Live;
            let mut fill = 0.0;
            if let Some((r, p)) = classify(&n.kind, s.io) {
                role = r;
                lit = truthy(p.value);
                sig = signature(&[Some(p.value)]);
                if r == Role::Lamp {
                    hue = hue_of(&p.name);
                }
                if matches!(r, Role::Button | Role::Numeric) {
                    sc.controls.push(Control {
                        scope: i,
                        node: n.id,
                        name: p.name.clone(),
                        widget: p.widget.unwrap_or(Widget::Toggle),
                        top: Aabb::new([b.min[0], b.max[1], b.min[2]], b.max),
                    });
                }
                fill = fill_of(p);
            }
            sc.solids.push(Solid {
                item: Some(Item::Block(i, n.id)),
                bounds: b,
                role,
                lit,
                scope: i,
                label: n.label.clone(),
                sig,
                hue,
                fill,
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
                        scope: i,
                        phase0: run,
                    });
                    run += len;
                }
            }
        }
        sc.solids.push(Solid {
            item: Some(Item::Board(i)),
            bounds: Aabb::new(
                [board.min[0], by - 0.12, board.min[2]],
                [board.max[0], by, board.max[2]],
            ),
            role: Role::Board,
            lit: false,
            scope: i,
            label: String::new(),
            sig: 0,
            hue: Hue::Live,
            fill: 0.0,
        });
        sc.boards.push(board);
        sc.origins.push((ox, oz));
        sc.nodes.push(boxes);
        sc.regions.push(slabs);
    }
    // The calls between boards: a thin link from the calling block down
    // to the board it calls.
    for (i, s) in scopes.iter().enumerate() {
        let Some(p) = s.parent.filter(|&p| p < i) else {
            continue;
        };
        let from = s.via.and_then(|n| sc.nodes[p].get(n.0 as usize)).map_or_else(
            || sc.boards[p].center(),
            |b| [b.center()[0], b.max[1], b.center()[2]],
        );
        let c = sc.boards[i].center();
        let to = [c[0], sc.floor(i) + 0.02, c[2]];
        sc.segs.push(Seg {
            a: from,
            b: to,
            flow: Flow::Link,
            scope: i,
            phase0: 0.0,
        });
    }
    sc
}
