//! The scene as vertex data for the GL pass: no GL here, so the native
//! tests read what the pass is given.
//!
//! One path for every renderer: flat shapes drawn far to near (no depth
//! buffer, no offscreen target). A tile is a few rounded-rectangle layers
//! stacked up its side; the fragment shader rounds and antialiases them
//! with a signed distance, so edges are smooth without MSAA. Shadows are
//! analytic blurred rounded rectangles under each tile; lamps and live
//! wires add a soft glow on top. [`crate::look3d::Look`] holds every taste
//! decision.
//!
//! Shape vertices (six a quad): position (3), local coordinates (2: world
//! units from the shape's centre, or -1 to 1 on a sprite), half size (2),
//! corner radius (1), colour (4, straight alpha), parameters (4: mode,
//! top layer, live, extra).
//! Line vertices (six a segment, widened to quads in the vertex shader):
//! end A (3), end B (3), corner (2), colour (4), width in points (1),
//! emit (1), distance along the wire at A (1), whether pulses run (1).

use crate::anim3d::Glows;
use crate::cam3d::{Aabb, V3, add, dot, scale, sub};
use crate::look3d::Look;
use crate::scene3d::{Flow, Role, Scene, Seg, Solid};

pub const SHAPE_STRIDE: usize = 16;
pub const LINE_STRIDE: usize = 16;

/// What a shape is, in the first parameter.
pub const M_TILE: f32 = 0.0;
pub const M_SHADOW: f32 = 1.0;
pub const M_HALO: f32 = 2.0;
pub const M_SPHERE: f32 = 3.0;
pub const M_FLOOR: f32 = 4.0;
pub const M_POOL: f32 = 5.0;

/// Where the camera is: what the view dims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Every board.
    Overview,
    /// Down in one scope's board: the others are left as faint ghosts.
    Dive(usize),
}

/// The camera's frame, for ordering and for sprites that face it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rig {
    pub eye: V3,
    pub right: V3,
    pub up: V3,
    pub fwd: V3,
}

impl Rig {
    #[must_use]
    pub fn of(pose: &crate::cam3d::Pose) -> Self {
        let (right, up, fwd) = pose.basis();
        Self {
            eye: pose.eye,
            right,
            up,
            fwd,
        }
    }

    fn depth(&self, p: V3) -> f32 {
        dot(sub(p, self.eye), self.fwd)
    }
}

/// One board's shapes, in the order they are drawn: what lies on the
/// floor, the wires, then the tiles far to near.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardBatch {
    pub under: Vec<f32>,
    pub lines: Vec<f32>,
    pub over: Vec<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Batch {
    /// Far to near.
    pub boards: Vec<BoardBatch>,
    /// Added on top of everything: halos and the glow of live wires.
    pub glow: Vec<f32>,
    pub glow_lines: Vec<f32>,
    /// The links between boards, over all.
    pub links: Vec<f32>,
    /// Anything glows: the frame wants to keep moving.
    pub any_lit: bool,
}

#[allow(clippy::too_many_arguments)]
fn vert(out: &mut Vec<f32>, p: V3, uv: [f32; 2], half: [f32; 2], rad: f32, col: [f32; 4], par: [f32; 4]) {
    out.extend_from_slice(&p);
    out.extend_from_slice(&uv);
    out.extend_from_slice(&half);
    out.push(rad);
    out.extend_from_slice(&col);
    out.extend_from_slice(&par);
}

/// A rounded rectangle lying flat at `c`, `half` the size of its body and
/// `pad` more all round for what blurs past it.
fn flat(out: &mut Vec<f32>, c: V3, half: [f32; 2], pad: f32, rad: f32, col: [f32; 4], par: [f32; 4]) {
    let (hx, hz) = (half[0] + pad, half[1] + pad);
    let corner = |sx: f32, sz: f32| ([c[0] + sx * hx, c[1], c[2] + sz * hz], [sx * hx, sz * hz]);
    for (sx, sz) in [
        (-1.0, -1.0),
        (1.0, -1.0),
        (1.0, 1.0),
        (-1.0, -1.0),
        (1.0, 1.0),
        (-1.0, 1.0),
    ] {
        let (p, uv) = corner(sx, sz);
        vert(out, p, uv, half, rad, col, par);
    }
}

/// A square sprite of half size `r` at `c`, facing the camera.
fn sprite(out: &mut Vec<f32>, c: V3, r: f32, rig: &Rig, col: [f32; 4], par: [f32; 4]) {
    for (sx, sy) in [
        (-1.0, -1.0),
        (1.0, -1.0),
        (1.0, 1.0),
        (-1.0, -1.0),
        (1.0, 1.0),
        (-1.0, 1.0),
    ] {
        let p = add(c, add(scale(rig.right, sx * r), scale(rig.up, sy * r)));
        vert(out, p, [sx, sy], [1.0, 1.0], 0.0, col, par);
    }
}

fn with_alpha(c: [f32; 3], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], a]
}

fn alpha(scope: usize, focus: Focus) -> f32 {
    match focus {
        Focus::Dive(d) if scope != d => 0.14,
        _ => 1.0,
    }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// How bright a lit thing is: its eased level and the flare of a change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    pub level: f32,
    pub flare: f32,
}

impl Glow {
    fn live(self) -> f32 {
        (self.level + 0.5 * self.flare).clamp(0.0, 1.0)
    }
}

/// The footprint of a box: its centre on the floor and half sizes, drawn
/// inside the box by the look's inset.
fn footprint(b: &Aabb, inset: f32) -> (V3, [f32; 2]) {
    let c = b.center();
    let hx = ((b.max[0] - b.min[0]) / 2.0 - inset).max(0.06);
    let hz = ((b.max[2] - b.min[2]) / 2.0 - inset).max(0.06);
    ([c[0], b.min[1], c[2]], [hx, hz])
}

/// A tile: `look.layers` rounded layers up its side from `y0`, and its
/// shadow into `under`.
#[allow(clippy::too_many_arguments)]
fn tile(
    under: &mut Vec<f32>,
    over: &mut Vec<f32>,
    look: &Look,
    base: V3,
    half: [f32; 2],
    height: f32,
    rad: f32,
    color: [f32; 3],
    a: f32,
    live: f32,
    gloss: f32,
) {
    let sigma = look.shadow_sigma * (0.6 + height);
    flat(
        under,
        [base[0] + 0.05, base[1] + 0.004, base[2] + 0.12],
        half,
        sigma * 3.0,
        rad,
        [0.0, 0.0, 0.0, a],
        [M_SHADOW, sigma, look.shadow_opacity * a, 0.0],
    );
    let n = look.layers.max(1);
    for k in 1..=n {
        let top = k == n;
        let y = base[1] + height * k as f32 / n as f32;
        let shade = if top { 1.0 } else { look.side_shade(k) };
        let c = [color[0] * shade, color[1] * shade, color[2] * shade];
        flat(
            over,
            [base[0], y, base[2]],
            half,
            0.0,
            rad,
            with_alpha(c, a),
            [
                M_TILE,
                f32::from(u8::from(top)),
                if top { live } else { 0.0 },
                if top { gloss } else { 0.0 },
            ],
        );
    }
}

/// The scene's shapes for `focus`, with each thing as bright as `glows`
/// has it at `now` (a scene without them is lit as the scan has it).
#[must_use]
pub fn build(scene: &Scene, focus: Focus, look: &Look, rig: &Rig, glows: Option<&Glows>, now: f64) -> Batch {
    let mut batch = Batch::default();
    let glow_of = |k: usize, lit: bool| {
        glows.and_then(|g| g.solids.get(k)).map_or(
            Glow {
                level: f32::from(u8::from(lit)),
                flare: 0.0,
            },
            |f| Glow {
                level: f.level(now),
                flare: f.flare(now),
            },
        )
    };
    let mut order: Vec<(f32, BoardBatch)> = Vec::new();
    for (i, board) in scene.boards.iter().enumerate() {
        let a = alpha(i, focus);
        let mut bb = BoardBatch::default();
        // The floor: a clean matte surface with a faint dot grid.
        let c = board.center();
        let (hx, hz) = (
            (board.max[0] - board.min[0]) / 2.0 + 0.5,
            (board.max[2] - board.min[2]) / 2.0 + 0.5,
        );
        let floor_y = scene.floor(i);
        flat(
            &mut bb.under,
            [c[0], floor_y, c[2]],
            [hx, hz],
            0.0,
            0.8,
            with_alpha(look.floor, a),
            [M_FLOOR, 0.0, 0.0, if look.dark { 0.55 } else { 0.7 }],
        );
        // Regions: slightly raised, faintly tinted areas.
        for (k, s) in scene
            .solids
            .iter()
            .enumerate()
            .filter(|(_, s)| s.scope == i && matches!(s.role, Role::Region | Role::Panel))
        {
            let g = glow_of(k, s.lit);
            let (base, half) = footprint(&s.bounds, 0.0);
            let (mut shadow, mut layers) = (Vec::new(), Vec::new());
            tile(
                &mut shadow,
                &mut layers,
                look,
                base,
                half,
                s.bounds.max[1] - s.bounds.min[1],
                0.5,
                look.tile_color(s.role),
                if s.role == Role::Panel { a } else { a * 0.9 },
                g.live() * 0.5,
                0.0,
            );
            bb.under.extend(shadow);
            bb.under.extend(layers);
        }
        // The tiles, each with its own order key.
        let mut items: Vec<(f32, Vec<f32>)> = Vec::new();
        let mut shadows: Vec<f32> = Vec::new();
        for (k, s) in scene.solids.iter().enumerate().filter(|(_, s)| s.scope == i) {
            if matches!(s.role, Role::Board | Role::Region | Role::Panel) {
                continue;
            }
            let g = glow_of(k, s.lit);
            let mut v = Vec::new();
            block(&mut v, &mut shadows, &mut batch.glow, look, rig, s, a, g);
            if g.live() > 0.01 {
                batch.any_lit = true;
            }
            items.push((rig.depth(s.bounds.center()), v));
        }
        bb.under.extend(shadows);
        items.sort_by(|x, y| y.0.total_cmp(&x.0));
        for (_, v) in items {
            bb.over.extend(v);
        }
        order.push((rig.depth(board.center()), bb));
    }
    // The wires.
    let mut by_scope: Vec<Vec<usize>> = vec![Vec::new(); scene.boards.len()];
    for (k, s) in scene.segs.iter().enumerate() {
        if s.flow != Flow::Link
            && let Some(v) = by_scope.get_mut(s.scope)
        {
            v.push(k);
        }
    }
    for (i, ks) in by_scope.iter().enumerate() {
        let a = alpha(i, focus);
        for &k in ks {
            let s = &scene.segs[k];
            let g = glows.and_then(|g| g.segs.get(k)).map_or(
                Glow {
                    level: f32::from(u8::from(s.lit())),
                    flare: 0.0,
                },
                |f| Glow {
                    level: f.level(now),
                    flare: 0.0,
                },
            );
            wire(&mut order[i].1.lines, &mut batch.glow_lines, look, s, a, g);
            if s.lit() {
                batch.any_lit = true;
            }
        }
    }
    for s in scene.segs.iter().filter(|s| s.flow == Flow::Link) {
        let a = alpha(s.scope, focus) * 0.8;
        push_line(
            &mut batch.links,
            &Line {
                a: s.a,
                b: s.b,
                color: with_alpha(look.accent, a),
                width: look.link_px,
                emit: 0.0,
                phase0: 0.0,
                flow: 0.0,
            },
        );
    }
    order.sort_by(|x, y| y.0.total_cmp(&x.0));
    batch.boards = order.into_iter().map(|(_, b)| b).collect();
    batch
}

/// One block as a tile, a button, a lamp or a readout.
#[allow(clippy::too_many_arguments)]
fn block(
    over: &mut Vec<f32>,
    under: &mut Vec<f32>,
    glow: &mut Vec<f32>,
    look: &Look,
    rig: &Rig,
    s: &Solid,
    a: f32,
    g: Glow,
) {
    let (base, half) = footprint(&s.bounds, look.inset);
    let h = look.tile_h.min(s.bounds.max[1] - s.bounds.min[1]);
    let color = look.tile_color(s.role);
    let (w, d) = (half[0] * 2.0, half[1] * 2.0);
    match s.role {
        Role::Button => {
            tile(
                under,
                over,
                look,
                base,
                half,
                h * 0.6,
                look.radius,
                color,
                a,
                0.0,
                0.0,
            );
            // The cap sits toward the back of the tile; its name and state
            // run along the front strip.
            let r = (0.36 * w.min(d))
                .clamp(0.2, 0.8)
                .min(((d - 0.6) / 2.0).max(0.15) / 1.18);
            let (cx, cz) = (base[0], base[2] - half[1] + r * 1.18 + 0.08);
            let y = base[1] + h * 0.6;
            let neutral = if look.dark {
                [0.5, 0.52, 0.56]
            } else {
                [0.93, 0.93, 0.95]
            };
            let pressed = mix(neutral, look.accent, 0.6 * g.level);
            // The cap sinks while it is pressed.
            let rise = 0.2 - 0.09 * g.level;
            tile(
                under,
                over,
                look,
                [cx, y, cz],
                [r * 1.18, r * 1.18],
                0.04,
                r * 1.18,
                mix(color, [0.0; 3], 0.2),
                a,
                0.0,
                0.0,
            );
            let mut cap = Vec::new();
            tile(
                over,
                &mut cap,
                look,
                [cx, y + 0.04, cz],
                [r, r],
                rise,
                r,
                pressed,
                a,
                g.live(),
                0.9,
            );
            over.extend(cap);
            if g.live() > 0.01 && a >= 1.0 {
                flat(
                    glow,
                    [cx, y + 0.01, cz],
                    [r * 1.4, r * 1.4],
                    r,
                    r * 1.4,
                    with_alpha(look.accent, 1.0),
                    [M_POOL, 0.0, 0.25 * look.glow * g.live(), 0.0],
                );
            }
        }
        Role::Lamp => {
            // The panel is the plate: a dark socket and the glass on it.
            let r = (0.42 * w.min(d)).clamp(0.3, 0.72);
            let (cx, cz) = (base[0], base[2]);
            let sock = mix(color, [0.0; 3], 0.45);
            let mut ring = Vec::new();
            tile(
                under,
                &mut ring,
                look,
                [cx, base[1], cz],
                [r * 1.22, r * 1.22],
                0.06,
                r * 1.22,
                sock,
                a,
                0.0,
                0.0,
            );
            over.extend(ring);
            let c = [cx, base[1] + 0.06 + r * 0.5, cz];
            let hue = Look::hue(s.hue);
            sprite(
                over,
                c,
                r * 1.12,
                rig,
                with_alpha(hue, a),
                [M_SPHERE, 0.0, g.live(), 0.0],
            );
            if g.live() > 0.01 && a >= 1.0 {
                sprite(
                    glow,
                    c,
                    r * 3.6,
                    rig,
                    with_alpha(hue, 1.0),
                    [M_HALO, 0.0, 0.55 * look.glow * g.live(), 0.0],
                );
                flat(
                    glow,
                    [cx, base[1] + 0.07, cz],
                    [r * 2.0, r * 2.0],
                    r * 2.0,
                    r * 2.0,
                    with_alpha(hue, 1.0),
                    [M_POOL, 0.0, 0.3 * look.glow * g.live(), 0.0],
                );
            }
        }
        Role::Numeric | Role::Readout => {
            tile(under, over, look, base, half, h, look.radius, color, a, 0.0, 0.0);
            // A track along the tile with how far the value is along it.
            let tw = (half[0] * 1.5).max(0.1);
            let ty = base[1] + h;
            let track = if look.dark {
                [0.08, 0.08, 0.09]
            } else {
                [0.82, 0.83, 0.86]
            };
            let z = base[2] - half[1] * 0.45;
            flat(
                over,
                [base[0], ty + 0.004, z],
                [tw, 0.06],
                0.0,
                0.06,
                with_alpha(track, a),
                [M_TILE, 1.0, 0.0, 0.0],
            );
            let f = s.fill.clamp(0.0, 1.0);
            if f > 0.01 {
                flat(
                    over,
                    [base[0] - tw + tw * f, ty + 0.008, z],
                    [tw * f, 0.06],
                    0.0,
                    0.06,
                    with_alpha(look.accent, a),
                    [M_TILE, 1.0, 0.0, 0.0],
                );
            }
            if s.role == Role::Numeric {
                let knob = look.accent;
                let mut k = Vec::new();
                tile(
                    over,
                    &mut k,
                    look,
                    [base[0] - tw + 2.0 * tw * f, ty + 0.01, z],
                    [0.14, 0.14],
                    0.06,
                    0.14,
                    mix(knob, [1.0; 3], 0.4),
                    a,
                    0.0,
                    0.6,
                );
                over.extend(k);
            }
        }
        _ => {
            tile(
                under,
                over,
                look,
                base,
                half,
                h,
                look.radius,
                color,
                a,
                g.live(),
                0.0,
            );
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

/// A wire: dim while idle, the accent while live, with a soft pulse.
fn wire(lines: &mut Vec<f32>, glow_lines: &mut Vec<f32>, look: &Look, s: &Seg, a: f32, g: Glow) {
    let (base, alpha) = match s.flow {
        Flow::Idle => (look.idle, 0.85),
        Flow::Off => (look.idle, 0.6),
        Flow::On | Flow::Link => (look.accent, 1.0),
    };
    let col = mix(mix(look.idle, base, 1.0), look.accent, g.level);
    let live = g.level.clamp(0.0, 1.0);
    push_line(
        lines,
        &Line {
            a: s.a,
            b: s.b,
            color: with_alpha(col, a * (alpha * (1.0 - live) + live)),
            width: look.wire_px + 0.4 * live,
            emit: live,
            phase0: s.phase0,
            flow: live,
        },
    );
    if live > 0.01 && a >= 1.0 {
        push_line(
            glow_lines,
            &Line {
                a: s.a,
                b: s.b,
                color: with_alpha(look.accent, 0.16 * look.glow * live),
                width: look.wire_px * 4.0,
                emit: 0.0,
                phase0: 0.0,
                flow: 0.0,
            },
        );
    }
}
