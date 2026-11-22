//! The 3D scene held to the diagram it is built from: every node once,
//! the levels in order, a click on a block's projected centre picks that
//! block, a scan lights what it should, hostile diagrams stay finite.

use std::collections::BTreeSet;

use helmsman_diagram::{Diagram, Live, NodeId, Scan};

use crate::cam3d::{Camera, finite};
use crate::playground::{Session, example};
use crate::scene3d::{self, Flow, Item, Level, MIDDLE_Y, Role, Scene, ScopeIn};
use crate::st_view::Viewer;
use crate::view3d::{Nav, scene_of};

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

#[test]
fn every_node_of_every_scope_is_in_the_scene_once() {
    for src in [example("Traffic light").src, example("Blink").src, WITH_FB] {
        let (v, s) = viewer(src, 2);
        let scene = scene_of(&v, scan(&s));
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
        let pous = scene
            .solids
            .iter()
            .filter(|s| matches!(s.item, Some(Item::Pou(_))))
            .count();
        assert_eq!(pous, v.scopes.len(), "one node per POU scope");
    }
}

#[test]
fn a_function_block_instance_is_a_second_pou_called_from_the_program() {
    let (v, s) = viewer(WITH_FB, 1);
    assert_eq!(v.scopes.len(), 2, "{:?}", v.scopes);
    let scene = scene_of(&v, scan(&s));
    assert_eq!(scene.pous.len(), 2);
    let links = scene
        .segs
        .iter()
        .filter(|s| s.flow == Flow::Link && s.level == Level::Middle && (s.a[1] - s.b[1]).abs() < 1e-6)
        .count();
    assert_eq!(links, 1, "program to instance");
}

#[test]
fn the_levels_are_ordered_by_height_and_the_boards_do_not_overlap() {
    let (v, s) = viewer(WITH_FB, 1);
    let scene = scene_of(&v, scan(&s));
    for p in &scene.pous {
        assert!(p.min[1] >= MIDDLE_Y);
    }
    for sol in scene.solids.iter().filter(|s| s.level == Level::Bottom) {
        assert!(sol.bounds.max[1] < MIDDLE_Y - 1.0, "{:?}", sol.role);
    }
    for (i, a) in scene.boards.iter().enumerate() {
        for b in &scene.boards[i + 1..] {
            let apart =
                a.max[0] <= b.min[0] || b.max[0] <= a.min[0] || a.max[2] <= b.min[2] || b.max[2] <= a.min[2];
            assert!(apart, "{a:?} overlaps {b:?}");
        }
    }
}

#[test]
fn a_click_on_a_blocks_projected_centre_picks_that_block() {
    for src in [example("Traffic light").src, WITH_FB] {
        let (v, s) = viewer(src, 1);
        let scene = scene_of(&v, scan(&s));
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
fn from_the_overview_a_pou_node_is_picked_by_its_centre() {
    let (v, s) = viewer(WITH_FB, 1);
    let scene = scene_of(&v, scan(&s));
    let size = (1000.0, 700.0);
    let mut nav = Nav::default();
    let cam = Camera::new(nav.pose(&scene, size.0 / size.1, 0.0), size);
    for (i, p) in scene.pous.iter().enumerate() {
        let c = p.center();
        let (x, y, _) = cam.project(c).unwrap();
        assert_eq!(scene.pick(&cam.ray(x, y).unwrap(), None), Some(Item::Pou(i)));
    }
    // A click on empty space picks nothing.
    assert_eq!(scene.pick(&cam.ray(1.0, 1.0).unwrap(), None), None);
}

#[test]
fn in_a_dive_the_ghost_above_is_not_pickable() {
    let (v, s) = viewer(example("Traffic light").src, 1);
    let scene = scene_of(&v, scan(&s));
    let size = (1000.0, 700.0);
    let cam = Camera::new(Nav::default().pose(&scene, 1000.0 / 700.0, 0.0), size);
    let c = scene.pous[0].center();
    let (x, y, _) = cam.project(c).unwrap();
    assert_eq!(scene.pick(&cam.ray(x, y).unwrap(), Some(0)), None);
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
    // `run` is TRUE: its read, and the wires it drives, are live.
    assert!(
        hot.solids
            .iter()
            .any(|s| s.lit && matches!(s.item, Some(Item::Block(..))))
    );
    assert!(hot.segs.iter().any(crate::scene3d::Seg::lit));
    assert!(
        hot.segs
            .iter()
            .all(|s| s.flow != Flow::Idle || s.level == Level::Bottom)
    );
    let _ = s;
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

#[test]
fn one_empty_diagram_is_one_node_on_an_empty_board() {
    let d = Diagram::default();
    let scene = scene3d::build(&[ScopeIn {
        label: "EMPTY".into(),
        diagram: &d,
        live: None,
        parent: None,
    }]);
    assert_eq!(scene.pous.len(), 1);
    assert_eq!(scene.boards.len(), 1);
    assert!(scene.solids.iter().any(|s| s.role == Role::Pou));
    for s in &scene.solids {
        assert!(finite(s.bounds.min) && finite(s.bounds.max), "{:?}", s.role);
    }
}

#[test]
fn a_diagram_with_nan_geometry_gives_a_scene_with_none() {
    let (v, _s) = viewer(example("Blink").src, 0);
    let mut d = v.module.pous[0].diagram.clone();
    d.size = (f32::NAN, f32::INFINITY);
    for n in &mut d.nodes {
        n.rect.x = f32::NAN;
        n.rect.w = f32::INFINITY;
    }
    for r in &mut d.regions {
        r.rect.y = f32::NAN;
    }
    let scene = scene3d::build(&[ScopeIn {
        label: "X".into(),
        diagram: &d,
        live: None,
        parent: Some(7),
    }]);
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
    let before = scene3d::build(&[ScopeIn {
        label: "X".into(),
        diagram: &d,
        live: None,
        parent: None,
    }])
    .segs
    .len();
    d.wires.push(helmsman_diagram::Wire {
        from: NodeId(9999),
        out: 0,
        to: NodeId(0),
        inp: 0,
        negated: false,
    });
    let after = scene3d::build(&[ScopeIn {
        label: "X".into(),
        diagram: &d,
        live: None,
        parent: None,
    }])
    .segs
    .len();
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
    let scene: Scene = scene3d::build(&[ScopeIn {
        label: "X".into(),
        diagram: d,
        live: Some(&live),
        parent: None,
    }]);
    assert!(scene.solids.iter().all(|s| !s.lit || s.role == Role::Pou));
}
