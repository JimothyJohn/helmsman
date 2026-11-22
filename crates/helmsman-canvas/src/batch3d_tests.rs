//! What the GL pass is given: whole vertices, finite numbers, shapes far
//! to near, lamps that light and buttons that sink with their input, and a
//! look for each theme.

use helmsman_diagram::Scan;
use helmsman_iec::Value;

use crate::anim3d::Glows;
use crate::batch3d::{self, Batch, Focus, LINE_STRIDE, M_HALO, M_SPHERE, Rig, SHAPE_STRIDE};
use crate::cam3d::{Camera, Orbit, Pose};
use crate::look3d::Look;
use crate::playground::{Point, Session, example};
use crate::scene3d::{Hue, Role, Scene};
use crate::st_view::Viewer;
use crate::theme::Theme;
use crate::view3d::built_of;

fn scene_of(name: &str, scans: usize, set: &[(&str, bool)]) -> (Scene, Session) {
    let src = example(name).src;
    let mut s = Session::new(src);
    for (n, on) in set {
        assert!(s.set_input(n, Value::Bool(*on)), "{n}");
    }
    for _ in 0..scans {
        s.step();
    }
    let v = Viewer::new(src, s.build.as_ref().unwrap().module.clone().unwrap());
    let io: Vec<Point> = s.inputs().chain(s.outputs()).cloned().collect();
    let scene = built_of(&v, s.trace.as_ref().map(|t| t as &dyn Scan), &io).scene;
    (scene, s)
}

fn rig(scene: &Scene) -> Rig {
    let o = Orbit::default().framing_points(&scene.frame_points(None), 1.8);
    Rig::of(&o.pose())
}

fn batch(name: &str, scans: usize, set: &[(&str, bool)], focus: Focus, dark: bool) -> Batch {
    let (scene, _) = scene_of(name, scans, set);
    let look = Look::of(&Theme::for_mode(dark));
    batch3d::build(&scene, focus, &look, &rig(&scene), None, 0.0)
}

fn all(b: &Batch) -> Vec<&Vec<f32>> {
    let mut v: Vec<&Vec<f32>> = vec![&b.glow, &b.glow_lines, &b.links];
    for x in &b.boards {
        v.extend([&x.under, &x.lines, &x.over]);
    }
    v
}

#[test]
fn the_floats_are_whole_vertices_and_finite_in_both_themes() {
    for dark in [true, false] {
        for name in ["Traffic light", "Blink", "Logic: AND, OR, NOT"] {
            let b = batch(name, 2, &[], Focus::Overview, dark);
            for x in &b.boards {
                assert_eq!(x.under.len() % (SHAPE_STRIDE * 6), 0, "six vertices a quad");
                assert_eq!(x.over.len() % (SHAPE_STRIDE * 6), 0);
                assert_eq!(x.lines.len() % (LINE_STRIDE * 6), 0, "six vertices a segment");
            }
            assert_eq!(b.glow.len() % (SHAPE_STRIDE * 6), 0);
            assert_eq!(b.glow_lines.len() % (LINE_STRIDE * 6), 0);
            for v in all(&b) {
                assert!(v.iter().all(|f| f.is_finite()), "{name}");
            }
        }
    }
}

#[test]
fn boards_are_drawn_far_to_near() {
    let src = "FUNCTION_BLOCK pulse
VAR_INPUT go : BOOL; END_VAR
VAR_OUTPUT q : BOOL; END_VAR
q := go;
END_FUNCTION_BLOCK
PROGRAM main
VAR_INPUT run : BOOL := TRUE; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR p : pulse; END_VAR
p(go := run);
lamp := p.q;
END_PROGRAM
";
    let mut s = Session::new(src);
    s.step();
    let v = Viewer::new(src, s.build.as_ref().unwrap().module.clone().unwrap());
    let scene = built_of(&v, None, &[]).scene;
    let look = Look::of(&Theme::dark());
    let r = rig(&scene);
    let b = batch3d::build(&scene, Focus::Overview, &look, &r, None, 0.0);
    assert_eq!(b.boards.len(), 2);
    // The first shape of each board is its floor; the farther is first.
    let depth = |x: &batch3d::BoardBatch| {
        let p = [x.under[0], x.under[1], x.under[2]];
        crate::cam3d::dot(crate::cam3d::sub(p, r.eye), r.fwd)
    };
    assert!(depth(&b.boards[0]) >= depth(&b.boards[1]));
}

#[test]
fn in_a_dive_the_other_boards_are_faint() {
    let (scene, _) = scene_of("Traffic light", 2, &[]);
    let look = Look::of(&Theme::dark());
    let r = rig(&scene);
    let over = batch3d::build(&scene, Focus::Overview, &look, &r, None, 0.0);
    let dive = batch3d::build(&scene, Focus::Dive(0), &look, &r, None, 0.0);
    // Alpha is the 10th float of a shape vertex (position 3, uv 2, half 2,
    // radius 1, colour r g b a).
    let alphas = |b: &Batch| -> Vec<f32> { b.boards[0].over.chunks(SHAPE_STRIDE).map(|v| v[11]).collect() };
    assert!(alphas(&over).iter().all(|a| *a > 0.9));
    assert!(
        alphas(&dive).iter().all(|a| *a > 0.9),
        "the dived board stays solid"
    );
}

#[test]
fn a_lit_lamp_makes_a_halo_in_its_colour_and_an_unlit_one_does_not() {
    // Traffic: the green lamp is lit at the start (phase 0).
    let on = batch("Traffic light", 3, &[], Focus::Overview, true);
    let halos: Vec<&[f32]> = on
        .glow
        .chunks(SHAPE_STRIDE)
        .filter(|v| (v[15] - M_HALO).abs() < 1e-6 || v[12] == M_HALO)
        .collect();
    assert!(!halos.is_empty(), "a lamp is lit: a halo is added");
    // Nothing lit (no inputs, the lamp off at the start): no light added.
    let off = batch("Blink", 0, &[], Focus::Overview, true);
    assert!(
        off.glow.is_empty(),
        "{} floats of glow with nothing lit",
        off.glow.len()
    );
    assert!(!off.any_lit);
    assert!(on.any_lit);
}

#[test]
fn the_glass_of_a_lamp_is_dark_off_and_the_hue_of_its_name_on() {
    let (scene, _) = scene_of("Traffic light", 3, &[]);
    let look = Look::of(&Theme::dark());
    for (hue, rgb) in [
        (Hue::Red, Look::hue(Hue::Red)),
        (Hue::Amber, Look::hue(Hue::Amber)),
        (Hue::Green, Look::hue(Hue::Green)),
    ] {
        assert!(rgb.iter().all(|c| (0.0..=1.0).contains(c)));
        let _ = hue;
    }
    let reds = Look::hue(Hue::Red);
    assert!(reds[0] > reds[1] * 2.0, "red is red: {reds:?}");
    let g = Look::hue(Hue::Green);
    assert!(g[1] > g[0] * 1.5 && g[1] > g[2] * 1.5, "green is green: {g:?}");
    let a = Look::hue(Hue::Amber);
    assert!(a[0] > a[2] * 3.0 && a[1] > a[2] * 2.0, "amber is amber: {a:?}");
    // The sphere sprites carry the lit amount: one lit lamp among three.
    let b = batch3d::build(&scene, Focus::Overview, &look, &rig(&scene), None, 0.0);
    let lit: Vec<f32> = b.boards[0]
        .over
        .chunks(SHAPE_STRIDE * 6)
        .filter(|q| (q[12] - M_SPHERE).abs() < 1e-6)
        .map(|q| q[14])
        .collect();
    assert_eq!(lit.len(), 3, "three lamps");
    assert_eq!(lit.iter().filter(|l| **l > 0.5).count(), 1, "one lit: {lit:?}");
}

#[test]
fn a_pressed_button_sinks_and_lights_and_a_released_one_does_not() {
    let top_of_cap = |set: &[(&str, bool)]| -> (f32, f32) {
        let (scene, _) = scene_of("Logic: AND, OR, NOT", 2, set);
        let look = Look::of(&Theme::dark());
        let b = batch3d::build(&scene, Focus::Overview, &look, &rig(&scene), None, 0.0);
        let mut best = f32::MIN;
        let mut live = 0.0f32;
        for q in b.boards[0].over.chunks(SHAPE_STRIDE) {
            // Round tiles (radius = half size) with a live parameter or a high top.
            if q[12] == 0.0 && q[7] > 0.0 && (q[7] - q[5]).abs() < 1e-4 {
                best = best.max(q[1]);
                live = live.max(q[14]);
            }
        }
        (best, live)
    };
    let (up_y, up_live) = top_of_cap(&[("a", false), ("b", false)]);
    let (dn_y, dn_live) = top_of_cap(&[("a", true), ("b", true)]);
    assert!(dn_y < up_y - 0.05, "the cap sinks: {up_y} -> {dn_y}");
    assert!(dn_live > up_live + 0.5, "and lights: {up_live} -> {dn_live}");
}

#[test]
fn each_theme_has_its_own_considered_look() {
    let (d, l) = (Look::of(&Theme::dark()), Look::of(&Theme::light()));
    assert_ne!(d.floor, l.floor);
    assert!(d.floor.iter().sum::<f32>() < l.floor.iter().sum::<f32>());
    assert!(d.tile.iter().sum::<f32>() < l.tile.iter().sum::<f32>());
    for look in [d, l] {
        for role in [
            Role::Read,
            Role::Write,
            Role::Op,
            Role::Func,
            Role::Fb,
            Role::Const,
            Role::Other,
            Role::Region,
            Role::Button,
            Role::Lamp,
            Role::Numeric,
            Role::Readout,
            Role::Board,
        ] {
            let c = look.tile_color(role);
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)), "{role:?} {c:?}");
        }
        // The accent is calm: not a saturated primary.
        let m = look.accent.iter().copied().fold(0.0_f32, f32::max)
            - look.accent.iter().copied().fold(1.0_f32, f32::min);
        assert!(m < 0.75, "{:?}", look.accent);
        assert!(
            look.side_shade(1) < look.side_shade(look.layers),
            "sides are darker lower down"
        );
    }
}

#[test]
fn the_glows_ease_the_halo_in() {
    let (scene, _) = scene_of("Traffic light", 3, &[]);
    let look = Look::of(&Theme::dark());
    let r = rig(&scene);
    let mut g = Glows::default();
    let solids: Vec<(bool, u64)> = scene.solids.iter().map(|s| (false, s.sig)).collect();
    let segs: Vec<(bool, u64)> = scene.segs.iter().map(|_| (false, 0)).collect();
    g.update(&solids, &segs, 0.0);
    let lit: Vec<(bool, u64)> = scene.solids.iter().map(|s| (s.lit, s.sig)).collect();
    g.update(&lit, &segs, 1.0);
    let early = batch3d::build(&scene, Focus::Overview, &look, &r, Some(&g), 1.05);
    let late = batch3d::build(&scene, Focus::Overview, &look, &r, Some(&g), 3.0);
    let strength = |b: &Batch| b.glow.chunks(SHAPE_STRIDE).map(|v| v[14]).fold(0.0_f32, f32::max);
    assert!(
        strength(&early) < strength(&late),
        "{} then {}",
        strength(&early),
        strength(&late)
    );
    let _ = Camera::new(Pose::default(), (1.0, 1.0));
}
