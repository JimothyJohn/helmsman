//! The scene as vertex data for the GL pass, and the theme's colours on
//! it. Plain `Vec<f32>`: no GL here, so the native tests read what the
//! pass is given.
//!
//! Colours are linear: the theme's sRGB bytes are decoded here, lit in
//! the shaders, tone mapped and encoded once at the end (`gl3d.rs`).
//!
//! Solid vertices (a box is 36): position (3), normal (3), colour (4),
//! emit (1), the box's centre (3) and half size (3), material (1).
//! Line vertices (six a segment, widened to quads in the vertex shader):
//! end A (3), end B (3), corner (2: 0 or 1 along, -1 or 1 across), colour
//! (4), width in points (1), emit (1), distance along the wire at A (1),
//! whether pulses run on it (1).
//! Pool vertices (the light a lit thing throws on the floor beneath, six
//! a quad): position (3), the quad's coordinates (2, -1 to 1), colour (3).

use crate::anim3d::Glows;
use crate::cam3d::{Aabb, V3};
use crate::scene3d::{Flow, Level, Role, Scene, Seg, Solid};
use crate::st_view::brand::{GREEN, SELECT, STEEL, YELLOW};
use crate::theme::Theme;

pub const SOLID_STRIDE: usize = 18;
pub const LINE_STRIDE: usize = 16;
pub const POOL_STRIDE: usize = 8;
/// Points: a wire, a link between levels, a strut.
pub const WIRE_PX: f32 = 3.2;
pub const LINK_PX: f32 = 5.0;
pub const STRUT_PX: f32 = 1.6;

/// Material numbers the shader reads.
pub const MAT_BLOCK: f32 = 0.0;
pub const MAT_BOARD: f32 = 1.0;
pub const MAT_SLAB: f32 = 2.0;
pub const MAT_POU: f32 = 3.0;

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
    pub pools: Vec<f32>,
    /// Anything glows: bloom has something to do.
    pub any_lit: bool,
}

fn lin(c: eframe::egui::Color32) -> [f32; 3] {
    let [r, g, b, _] = c.to_array();
    [r, g, b].map(|v| (f32::from(v) / 255.0).powf(2.2))
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A solid's colour from the theme (before it glows), linear.
#[must_use]
pub fn solid_color(role: Role, t: &Theme) -> [f32; 3] {
    match role {
        Role::Pou => mix(lin(t.panel), lin(YELLOW), 0.38),
        Role::Board => mix(lin(t.bg), lin(t.rule), 0.55),
        Role::Read => mix(lin(t.panel), lin(SELECT), 0.4),
        Role::Write => mix(lin(t.panel), lin(YELLOW), 0.5),
        Role::Op => mix(lin(t.panel), lin(t.muted), 0.55),
        Role::Func => mix(lin(t.panel), lin(STEEL), 0.6),
        Role::Fb => mix(lin(t.panel), lin(YELLOW), 0.3),
        Role::Const => mix(lin(t.panel), lin(t.rule), 0.7),
        Role::Other => lin(t.shade),
        Role::Region => mix(lin(t.panel), lin(t.accent), 0.5),
    }
}

/// The glow of a lit solid: the lamps' green, yellow for a POU.
#[must_use]
pub fn glow_color(role: Role) -> [f32; 3] {
    if role == Role::Pou {
        lin(YELLOW)
    } else {
        lin(GREEN)
    }
}

fn flow_color(f: Flow, t: &Theme) -> [f32; 3] {
    match f {
        Flow::On => lin(GREEN),
        Flow::Off => lin(t.muted),
        Flow::Idle => lin(t.idle),
        Flow::Link => lin(t.accent),
        Flow::Strut => lin(t.rule),
    }
}

/// The alpha a thing at `level` of `scope` is drawn with, 1 for solid.
fn alpha(level: Level, scope: usize, focus: Focus) -> f32 {
    match focus {
        Focus::Overview => 1.0,
        Focus::Dive(d) if level == Level::Bottom && scope == d => 1.0,
        Focus::Dive(_) if level == Level::Middle => 0.18,
        Focus::Dive(_) => 0.12,
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

fn push_box(out: &mut Vec<f32>, b: &Aabb, color: [f32; 4], emit: f32, mat: f32) {
    let c = b.center();
    let h = crate::cam3d::scale(crate::cam3d::sub(b.max, b.min), 0.5);
    for (n, quad) in &FACES {
        let p = |q: &[usize; 3]| -> V3 {
            [
                if q[0] == 0 { b.min[0] } else { b.max[0] },
                if q[1] == 0 { b.min[1] } else { b.max[1] },
                if q[2] == 0 { b.min[2] } else { b.max[2] },
            ]
        };
        for k in [0, 1, 2, 0, 2, 3] {
            out.extend_from_slice(&p(&quad[k]));
            out.extend_from_slice(n);
            out.extend_from_slice(&color);
            out.push(emit);
            out.extend_from_slice(&c);
            out.extend_from_slice(&h);
            out.push(mat);
        }
    }
}

struct Line {
    a: V3,
    b: V3,
    color: [f32; 4],
    width: f32,
    emit: f32,
    phase0: f32,
    flow: f32,
}

fn push_line(out: &mut Vec<f32>, l: &Line) {
    for (t, s) in [
        (0.0, -1.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (0.0, -1.0),
        (1.0, 1.0),
        (1.0, -1.0),
    ] {
        out.extend_from_slice(&l.a);
        out.extend_from_slice(&l.b);
        out.extend_from_slice(&[t, s]);
        out.extend_from_slice(&l.color);
        out.extend_from_slice(&[l.width, l.emit, l.phase0, l.flow]);
    }
}

/// A flat quad of light on the floor at height `y` over the x and z
/// ranges.
fn push_pool(out: &mut Vec<f32>, x: (f32, f32), z: (f32, f32), y: f32, color: [f32; 3]) {
    for (u, v) in [
        (-1.0, -1.0),
        (1.0, -1.0),
        (1.0, 1.0),
        (-1.0, -1.0),
        (1.0, 1.0),
        (-1.0, 1.0),
    ] {
        let px = if u < 0.0 { x.0 } else { x.1 };
        let pz = if v < 0.0 { z.0 } else { z.1 };
        out.extend_from_slice(&[px, y, pz, u, v]);
        out.extend_from_slice(&color);
    }
}

/// How bright a lit thing is: its eased level and the flare of a change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    pub level: f32,
    pub flare: f32,
}

impl Glow {
    const fn emit(self, base: f32) -> f32 {
        self.level * base + self.flare * 3.0
    }
}

/// The scene's vertices for `focus`, in the colours of `t`, with each
/// thing as bright as `glows` has it at `now` (a scene without them is
/// lit as the scan has it).
#[must_use]
pub fn build(scene: &Scene, focus: Focus, t: &Theme, glows: Option<&Glows>, now: f64) -> Batch {
    let mut b = Batch::default();
    for (k, s) in scene.solids.iter().enumerate() {
        let g = glows.and_then(|g| g.solids.get(k)).map_or(
            Glow {
                level: f32::from(u8::from(s.lit)),
                flare: 0.0,
            },
            |f| Glow {
                level: f.level(now),
                flare: f.flare(now),
            },
        );
        solid(&mut b, s, focus, t, g);
    }
    for (k, s) in scene.segs.iter().enumerate() {
        let lit = s.lit();
        let g = glows.and_then(|g| g.segs.get(k)).map_or(
            Glow {
                level: f32::from(u8::from(lit)),
                flare: 0.0,
            },
            |f| Glow {
                level: f.level(now),
                flare: 0.0 * f.flare(now),
            },
        );
        seg(&mut b, s, focus, t, g);
    }
    b
}

fn solid(b: &mut Batch, s: &Solid, focus: Focus, t: &Theme, g: Glow) {
    let a = alpha(s.level, s.scope, focus);
    let base = solid_color(s.role, t);
    let live = a >= 1.0 && (g.level > 0.01 || g.flare > 0.01);
    let col = if live {
        mix(base, glow_color(s.role), 0.8 * g.level.max(g.flare))
    } else {
        base
    };
    let strength = match s.role {
        Role::Pou => 0.75,
        Role::Region => 0.12,
        _ => 1.6,
    };
    let emit = if live { g.emit(strength) } else { 0.0 };
    let mat = match s.role {
        Role::Board => MAT_BOARD,
        Role::Region => MAT_SLAB,
        Role::Pou => MAT_POU,
        _ => MAT_BLOCK,
    };
    // A slab is a veil over the floor: it blends with what is under it,
    // and so does anything dimmed for the dive.
    let alpha = if s.role == Role::Region { a * 0.3 } else { a };
    let list = if alpha < 1.0 {
        &mut b.solids_ghost
    } else {
        &mut b.solids
    };
    push_box(list, &s.bounds, [col[0], col[1], col[2], alpha], emit, mat);
    if emit > 0.01 {
        b.any_lit = true;
    }
    // The light a lit block throws on what it stands on.
    if live && s.role != Role::Pou && s.role != Role::Region && s.role != Role::Board {
        let k = 0.5 * g.level + 0.9 * g.flare;
        let c = glow_color(s.role).map(|v| v * k);
        let r = 2.0;
        push_pool(
            &mut b.pools,
            (s.bounds.min[0] - r, s.bounds.max[0] + r),
            (s.bounds.min[2] - r, s.bounds.max[2] + r),
            s.bounds.min[1] + 0.02,
            c,
        );
    }
}

fn seg(b: &mut Batch, s: &Seg, focus: Focus, t: &Theme, g: Glow) {
    let a = alpha(s.level, s.scope, focus);
    let c = flow_color(s.flow, t);
    let (width, emit_base) = match s.flow {
        Flow::On | Flow::Off | Flow::Idle => (WIRE_PX, 2.0),
        Flow::Link => (LINK_PX, 0.9),
        Flow::Strut => (STRUT_PX, 0.0),
    };
    let level = if s.flow == Flow::Link { 1.0 } else { g.level };
    let live = a >= 1.0 && matches!(s.flow, Flow::On | Flow::Link);
    let emit = if live { level * emit_base + g.flare } else { 0.0 };
    // A wire that has just gone dark still shows its own colour: the
    // eased level mixes toward the lit one.
    let col = if s.flow == Flow::On || g.level > 0.01 {
        mix(c, lin(GREEN), g.level)
    } else {
        c
    };
    let strut_alpha = if s.flow == Flow::Strut { 0.55 } else { 1.0 };
    let l = Line {
        a: s.a,
        b: s.b,
        color: [col[0], col[1], col[2], a * strut_alpha],
        width,
        emit,
        phase0: s.phase0,
        flow: if s.flow == Flow::On { g.level } else { 0.0 },
    };
    let list = if a < 1.0 { &mut b.lines_ghost } else { &mut b.lines };
    push_line(list, &l);
    if emit > 0.01 && s.flow == Flow::On {
        b.any_lit = true;
    }
    // Its light on the floor, along the horizontal runs.
    if s.flow == Flow::On && g.level > 0.01 && a >= 1.0 {
        let flat = (s.a[0] - s.b[0]).abs() > 1e-4 || (s.a[2] - s.b[2]).abs() > 1e-4;
        if flat {
            let r = 1.0;
            let k = 0.32 * g.level;
            push_pool(
                &mut b.pools,
                (s.a[0].min(s.b[0]) - r, s.a[0].max(s.b[0]) + r),
                (s.a[2].min(s.b[2]) - r, s.a[2].max(s.b[2]) + r),
                s.a[1] - crate::scene3d::WIRE_Y + 0.02,
                lin(GREEN).map(|v| v * k),
            );
        }
    }
}
