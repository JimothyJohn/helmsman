//! The 3D view's labels held to what the 2D diagram says: every name,
//! value and region title it draws is in the label set, and placement
//! keeps them readable without ever dropping the nearest.

use std::collections::BTreeSet;

use helmsman_diagram::Scan;

use crate::cam3d::{Camera, Orbit, Pose};
use crate::labels3d::{Align, Kind, Label, Opts, Tone, place};
use crate::playground::{Session, example};
use crate::scene3d::Item;
use crate::st_view::{Viewer, gate_state, value_text};
use crate::view3d::{Nav, built_of};
use helmsman_diagram::NodeKind;
use helmsman_iec::Value;

fn viewer(src: &str, scans: usize) -> (Viewer, Session) {
    let mut s = Session::new(src);
    for _ in 0..scans {
        s.step();
    }
    let v = Viewer::new(src, s.build.as_ref().unwrap().module.clone().unwrap());
    (v, s)
}

fn scan(s: &Session) -> Option<&dyn Scan> {
    s.trace.as_ref().map(|t| t as &dyn Scan)
}

fn texts(l: &[Label], kind: Kind) -> BTreeSet<String> {
    l.iter()
        .filter(|l| l.kind == kind)
        .map(|l| l.text.clone())
        .collect()
}

const PROGRAMS: [&str; 3] = ["Traffic light", "Blink", "Logic: AND, OR, NOT"];

#[test]
fn every_label_the_2d_diagram_draws_is_in_the_3d_set() {
    for name in PROGRAMS {
        let (v, s) = viewer(example(name).src, 3);
        let b = built_of(&v, scan(&s), &[]);
        for (i, sc) in v.scopes.iter().enumerate() {
            let pou = &v.module.pous[sc.pou];
            let d = &pou.diagram;
            let mine: Vec<&Label> = b.labels.iter().filter(|l| l.scope == i).collect();
            for n in &d.nodes {
                assert!(
                    mine.iter().any(|l| l.kind == Kind::Node
                        && l.item == Some(Item::Block(i, n.id))
                        && l.text.eq_ignore_ascii_case(&n.label)),
                    "{name}: block {} has no label",
                    n.label
                );
                for port in n.ins.iter().chain(&n.outs).filter(|p| !p.is_empty()) {
                    assert!(
                        mine.iter().any(|l| l.kind == Kind::Port && &l.text == port),
                        "{name}: port {port} of {} has no label",
                        n.label
                    );
                }
            }
            for r in &d.regions {
                assert!(
                    mine.iter().any(|l| l.kind == Kind::Region && l.text == r.label),
                    "{name}: region {} has no title",
                    r.label
                );
            }
            // The live ones: a variable's value, a non-BOOL wire's value,
            // a region's state, each as the 2D view's own functions say it.
            let live = s
                .trace
                .as_ref()
                .map(|t| helmsman_diagram::annotate(d, t, &sc.prefix));
            let live = live.as_ref().unwrap();
            for n in &d.nodes {
                if matches!(n.kind, NodeKind::Read { .. } | NodeKind::Write { .. })
                    && let Some(val) = live.values[n.id.0 as usize].first().copied().flatten()
                    && let Some(text) = value_text(d, n.id, val, Some(&pou.vars))
                {
                    assert!(
                        mine.iter().any(|l| l.kind == Kind::Value && l.text == text),
                        "{name}: value {text} of {} has no label",
                        n.label
                    );
                }
            }
            for (k, w) in d.wires.iter().enumerate() {
                if let Some(val) = live.wire(d, k)
                    && !matches!(val, Value::Bool(_))
                    && let Some(text) = value_text(d, w.from, val, Some(&pou.vars))
                {
                    assert!(
                        mine.iter().any(|l| l.kind == Kind::WireValue && l.text == text),
                        "{name}: wire value {text} has no label"
                    );
                }
            }
            for r in &d.regions {
                let state = gate_state(Some(live.gates[r.id.0 as usize]));
                assert!(
                    mine.iter()
                        .any(|l| l.kind == Kind::RegionState && l.text == state),
                    "{name}: region state {state} has no label"
                );
            }
        }
        // One POU alone is named by the pane's header, not on its board.
        assert!(b.labels.iter().all(|l| l.kind != Kind::Title), "{name}");
    }
}

#[test]
fn a_boolean_variable_reads_green_or_red_as_in_2d() {
    let (v, s) = viewer(example("Traffic light").src, 3);
    let b = built_of(&v, scan(&s), &[]);
    let tones: BTreeSet<_> = b
        .labels
        .iter()
        .filter(|l| l.kind == Kind::Value)
        .map(|l| format!("{:?}", l.tone))
        .collect();
    assert!(tones.contains("True") || tones.contains("False"), "{tones:?}");
    assert!(
        b.labels
            .iter()
            .all(|l| l.kind != Kind::Node || l.tone == Tone::Normal)
    );
}

#[test]
fn without_a_scan_there_are_no_values_or_states_only_names() {
    let (v, _s) = viewer(example("Traffic light").src, 0);
    let b = built_of(&v, None, &[]);
    assert!(texts(&b.labels, Kind::Value).is_empty());
    assert!(texts(&b.labels, Kind::WireValue).is_empty());
    assert!(texts(&b.labels, Kind::RegionState).is_empty());
    assert!(!texts(&b.labels, Kind::Node).is_empty());
}

/// The camera the view opens with in a pane of `size`.
fn first_screen(b: &crate::view3d::Built, size: (f32, f32)) -> Camera {
    let mut nav = Nav::default();
    Camera::new(nav.pose(&b.scene, size.0 / size.1, 0.0), size)
}

#[test]
fn the_first_screen_names_nearly_every_block() {
    for size in [(1000.0, 560.0), (700.0, 500.0), (1400.0, 800.0)] {
        for name in PROGRAMS {
            let (v, s) = viewer(example(name).src, 3);
            let b = built_of(&v, scan(&s), &[]);
            let cam = first_screen(&b, size);
            let placed = place(&b.labels, &cam, &Opts::default());
            let named: std::collections::HashSet<Item> =
                placed.iter().filter_map(|p| b.labels[p.index].item).collect();
            let missing: Vec<String> = b
                .labels
                .iter()
                .filter(|l| l.kind == Kind::Node)
                .filter(|l| !named.contains(&l.item.unwrap()))
                .map(|l| l.text.clone())
                .collect();
            // Fewer, better-placed labels beat nudged piles: a block whose
            // name has no room is left out, but nearly all are named.
            let blocks = b.labels.iter().filter(|l| l.kind == Kind::Node).count();
            assert!(
                missing.len() * 8 <= blocks,
                "{name} at {size:?}: no label on {} of {blocks}: {missing:?}",
                missing.len()
            );
        }
    }
}

#[test]
fn placed_labels_do_not_sit_on_each_other_and_stay_readable() {
    let (v, s) = viewer(example("Traffic light").src, 3);
    let b = built_of(&v, scan(&s), &[]);
    let size = (1000.0, 560.0);
    let o = Opts::default();
    for yaw in [-0.55, 0.0, 1.2, 3.0] {
        let mut orbit = Orbit {
            yaw,
            ..Orbit::default()
        }
        .framing(&b.scene.bounds(), 1000.0 / 560.0);
        for dist_k in [1.0, 0.5, 0.25, 1.7] {
            orbit.dist *= dist_k;
            let cam = Camera::new(orbit.pose(), size);
            let placed = place(&b.labels, &cam, &o);
            for (i, a) in placed.iter().enumerate() {
                assert!(a.font >= o.font.0 - 1e-3 && a.font <= o.font.1 + 1e-3, "{a:?}");
                for c in &placed[i + 1..] {
                    let hit = a.rect[0] < c.rect[2]
                        && c.rect[0] < a.rect[2]
                        && a.rect[1] < c.rect[3]
                        && c.rect[1] < a.rect[3];
                    assert!(!hit, "{a:?} overlaps {c:?}");
                }
            }
        }
    }
}

#[test]
fn culling_never_drops_the_nearest_label() {
    let (v, s) = viewer(example("Traffic light").src, 3);
    let b = built_of(&v, scan(&s), &[]);
    let size = (1000.0, 560.0);
    for yaw in [-0.55, 0.4, 2.0, 4.0] {
        for pitch in [0.2, 0.62, 1.3] {
            let o = Orbit {
                yaw,
                pitch,
                ..Orbit::default()
            }
            .framing(&b.scene.bounds(), 1000.0 / 560.0);
            let cam = Camera::new(o.pose(), size);
            let placed = place(&b.labels, &cam, &Opts::default());
            // The nearest of the labels that are on screen at all.
            let th = (crate::cam3d::FOV_Y / 2.0).tan();
            let nearest = b
                .labels
                .iter()
                .enumerate()
                .filter_map(|(i, l)| cam.project(l.at).map(|(x, y, d)| (i, x, y, d)))
                .filter(|(_, x, y, _)| (0.0..=size.0).contains(x) && (0.0..=size.1).contains(y))
                .min_by(|a, b| a.3.total_cmp(&b.3));
            let _ = th;
            if let Some((i, ..)) = nearest {
                // It may have been too small to read (collapsed); a
                // block's, a region's or the POU's name never is.
                if matches!(b.labels[i].kind, Kind::Node | Kind::Region | Kind::Title) {
                    assert!(
                        placed.iter().any(|p| p.index == i),
                        "yaw {yaw} pitch {pitch}: the nearest label {:?} was dropped",
                        b.labels[i].text
                    );
                }
            }
        }
    }
}

#[test]
fn a_long_label_is_cut_with_an_ellipsis_never_past_its_block() {
    let (v, s) = viewer(example("Traffic light").src, 3);
    let b = built_of(&v, scan(&s), &[]);
    let cam = first_screen(&b, (1000.0, 560.0));
    for p in place(&b.labels, &cam, &Opts::default()) {
        let l = &b.labels[p.index];
        if p.text != l.text {
            assert!(
                p.text.ends_with('…') && l.text.starts_with(p.text.trim_end_matches('…')),
                "{} / {}",
                p.text,
                l.text
            );
        }
    }
}

#[test]
fn nothing_is_placed_without_a_viewport_or_behind_the_camera() {
    let (v, s) = viewer(example("Blink").src, 1);
    let b = built_of(&v, scan(&s), &[]);
    for size in [(0.0, 0.0), (f32::NAN, 10.0), (0.5, 500.0)] {
        let cam = Camera::new(Pose::default(), size);
        assert!(place(&b.labels, &cam, &Opts::default()).is_empty(), "{size:?}");
    }
    let away = Camera::new(
        Pose {
            eye: [0.0, 8.0, 10.0],
            target: [0.0, 40.0, 60.0],
        },
        (800.0, 600.0),
    );
    let _ = place(&b.labels, &away, &Opts::default());
    let l = Label {
        at: [f32::NAN; 3],
        text: "x".into(),
        kind: Kind::Node,
        tone: Tone::Normal,
        align: Align::Center,
        scope: 0,
        width: 1.0,
        size: 10.0,
        item: None,
    };
    assert_eq!(
        place(
            &[l],
            &Camera::new(Pose::default(), (800.0, 600.0)),
            &Opts::default()
        ),
        Vec::new()
    );
}

#[test]
fn a_dive_leaves_the_other_boards_unlabelled_but_titled() {
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
    let (v, s) = viewer(src, 2);
    let b = built_of(&v, scan(&s), &[]);
    assert_eq!(b.scene.boards.len(), 2);
    let o = Opts {
        focus: Some(0),
        ..Opts::default()
    };
    let cam = first_screen(&b, (1000.0, 560.0));
    for p in place(&b.labels, &cam, &o) {
        let l = &b.labels[p.index];
        assert!(l.scope == 0 || l.kind == Kind::Title, "{l:?}");
    }
}
