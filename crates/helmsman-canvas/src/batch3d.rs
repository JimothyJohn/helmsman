//! The scene as vertex data for the GL pass, and the theme's colours on
//! it. Plain `Vec<f32>`: no GL here, so the native tests read what the
//! pass is given.
//!
//! Solid vertices: position (3), normal (3), colour (4), emit (1).
//! Line vertices: end A (3), end B (3), corner (2: 0 or 1 along the
//! segment, -1 or 1 across it), colour (4), width in pixels (1), emit (1);
//! six per segment, widened to quads in the vertex shader (GL's own lines
//! are one pixel wide, and WebGL has no wider).

use crate::cam3d::{Aabb, V3};
use crate::scene3d::{Flow, Level, Role, Scene, Seg, Solid};
use crate::st_view::brand::{GREEN, SELECT, STEEL, YELLOW};
use crate::theme::Theme;

pub const SOLID_STRIDE: usize = 11;
pub const LINE_STRIDE: usize = 14;
/// Pixels: a wire, and a block's edge.
pub const WIRE_PX: f32 = 3.0;
pub const EDGE_PX: f32 = 1.5;

/// Where the camera is: what the view dims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The whole stack.
    Overview,
    /// Down in one scope's blocks: the middle level and the other boards
    /// are left as faint ghosts.
    Dive(usize),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Batch {
    pub solids: Vec<f32>,
    pub solids_ghost: Vec<f32>,
    pub lines: Vec<f32>,
    pub lines_ghost: Vec<f32>,
    /// Anything is lit: bloom has something to do.
    pub any_lit: bool,
}

fn rgb(c: eframe::egui::Color32) -> [f32; 3] {
    let [r, g, b, _] = c.to_array();
    [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A solid's colour from the theme (before it glows).
#[must_use]
pub fn solid_color(role: Role, t: &Theme) -> [f32; 3] {
    match role {
        Role::Pou => mix(rgb(t.panel), rgb(YELLOW), 0.35),
        Role::Board => mix(rgb(t.bg), rgb(t.rule), 0.6),
        Role::Read => mix(rgb(t.panel), rgb(SELECT), 0.35),
        Role::Write => mix(rgb(t.panel), rgb(YELLOW), 0.45),
        Role::Op => mix(rgb(t.panel), rgb(t.muted), 0.6),
        Role::Func => mix(rgb(t.panel), rgb(STEEL), 0.7),
        Role::Fb => mix(rgb(t.panel), rgb(YELLOW), 0.2),
        Role::Const => rgb(t.rule),
        Role::Other => rgb(t.shade),
        Role::Region => rgb(t.accent),
    }
}

/// The glow of a lit solid: the lamps' green, yellow for a POU.
#[must_use]
pub fn glow_color(role: Role) -> [f32; 3] {
    if role == Role::Pou {
        rgb(YELLOW)
    } else {
        rgb(GREEN)
    }
}

fn flow_color(f: Flow, t: &Theme) -> [f32; 3] {
    match f {
        Flow::On => rgb(GREEN),
        Flow::Off => rgb(t.muted),
        Flow::Idle => rgb(t.idle),
        Flow::Link => rgb(t.accent),
    }
}

/// The alpha a thing at `level` of `scope` is drawn with, 1 for solid.
fn alpha(level: Level, scope: usize, focus: Focus) -> f32 {
    match focus {
        Focus::Overview => 1.0,
        Focus::Dive(d) if level == Level::Bottom && scope == d => 1.0,
        Focus::Dive(_) if level == Level::Middle => 0.14,
        Focus::Dive(_) => 0.1,
    }
}

const FACES: [([f32; 3], [[usize; 3]; 4]); 6] = [
    // normal, the four corners as (x, y, z) picks of min (0) or max (1)
    ([0.0, 1.0, 0.0], [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]]),
    ([0.0, -1.0, 0.0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]),
    ([0.0, 0.0, 1.0], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]),
    ([0.0, 0.0, -1.0], [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]),
    ([1.0, 0.0, 0.0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]),
    ([-1.0, 0.0, 0.0], [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]]),
];

fn push_box(out: &mut Vec<f32>, b: &Aabb, color: [f32; 4], emit: f32) {
    for (n, quad) in &FACES {
        let p = |c: &[usize; 3]| -> V3 {
            [
                if c[0] == 0 { b.min[0] } else { b.max[0] },
                if c[1] == 0 { b.min[1] } else { b.max[1] },
                if c[2] == 0 { b.min[2] } else { b.max[2] },
            ]
        };
        for k in [0, 1, 2, 0, 2, 3] {
            out.extend_from_slice(&p(&quad[k]));
            out.extend_from_slice(n);
            out.extend_from_slice(&color);
            out.push(emit);
        }
    }
}

fn push_line(out: &mut Vec<f32>, a: V3, b: V3, color: [f32; 4], width: f32, emit: f32) {
    for (t, s) in [
        (0.0, -1.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (0.0, -1.0),
        (1.0, 1.0),
        (1.0, -1.0),
    ] {
        out.extend_from_slice(&a);
        out.extend_from_slice(&b);
        out.extend_from_slice(&[t, s]);
        out.extend_from_slice(&color);
        out.push(width);
        out.push(emit);
    }
}

fn edges(b: &Aabb) -> [(V3, V3); 12] {
    let c = b.corners();
    // Corner index bits: x = 1, y = 2, z = 4.
    let e = |i: usize, j: usize| (c[i], c[j]);
    [
        e(0, 1),
        e(2, 3),
        e(4, 5),
        e(6, 7),
        e(0, 2),
        e(1, 3),
        e(4, 6),
        e(5, 7),
        e(0, 4),
        e(1, 5),
        e(2, 6),
        e(3, 7),
    ]
}

/// The scene's vertices for `focus`, in the colours of `t`.
#[must_use]
pub fn build(scene: &Scene, focus: Focus, t: &Theme) -> Batch {
    let mut b = Batch::default();
    for s in &scene.solids {
        solid(&mut b, s, focus, t);
    }
    for s in &scene.segs {
        seg(&mut b, s, focus, t);
    }
    b
}

fn solid(b: &mut Batch, s: &Solid, focus: Focus, t: &Theme) {
    let a = alpha(s.level, s.scope, focus);
    let base = solid_color(s.role, t);
    let (col, emit) = if s.lit && a >= 1.0 {
        (mix(base, glow_color(s.role), 0.75), 1.0)
    } else {
        (base, 0.0)
    };
    // A region is a veil over its blocks; ghosts blend, the rest do not.
    let translucent = s.role == Role::Region;
    let alpha = if translucent {
        a * if s.lit { 0.3 } else { 0.14 }
    } else {
        a
    };
    let ghost = alpha < 1.0;
    let (sv, lv) = if ghost {
        (&mut b.solids_ghost, &mut b.lines_ghost)
    } else {
        (&mut b.solids, &mut b.lines)
    };
    push_box(sv, &s.bounds, [col[0], col[1], col[2], alpha], emit);
    if s.lit && !ghost {
        b.any_lit = true;
    }
    if s.role != Role::Board && s.role != Role::Region {
        let e = rgb(t.fg);
        let edge = if s.lit { glow_color(s.role) } else { e };
        for (p, q) in edges(&s.bounds) {
            push_line(
                lv,
                p,
                q,
                [edge[0], edge[1], edge[2], if ghost { alpha } else { 0.55 }],
                EDGE_PX,
                emit,
            );
        }
    }
}

fn seg(b: &mut Batch, s: &Seg, focus: Focus, t: &Theme) {
    let a = alpha(s.level, s.scope, focus);
    let c = flow_color(s.flow, t);
    let emit = if s.lit() && a >= 1.0 { 1.0 } else { 0.0 };
    if emit > 0.0 {
        b.any_lit = true;
    }
    let out = if a < 1.0 { &mut b.lines_ghost } else { &mut b.lines };
    push_line(out, s.a, s.b, [c[0], c[1], c[2], a], WIRE_PX, emit);
}
