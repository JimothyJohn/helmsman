//! The 3D scene held to the diagram it is built from: every node once,
//! every POU a board with a way to reach it, a click on a block's
//! projected centre picks that block, a scan lights what it should, the
//! simulation's declared inputs and outputs become buttons and lamps (and
//! nothing else does), hostile diagrams stay finite.

use std::collections::BTreeSet;

use helmsman_diagram::{Diagram, Live, NodeId, Scan};
use helmsman_iec::Value;

use crate::cam3d::{Aabb, Camera, finite};
use crate::playground::Widget;
use crate::playground::{Point, Session, example};
use crate::scene3d::{self, DROP, Flow, Hue, Item, Role, Scene, ScopeIn, classify, fill_of, hue_of};
use crate::st_view::Viewer;
use crate::view3d::{Nav, built_of, scene_of};

/// A viewer on `src` with a scan or two run.
fn viewer(src: &str, scans: usize) -> (Viewer, Session) {
    let mut session = Session::new(src);
    assert!(session.diags.is_empty(), "{:?}", session.diag_lines());
    for _ in 0..scans {
        session.step();
    }
    let module = session.build.as_ref().unwrap().module.clone().unwrap();
    (Viewer::new(src, module), session)
}

fn scan(s: &Session) -> Option<&dyn Scan> {
    s.trace.as_ref().map(|t| t as &dyn Scan)
}

fn io_of(s: &Session) -> Vec<Point> {
    s.inputs().chain(s.outputs()).cloned().collect()
}

/// The scene of a viewer with the simulation's I/O.
fn scene_io(v: &Viewer, s: &Session) -> Scene {
    built_of(v, scan(s), &io_of(s)).scene
}

const WITH_FB: &str = "FUNCTION_BLOCK pulse
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

fn roles(scene: &Scene, scope: usize) -> Vec<(String, Role)> {
    scene
        .solids
        .iter()
        .filter(|s| s.scope == scope && matches!(s.item, Some(Item::Block(..))))
        .map(|s| (s.label.clone(), s.role))
        .collect()
}

#[test]
fn every_node_of_every_scope_is_in_the_scene_once() {
    for src in [example("Traffic light").src, example("Blink").src, WITH_FB] {
        let (v, s) = viewer(src, 2);
        let scene = scene_io(&v, &s);
        for (i, sc) in v.scopes.iter().enumerate() {
            let d = &v.module.pous[sc.pou].diagram;
            let got: Vec<NodeId> = scene
                .solids
                .iter()
                .filter_map(|s| match s.item {
                    Some(Item::Block(b, n)) if b == i => Some(n),
                    _ => None,
                })
                .collect();
            let want: Vec<NodeId> = d.nodes.iter().map(|n| n.id).collect();
            assert_eq!(got, want, "scope {i} of {src}");
            let uniq: BTreeSet<NodeId> = got.iter().copied().collect();
            assert_eq!(uniq.len(), got.len());
        }
        assert_eq!(scene.boards.len(), v.scopes.len(), "one board per POU scope");
    }
}

#[test]
fn every_pou_has_a_board_a_title_and_a_way_to_reach_it() {
    let (v, s) = viewer(WITH_FB, 1);
    assert_eq!(v.scopes.len(), 2, "{:?}", v.scopes);
    let b = built_of(&v, scan(&s), &io_of(&s));
    for (i, sc) in v.scopes.iter().enumerate() {
        assert!(b.scene.solid(Item::Board(i)).is_some(), "board {i}");
        assert!(
            b.labels
                .iter()
                .any(|l| l.kind == crate::labels3d::Kind::Title && l.scope == i && l.text == sc.label),
            "title of board {i}"
        );
    }
    // A way to reach each: the dive and the keys take any scope.
    let mut nav = Nav::default();
    for i in 0..2 {
        assert!(nav.dive(&b.scene, i, 1.5, 0.0));
    }
    // One POU alone is named by the pane's header: no title.
    let (v1, s1) = viewer(example("Blink").src, 1);
    let b1 = built_of(&v1, scan(&s1), &io_of(&s1));
    assert!(b1.labels.iter().all(|l| l.kind != crate::labels3d::Kind::Title));
}

#[test]
fn a_called_board_stands_below_its_caller_with_a_link_from_the_calling_block() {
    let (v, s) = viewer(WITH_FB, 1);
    let scene = scene_io(&v, &s);
    assert!(
        scene.floor(1) <= scene.floor(0) - DROP + 1e-3,
        "{} under {}",
        scene.floor(1),
        scene.floor(0)
    );
    let links: Vec<_> = scene.segs.iter().filter(|s| s.flow == Flow::Link).collect();
    assert_eq!(links.len(), 1, "one call, one link");
    let l = links[0];
    // It starts on top of the block that calls (an Fb of the program) and
    // ends on the callee's board.
    let d = &v.module.pous[v.scopes[0].pou].diagram;
    let call = d
        .nodes
        .iter()
        .find(|n| matches!(n.kind, helmsman_diagram::NodeKind::Fb { .. }))
        .unwrap();
    let top = scene.nodes[0][call.id.0 as usize];
    assert!(
        (l.a[0] - top.center()[0]).abs() < 1e-3 && (l.a[1] - top.max[1]).abs() < 1e-3,
        "{l:?} {top:?}"
    );
    assert!(
        l.b[1] <= scene.floor(1) + 0.2 && l.b[1] >= scene.floor(1),
        "{} on {}",
        l.b[1],
        scene.floor(1)
    );
}

#[test]
fn no_board_hides_another() {
    let (v, s) = viewer(WITH_FB, 1);
    let scene = scene_io(&v, &s);
    let hit = |a: &Aabb, b: &Aabb| (0..3).all(|i| a.min[i] < b.max[i] && b.min[i] < a.max[i]);
    for (i, a) in scene.boards.iter().enumerate() {
        for b in &scene.boards[i + 1..] {
            assert!(!hit(a, b), "{a:?} overlaps {b:?}");
        }
        // Nor does one stand straight under another on screen: the two
        // are apart along x or z as well as y.
    }
    let (a, b) = (&scene.boards[0], &scene.boards[1]);
    assert!(a.max[0] <= b.min[0] || b.max[0] <= a.min[0] || a.max[2] <= b.min[2] || b.max[2] <= a.min[2]);
}

#[test]
fn a_click_on_a_blocks_projected_centre_picks_that_block() {
    for src in [example("Traffic light").src, WITH_FB] {
        let (v, s) = viewer(src, 1);
        let scene = scene_io(&v, &s);
        let size = (1000.0, 700.0);
        for scope in 0..scene.boards.len() {
            let mut nav = Nav::default();
            assert!(nav.dive(&scene, scope, size.0 / size.1, 0.0));
            let cam = Camera::new(nav.pose(&scene, size.0 / size.1, 10.0), size);
            let mut checked = 0;
            for sol in &scene.solids {
                let Some(Item::Block(sc, n)) = sol.item else {
                    continue;
                };
                if sc != scope {
                    continue;
                }
                let c = sol.bounds.center();
                let (x, y, _) = cam.project([c[0], sol.bounds.max[1], c[2]]).unwrap();
                let ray = cam.ray(x, y).unwrap();
                assert_eq!(
                    scene.pick(&ray, Some(scope)),
                    Some(Item::Block(sc, n)),
                    "{} ({:?}) at {x},{y}",
                    sol.label,
                    sol.role
                );
                checked += 1;
            }
            assert!(checked > 0);
        }
    }
}

#[test]
fn from_the_overview_an_empty_spot_of_a_board_picks_the_board_and_empty_space_nothing() {
    let (v, s) = viewer(WITH_FB, 1);
    let scene = scene_io(&v, &s);
    let size = (1000.0, 700.0);
    let mut nav = Nav::default();
    let cam = Camera::new(nav.pose(&scene, size.0 / size.1, 0.0), size);
    for (i, b) in scene.boards.iter().enumerate() {
        // A spot on the board that no block covers.
        let mut found = None;
        'g: for gx in 1..40 {
            for gz in 1..40 {
                let p = [
                    b.min[0] + (b.max[0] - b.min[0]) * gx as f32 / 40.0,
                    scene.floor(i),
                    b.min[2] + (b.max[2] - b.min[2]) * gz as f32 / 40.0,
                ];
                let covered = scene.nodes[i].iter().any(|n| {
                    p[0] >= n.min[0] - 0.3
                        && p[0] <= n.max[0] + 0.3
                        && p[2] >= n.min[2] - 0.3
                        && p[2] <= n.max[2] + 0.3
                }) || scene.regions[i]
                    .iter()
                    .any(|r| p[0] >= r.min[0] && p[0] <= r.max[0] && p[2] >= r.min[2] && p[2] <= r.max[2]);
                if !covered {
                    found = Some(p);
                    break 'g;
                }
            }
        }
        let p = found.expect("an empty spot");
        let (x, y, _) = cam.project(p).unwrap();
        let got = scene.pick(&cam.ray(x, y).unwrap(), None);
        // Another board nearer the camera may be in the way; never a block
        // of this one.
        assert!(
            got == Some(Item::Board(i))
                || matches!(got, Some(Item::Board(_) | Item::Block(..)))
                    && got != Some(Item::Block(i, NodeId(0))),
            "board {i}: {got:?}"
        );
    }
    assert_eq!(scene.pick(&cam.ray(1.0, 1.0).unwrap(), None), None);
}

#[test]
fn nothing_glows_without_a_scan_and_something_does_with_one() {
    let (v, _s) = viewer(example("Traffic light").src, 0);
    let cold = scene_of(&v, None);
    assert!(cold.solids.iter().all(|s| !s.lit));
    assert!(cold.segs.iter().all(|s| !s.lit() && s.flow != Flow::On));
    assert!(cold.segs.iter().any(|s| s.flow == Flow::Idle));
    let (v, s) = viewer(example("Traffic light").src, 3);
    let hot = scene_of(&v, scan(&s));
    assert!(
        hot.solids
            .iter()
            .any(|s| s.lit && matches!(s.item, Some(Item::Block(..))))
    );
    assert!(hot.segs.iter().any(scene3d::Seg::lit));
}

#[test]
fn an_empty_scene_still_has_bounds_a_camera_can_frame() {
    let scene = scene3d::build(&[]);
    assert!(scene.solids.is_empty() && scene.segs.is_empty());
    let b = scene.bounds();
    assert!(finite(b.min) && finite(b.max));
    let mut nav = Nav::default();
    assert!(!nav.dive(&scene, 0, 1.0, 0.0), "nothing to dive into");
    assert!(!nav.up(&scene, 1.0, 0.0), "already up");
    let p = nav.pose(&scene, 1.0, 1.0);
    assert!(finite(p.eye) && finite(p.target));
}

fn scope<'a>(d: &'a Diagram, live: Option<&'a Live>, io: &'a [Point]) -> ScopeIn<'a> {
    ScopeIn {
        label: "X".into(),
        diagram: d,
        live,
        parent: None,
        via: None,
        io,
    }
}

#[test]
fn one_empty_diagram_is_one_board() {
    let d = Diagram::default();
    let scene = scene3d::build(&[scope(&d, None, &[])]);
    assert_eq!(scene.boards.len(), 1);
    assert!(scene.solid(Item::Board(0)).is_some());
    for s in &scene.solids {
        assert!(finite(s.bounds.min) && finite(s.bounds.max), "{:?}", s.role);
    }
}

#[test]
fn a_diagram_with_nan_geometry_gives_a_scene_with_none() {
    let (v, s) = viewer(example("Blink").src, 0);
    let mut d = v.module.pous[0].diagram.clone();
    d.size = (f32::NAN, f32::INFINITY);
    for n in &mut d.nodes {
        n.rect.x = f32::NAN;
        n.rect.w = f32::INFINITY;
    }
    for r in &mut d.regions {
        r.rect.y = f32::NAN;
    }
    let io = io_of(&s);
    let mut sc = scope(&d, None, &io);
    sc.parent = Some(7);
    let scene = scene3d::build(&[sc]);
    for s in &scene.solids {
        assert!(finite(s.bounds.min) && finite(s.bounds.max), "{:?}", s.role);
    }
    for s in &scene.segs {
        assert!(finite(s.a) && finite(s.b));
    }
    assert!(finite(scene.bounds().min));
}

#[test]
fn a_wire_to_a_node_that_is_not_there_is_left_out() {
    let (v, _s) = viewer(example("Blink").src, 0);
    let mut d = v.module.pous[0].diagram.clone();
    let before = scene3d::build(&[scope(&d, None, &[])]).segs.len();
    d.wires.push(helmsman_diagram::Wire {
        from: NodeId(9999),
        out: 0,
        to: NodeId(0),
        inp: 0,
        negated: false,
    });
    let after = scene3d::build(&[scope(&d, None, &[])]).segs.len();
    assert_eq!(before, after);
}

#[test]
fn a_live_that_does_not_fit_the_diagram_does_not_panic() {
    let (v, _s) = viewer(example("Blink").src, 0);
    let d = &v.module.pous[0].diagram;
    let live = Live {
        values: Vec::new(),
        ran: Vec::new(),
        gates: Vec::new(),
    };
    let scene: Scene = scene3d::build(&[scope(d, Some(&live), &[])]);
    assert!(scene.solids.iter().all(|s| !s.lit));
}

// ---- the simulation's inputs and outputs ----

#[test]
fn the_traffic_light_has_a_switch_and_three_lamps() {
    let (v, s) = viewer(example("Traffic light").src, 2);
    let scene = scene_io(&v, &s);
    let r = roles(&scene, 0);
    let of = |role: Role| -> Vec<String> {
        r.iter()
            .filter(|(_, x)| *x == role)
            .map(|(l, _)| l.to_ascii_lowercase())
            .collect()
    };
    let lamps = of(Role::Lamp);
    for name in ["red :=", "yellow :=", "green :="] {
        assert!(lamps.iter().any(|l| l == name), "{name} in {lamps:?}");
    }
    let buttons = of(Role::Button);
    assert!(
        !buttons.is_empty() && buttons.iter().all(|b| b == "run"),
        "{buttons:?}"
    );
    // The internal variable and the timer's member stay plain blocks.
    assert!(r.iter().any(|(l, role)| l == "PHASE" && *role == Role::Read));
    assert!(r.iter().all(|(l, role)| !(l == "T.Q" && *role != Role::Read)));
    // Every control is the program's input, run, a switch (not a hold).
    assert!(
        scene
            .controls
            .iter()
            .all(|c| c.name == "RUN" && c.widget == Widget::Toggle)
    );
    assert_eq!(scene.controls.len(), buttons.len());
}

#[test]
fn blink_is_one_lamp_and_logic_is_two_switches_and_three_lamps() {
    let (v, s) = viewer(example("Blink").src, 2);
    let scene = scene_io(&v, &s);
    let r = roles(&scene, 0);
    assert!(
        r.iter().any(|(l, role)| l == "LAMP :=" && *role == Role::Lamp),
        "{r:?}"
    );
    // Reading the output (NOT lamp) is a block, not a button.
    assert!(r.iter().all(|(_, role)| *role != Role::Button), "{r:?}");
    assert_eq!(scene.controls, Vec::new());
    let (v, s) = viewer(example("Logic: AND, OR, NOT").src, 2);
    let scene = scene_io(&v, &s);
    let r = roles(&scene, 0);
    let n = |role: Role| r.iter().filter(|(_, x)| *x == role).count();
    assert!(n(Role::Button) >= 2, "{r:?}");
    assert_eq!(n(Role::Lamp), 3, "{r:?}");
    let names: BTreeSet<&str> = scene.controls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, BTreeSet::from(["A", "B"]));
}

#[test]
fn with_no_io_declared_nothing_is_a_control() {
    let (v, s) = viewer(example("Traffic light").src, 2);
    let scene = built_of(&v, scan(&s), &[]).scene;
    assert_eq!(scene.controls, Vec::new());
    assert!(
        scene
            .solids
            .iter()
            .all(|s| !matches!(s.role, Role::Button | Role::Lamp | Role::Numeric | Role::Readout))
    );
}

#[test]
fn the_same_name_in_two_scopes_is_the_programs_alone() {
    // The function block's input is also called `run`.
    let src = "FUNCTION_BLOCK pulse
VAR_INPUT run : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
lamp := run;
END_FUNCTION_BLOCK
PROGRAM main
VAR_INPUT run : BOOL := TRUE; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR p : pulse; END_VAR
p(run := run);
lamp := p.lamp;
END_PROGRAM
";
    let (v, s) = viewer(src, 1);
    let scene = scene_io(&v, &s);
    assert_eq!(v.scopes.len(), 2);
    let program = roles(&scene, 0);
    let fb = roles(&scene, 1);
    assert!(program.iter().any(|(_, r)| *r == Role::Button), "{program:?}");
    assert!(
        fb.iter().all(|(_, r)| !matches!(r, Role::Button | Role::Lamp)),
        "{fb:?}"
    );
    assert!(scene.controls.iter().all(|c| c.scope == 0));
}

#[test]
fn an_input_never_read_has_no_control_and_a_wrong_way_round_name_is_no_lamp() {
    let src = "PROGRAM p\nVAR_INPUT unused : BOOL; used : BOOL; END_VAR\nVAR_OUTPUT out : BOOL; END_VAR\nout := used;\nEND_PROGRAM\n";
    let (v, s) = viewer(src, 1);
    let scene = scene_io(&v, &s);
    assert_eq!(scene.controls.len(), 1);
    assert_eq!(scene.controls[0].name, "USED");
    // Hostile declarations: an output listed as an input only, and the
    // other way round, classify as nothing.
    let mut io = io_of(&s);
    for p in &mut io {
        p.dir = match p.dir {
            crate::playground::Dir::In => crate::playground::Dir::Out,
            crate::playground::Dir::Out => crate::playground::Dir::In,
        };
    }
    let swapped = built_of(&v, scan(&s), &io).scene;
    assert!(swapped.controls.is_empty(), "a Read of an output is no button");
    for n in &v.module.pous[0].diagram.nodes {
        assert!(classify(&n.kind, &[]).is_none());
    }
}

#[test]
fn numbers_get_a_control_and_a_readout() {
    let src = "PROGRAM p\nVAR_INPUT n : INT; END_VAR\nVAR_OUTPUT total : INT; END_VAR\ntotal := n + 1;\nEND_PROGRAM\n";
    let (v, s) = viewer(src, 1);
    let scene = scene_io(&v, &s);
    let r = roles(&scene, 0);
    assert!(r.iter().any(|(_, x)| *x == Role::Numeric), "{r:?}");
    assert!(r.iter().any(|(_, x)| *x == Role::Readout), "{r:?}");
    assert!(matches!(scene.controls[0].widget, Widget::Int { .. }));
}

#[test]
fn a_lamp_follows_the_traced_value_and_a_button_the_input() {
    let (v, mut s) = viewer(example("Logic: AND, OR, NOT").src, 1);
    let lit_lamps = |v: &Viewer, s: &Session| -> Vec<String> {
        let scene = scene_io(v, s);
        scene
            .solids
            .iter()
            .filter(|x| x.role == Role::Lamp && x.lit)
            .map(|x| x.label.to_ascii_lowercase())
            .collect()
    };
    let pressed = |v: &Viewer, s: &Session| -> bool {
        scene_io(v, s)
            .solids
            .iter()
            .any(|x| x.role == Role::Button && x.lit)
    };
    assert!(s.set_input("a", Value::Bool(true)));
    assert!(s.set_input("b", Value::Bool(true)));
    s.step();
    let on = lit_lamps(&v, &s);
    assert!(on.iter().any(|l| l.starts_with("both")), "{on:?}");
    assert!(on.iter().any(|l| l.starts_with("either")), "{on:?}");
    assert!(!on.iter().any(|l| l.starts_with("neither")), "{on:?}");
    assert!(pressed(&v, &s));
    assert!(s.set_input("a", Value::Bool(false)));
    assert!(s.set_input("b", Value::Bool(false)));
    s.step();
    let off = lit_lamps(&v, &s);
    assert!(
        off.iter().any(|l| l.starts_with("neither")) && !off.iter().any(|l| l.starts_with("both")),
        "{off:?}"
    );
    assert!(!pressed(&v, &s));
}

#[test]
fn a_lamp_says_its_colour_by_its_name() {
    for (name, hue) in [
        ("red", Hue::Red),
        ("RED", Hue::Red),
        ("lamp_red", Hue::Red),
        ("yellow", Hue::Amber),
        ("amber", Hue::Amber),
        ("green", Hue::Green),
        ("blue", Hue::Blue),
        ("white", Hue::White),
        ("ReadyGreen", Hue::Green),
        ("lamp", Hue::Live),
        ("both", Hue::Live),
        ("reddish", Hue::Live),
        ("greenhouse", Hue::Live),
        ("", Hue::Live),
        ("____", Hue::Live),
    ] {
        assert_eq!(hue_of(name), hue, "{name:?}");
    }
    let (v, s) = viewer(example("Traffic light").src, 2);
    let scene = scene_io(&v, &s);
    let hue = |l: &str| {
        scene
            .solids
            .iter()
            .find(|x| x.role == Role::Lamp && x.label.eq_ignore_ascii_case(l))
            .map(|x| x.hue)
    };
    assert_eq!(hue("red :="), Some(Hue::Red));
    assert_eq!(hue("yellow :="), Some(Hue::Amber));
    assert_eq!(hue("green :="), Some(Hue::Green));
}

#[test]
fn a_numbers_place_on_its_range_is_zero_to_one_whatever_it_is() {
    let p = |w, v| Point {
        name: "N".into(),
        dir: crate::playground::Dir::In,
        place: crate::playground::Place::Port,
        ty: helmsman_iec::Ty::Int,
        widget: Some(w),
        value: v,
    };
    assert!((fill_of(&p(Widget::Int { min: 0, max: 100 }, Value::Int(25))) - 0.25).abs() < 1e-6);
    assert_eq!(
        fill_of(&p(Widget::Int { min: 0, max: 100 }, Value::Int(1_000_000))),
        1.0
    );
    assert_eq!(
        fill_of(&p(Widget::Int { min: 5, max: 5 }, Value::Int(5))),
        5.0 / 6.0
    );
    for v in [
        Value::Real(f64::NAN),
        Value::Real(f64::INFINITY),
        Value::Real(-3.0),
        Value::Int(i128::MAX),
    ] {
        let f = fill_of(&p(Widget::Real, v));
        assert!((0.0..=1.0).contains(&f), "{v:?} {f}");
    }
}
