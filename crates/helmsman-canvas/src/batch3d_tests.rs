//! What the GL pass is given: whole vertices, finite numbers, the right
//! things dimmed while a dive is on.

use crate::batch3d::{self, Batch, Focus, LINE_STRIDE, SOLID_STRIDE};
use crate::playground::{Session, example};
use crate::scene3d::Role;
use crate::st_view::Viewer;
use crate::theme::Theme;
use crate::view3d::scene_of;

fn batch(focus: Focus, scans: usize, dark: bool) -> (Batch, usize) {
    let src = example("Traffic light").src;
    let mut s = Session::new(src);
    for _ in 0..scans {
        s.step();
    }
    let v = Viewer::new(src, s.build.as_ref().unwrap().module.clone().unwrap());
    let scene = scene_of(&v, s.trace.as_ref().map(|t| t as &dyn helmsman_diagram::Scan));
    let boxes = scene.solids.len();
    (
        batch3d::build(&scene, focus, &Theme::for_mode(dark), None, 0.0),
        boxes,
    )
}

#[test]
fn every_solid_is_twelve_triangles_and_the_floats_are_whole_vertices() {
    for dark in [true, false] {
        let (b, boxes) = batch(Focus::Overview, 2, dark);
        assert_eq!((b.solids.len() + b.solids_ghost.len()) / SOLID_STRIDE, boxes * 36);
        assert_eq!(b.solids.len() % SOLID_STRIDE, 0);
        assert_eq!(b.solids_ghost.len() % SOLID_STRIDE, 0);
        assert_eq!(b.lines.len() % (LINE_STRIDE * 6), 0, "six vertices a segment");
        assert_eq!(b.lines_ghost.len() % (LINE_STRIDE * 6), 0);
        for v in [&b.solids, &b.solids_ghost, &b.lines, &b.lines_ghost] {
            assert!(v.iter().all(|f| f.is_finite()));
        }
    }
}

#[test]
fn a_scan_gives_bloom_something_and_no_scan_does_not() {
    assert!(!batch(Focus::Overview, 0, true).0.any_lit);
    assert!(batch(Focus::Overview, 3, true).0.any_lit);
}

#[test]
fn in_a_dive_the_middle_level_is_a_ghost_and_the_board_is_not() {
    let (over, _) = batch(Focus::Overview, 3, true);
    let (dive, _) = batch(Focus::Dive(0), 3, true);
    // Overview: only the regions' veils blend. Dive: the POU node does too.
    assert!(dive.solids_ghost.len() > over.solids_ghost.len());
    assert!(!dive.solids.is_empty(), "the board's blocks stay solid");
    // A ghost does not glow: it must not feed the bloom.
    // (A region's veil is a ghost that glows a little: only the faint
    // ones, the dimmed levels, are held to it.)
    let glowing_ghost = dive
        .solids_ghost
        .chunks(SOLID_STRIDE)
        .any(|v| v[9] < 0.3 && v[10] > 0.0);
    assert!(!glowing_ghost);
}

#[test]
fn a_lit_thing_is_greener_than_the_same_thing_off() {
    let t = Theme::dark();
    let off = batch3d::solid_color(Role::Write, &t);
    let on = batch3d::glow_color(Role::Write);
    assert!(on[1] > off[1]);
}

#[test]
fn the_theme_changes_the_colours() {
    assert_ne!(
        batch3d::solid_color(Role::Board, &Theme::dark()),
        batch3d::solid_color(Role::Board, &Theme::light())
    );
}
