//! The 3D view's labels: everything the 2D diagram writes, anchored in
//! the scene, and the placement that keeps them readable at any camera
//! position (`labels3d_tests.rs`). No egui, no GL.
//!
//! The set ([`labels`]) is what 2D draws: a block's label, its port
//! names, a variable's value under its header, a non-BOOL wire's value
//! at its start, a region's title and its state. Placement ([`place`])
//! only fades, shrinks to a readable minimum, ellipsizes and culls: the
//! nearest label is placed first, then by kind, and one that would sit on
//! another already placed is left out.

use std::collections::HashMap;

use helmsman_diagram::NodeKind;
use helmsman_diagram::layout::{HEADER, LABEL_H, in_port, out_port};
use helmsman_diagram::lower::Var;
use helmsman_iec::Value;

use crate::cam3d::{Camera, FAR, FOV_Y, NEAR, V3};
use crate::scene3d::{Item, Role, Scene, ScopeIn, UNIT, WIRE_Y};
use crate::st_view::{gate_state, value_text};

/// What a label is, which is also how much it matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Title,
    Region,
    Node,
    Value,
    RegionState,
    WireValue,
    Port,
}

/// The colour a label takes from the theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Muted,
    True,
    False,
    Accent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub at: V3,
    pub text: String,
    pub kind: Kind,
    pub tone: Tone,
    pub align: Align,
    pub scope: usize,
    /// Room the label has before it runs off what it names, world units.
    pub width: f32,
    /// The type size in diagram units, as 2D sets it.
    pub size: f32,
    pub item: Option<Item>,
}

impl Label {
    fn new(at: V3, text: impl Into<String>, kind: Kind, scope: usize, width: f32, size: f32) -> Self {
        Self {
            at,
            text: text.into(),
            kind,
            tone: if kind == Kind::Node {
                Tone::Normal
            } else {
                Tone::Muted
            },
            align: Align::Center,
            scope,
            width,
            size,
            item: None,
        }
    }
}

/// Every label 2D draws for `scopes`, anchored on `scene`; `vars` is each
/// scope's POU's variables (a value's unit reads from them).
#[must_use]
pub fn labels(scopes: &[ScopeIn], vars: &[Option<&HashMap<String, Var>>], scene: &Scene) -> Vec<Label> {
    let mut out = Vec::new();
    // What each block is to the simulation, so a button's or a lamp's name
    // goes on the board in front of it and not over its cap.
    let special: std::collections::HashSet<(usize, u32)> = scene
        .solids
        .iter()
        .filter(|s| matches!(s.role, Role::Button | Role::Numeric))
        .filter_map(|s| match s.item {
            Some(Item::Block(sc, n)) => Some((sc, n.0)),
            _ => None,
        })
        .collect();
    for (i, s) in scopes.iter().enumerate() {
        let (Some(board), Some(&(ox, oz))) = (scene.boards.get(i), scene.origins.get(i)) else {
            continue;
        };
        let d = s.diagram;
        let at = |x: f32, y: f32| (ox + x * UNIT, oz + y * UNIT);
        let v = vars.get(i).copied().flatten();
        // Each board of several is titled at its front edge; one alone is
        // named by the pane's header.
        if scopes.len() > 1 {
            let mut t = Label::new(
                [board.min[0], board.max[1].min(scene.floor(i)), board.max[2] + 0.9],
                s.label.clone(),
                Kind::Title,
                i,
                board.max[0] - board.min[0],
                16.0,
            );
            t.align = Align::Left;
            t.tone = Tone::Accent;
            t.item = Some(Item::Board(i));
            out.push(t);
        }
        for r in &d.regions {
            let Some(b) = scene.regions[i].get(r.id.0 as usize) else {
                continue;
            };
            let top = b.max[1];
            let (x0, z0) = at(r.rect.x + 8.0, r.rect.y + LABEL_H / 2.0);
            let mut l = Label::new(
                [x0, top, z0],
                r.label.clone(),
                Kind::Region,
                i,
                (b.max[0] - b.min[0]) * 0.7,
                12.0,
            );
            l.align = Align::Left;
            l.tone = Tone::Accent;
            out.push(l);
            let state = gate_state(s.live.and_then(|l| l.gates.get(r.id.0 as usize).copied()));
            if !state.is_empty() {
                let (x1, z1) = at(r.rect.right() - 8.0, r.rect.y + LABEL_H / 2.0);
                let mut l = Label::new(
                    [x1, top, z1],
                    state,
                    Kind::RegionState,
                    i,
                    (b.max[0] - b.min[0]) * 0.3,
                    10.0,
                );
                l.align = Align::Right;
                out.push(l);
            }
        }
        for n in &d.nodes {
            let Some(b) = scene.nodes[i].get(n.id.0 as usize) else {
                continue;
            };
            let top = b.max[1];
            let wide = b.max[0] - b.min[0];
            let headed = n.kind.headed();
            let (cx, cz) = if headed {
                at(n.rect.x + n.rect.w / 2.0, n.rect.y + HEADER / 2.0)
            } else {
                at(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0)
            };
            let front = special.contains(&(i, n.id.0));
            let (cz, name_w) = if front {
                (b.max[2] - crate::look3d::INSET - 0.3, wide)
            } else {
                (cz, wide)
            };
            // Names in normal case, as the 2D tunnel tags write them;
            // operators and functions keep their capitals.
            let text = if matches!(n.kind, NodeKind::Read { .. } | NodeKind::Write { .. }) {
                n.label.to_ascii_lowercase()
            } else {
                n.label.clone()
            };
            let mut l = Label::new([cx, top, cz], text, Kind::Node, i, name_w, 11.5);
            l.item = Some(Item::Block(i, n.id));
            if front {
                // The name ends at the middle, the state starts there:
                // one line in front of the tile.
                l.align = Align::Right;
                l.at[0] -= 0.12;
            }
            out.push(l);
            if front {
                // The state under the name; no port names on these.
                if let Some(val) = s
                    .live
                    .and_then(|l| l.values.get(n.id.0 as usize)?.first().copied().flatten())
                    && let Some(text) = value_text(d, n.id, val, v)
                {
                    let mut vl = Label::new([cx + 0.12, top, cz], text, Kind::Value, i, wide, 10.0);
                    vl.align = Align::Left;
                    vl.tone = match val {
                        Value::Bool(true) => Tone::True,
                        Value::Bool(false) => Tone::False,
                        _ => Tone::Normal,
                    };
                    out.push(vl);
                }
                continue;
            }
            for (k, name) in n.ins.iter().enumerate().filter(|(_, n)| !n.is_empty()) {
                let (px, py) = in_port(n, k);
                let (x, z) = at(px + 6.0, py);
                let mut l = Label::new([x, top, z], name.clone(), Kind::Port, i, wide * 0.5, 10.0);
                l.align = Align::Left;
                out.push(l);
            }
            for (k, name) in n.outs.iter().enumerate().filter(|(_, n)| !n.is_empty()) {
                let (px, py) = out_port(n, k);
                let (x, z) = at(px - 6.0, py);
                let mut l = Label::new([x, top, z], name.clone(), Kind::Port, i, wide * 0.5, 10.0);
                l.align = Align::Right;
                out.push(l);
            }
            if matches!(n.kind, NodeKind::Read { .. } | NodeKind::Write { .. })
                && let Some(val) = s
                    .live
                    .and_then(|l| l.values.get(n.id.0 as usize)?.first().copied().flatten())
                && let Some(text) = value_text(d, n.id, val, v)
            {
                let (x, z) = at(
                    n.rect.x + n.rect.w / 2.0,
                    n.rect.y + HEADER + (n.rect.h - HEADER) / 2.0,
                );
                let mut l = Label::new([x, top, z], text, Kind::Value, i, wide, 11.0);
                l.tone = match val {
                    Value::Bool(true) => Tone::True,
                    Value::Bool(false) => Tone::False,
                    _ => Tone::Normal,
                };
                out.push(l);
            }
        }
        // The panel's indicators: name and state beside each lamp.
        for ind in scene.indicators.iter().filter(|x| x.scope == i) {
            let Some(p) = s.io.iter().find(|p| p.name == ind.name) else {
                continue;
            };
            let top = ind.cell.max[1];
            let x = ind.cell.max[0] + 0.45;
            let zc = f32::midpoint(ind.cell.min[2], ind.cell.max[2]);
            let mut name = Label::new(
                [x, top, zc - 0.32],
                p.name.to_ascii_lowercase(),
                Kind::Node,
                i,
                crate::scene3d::LABEL_W - 0.6,
                12.0,
            );
            name.align = Align::Left;
            name.item = Some(Item::Indicator(i, ind.order));
            out.push(name);
            let text = ind
                .writers
                .first()
                .and_then(|w| value_text(d, *w, p.value, v))
                .unwrap_or_else(|| p.value.to_string());
            let mut val = Label::new(
                [x, top, zc + 0.34],
                text,
                Kind::Value,
                i,
                crate::scene3d::LABEL_W - 0.6,
                10.5,
            );
            val.align = Align::Left;
            val.tone = match p.value {
                Value::Bool(true) => Tone::True,
                Value::Bool(false) => Tone::False,
                _ => Tone::Normal,
            };
            out.push(val);
        }
        for (k, w) in d.wires.iter().enumerate() {
            let Some(val) = s.live.and_then(|l| l.wire(d, k)) else {
                continue;
            };
            if matches!(val, Value::Bool(_)) {
                continue;
            }
            let Some(text) = value_text(d, w.from, val, v) else {
                continue;
            };
            let (Some(a), Some(from)) = (
                d.nodes.get(w.from.0 as usize),
                scene.nodes[i].get(w.from.0 as usize),
            ) else {
                continue;
            };
            let (px, py) = out_port(a, w.out);
            let (x, z) = at(px + 4.0, py - 7.0);
            let mut l = Label::new(
                [x, from.min[1] + WIRE_Y, z],
                text,
                Kind::WireValue,
                i,
                120.0 * UNIT,
                10.0,
            );
            l.align = Align::Left;
            l.tone = Tone::Normal;
            out.push(l);
        }
    }
    out
}

/// How to place labels on a screen.
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// The smallest and largest type, in points.
    pub font: (f32, f32),
    /// The scope the view is dived into: the others' blocks are left out.
    pub focus: Option<usize>,
    /// About how far the scene is seen from, to fade the far ones.
    pub reach: f32,
    /// Port names, wire values and arm states are left out (a low angle
    /// stacks them).
    pub hide_detail: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            font: (9.0, 17.0),
            focus: None,
            reach: 30.0,
            hide_detail: false,
        }
    }
}

/// A label on the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Index in the slice given to [`place`].
    pub index: usize,
    /// Left, top, right, bottom, in points from the viewport's corner.
    pub rect: [f32; 4],
    pub font: f32,
    pub text: String,
    /// 1 near, down to about a half far away.
    pub alpha: f32,
}

struct Cand {
    i: usize,
    x: f32,
    y: f32,
    z: f32,
    font: f32,
    text: String,
    alpha: f32,
}

/// The proportional face's size against the mono's for the same room (it
/// is narrower).
pub const PROPORTIONAL: f32 = 1.0;

/// A monospace advance, as a fraction of the type size.
pub const ADVANCE: f32 = 0.6;

fn overlaps(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[0] < b[2] + 2.0 && b[0] < a[2] + 2.0 && a[1] < b[3] + 1.0 && b[1] < a[3] + 1.0
}

/// The labels that fit on screen, in the order they were placed.
#[must_use]
pub fn place(labels: &[Label], cam: &Camera, o: &Opts) -> Vec<Placed> {
    if !cam.drawable() {
        return Vec::new();
    }
    let th = (FOV_Y / 2.0).tan();
    let mut cands: Vec<Cand> = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        if o.focus.is_some_and(|f| l.scope != f && l.kind != Kind::Title) {
            continue;
        }
        if o.hide_detail && matches!(l.kind, Kind::Port | Kind::WireValue | Kind::RegionState) {
            continue;
        }
        let Some((x, y, d)) = cam.project(l.at) else {
            continue;
        };
        let z = d * (FAR - NEAR) + NEAR;
        // Points on the screen of one world unit at this depth.
        let per_unit = cam.size.1 / (2.0 * th * z);
        let raw = l.size * UNIT * per_unit;
        // What is too small to read collapses; a block's name and a
        // title never do: they are held at the smallest type.
        let holds = matches!(l.kind, Kind::Title | Kind::Region | Kind::Node);
        if !holds && raw < o.font.0 + 0.5 {
            continue;
        }
        let font = raw.clamp(o.font.0, o.font.1);
        let room = l.width * per_unit;
        let fit = (room / (font * ADVANCE)).floor();
        let chars = l.text.chars().count();
        let text = if (chars as f32) <= fit || l.kind == Kind::Title || l.kind == Kind::Region {
            l.text.clone()
        } else if fit >= 3.0 || l.kind == Kind::Node {
            // A block's name is cut to its block but never dropped: at
            // the least two letters and the ellipsis.
            let keep = (fit as usize).saturating_sub(1).max(2);
            l.text.chars().take(keep).chain(['…']).collect()
        } else {
            continue;
        };
        let mut alpha = (1.3 - 0.6 * z / o.reach.max(1.0)).clamp(0.45, 1.0);
        if o.focus.is_some_and(|f| l.scope != f) {
            alpha *= 0.5;
        }
        cands.push(Cand {
            i,
            x,
            y,
            z,
            font,
            text,
            alpha,
        });
    }
    // The nearest first whatever it is, then by what matters.
    let nearest = cands
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.z.total_cmp(&b.1.z))
        .map(|(k, _)| k);
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| {
        let key = |k: usize| (Some(k) != nearest, labels[cands[k].i].kind);
        key(a).cmp(&key(b)).then(cands[a].z.total_cmp(&cands[b].z))
    });
    let mut placed: Vec<Placed> = Vec::new();
    for k in order {
        let c = &cands[k];
        let l = &labels[c.i];
        let w = c.text.chars().count() as f32 * c.font * ADVANCE + 4.0;
        let h = c.font * 1.1 + 1.0;
        let x0 = match l.align {
            Align::Left => c.x - 2.0,
            Align::Center => c.x - w / 2.0,
            Align::Right => c.x - w + 2.0,
        };
        let rect = [x0, c.y - h / 2.0, x0 + w, c.y + h / 2.0];
        if rect[2] < 0.0 || rect[3] < 0.0 || rect[0] > cam.size.0 || rect[1] > cam.size.1 {
            continue;
        }
        // A name that would sit on another is nudged up or down a little
        // before it is given up; the rest are left out.
        let nudge = matches!(l.kind, Kind::Node | Kind::Value | Kind::Region);
        let tries: &[f32] = if nudge {
            &[0.0, 0.6, -0.6, 1.2, -1.2]
        } else {
            &[0.0]
        };
        let Some(rect) = tries
            .iter()
            .map(|k| {
                let dy = k * h;
                [rect[0], rect[1] + dy, rect[2], rect[3] + dy]
            })
            .find(|r| !placed.iter().any(|p| overlaps(&p.rect, r)))
        else {
            continue;
        };
        placed.push(Placed {
            index: c.i,
            rect,
            font: c.font,
            text: c.text.clone(),
            alpha: c.alpha,
        });
    }
    placed
}
