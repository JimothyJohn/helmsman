//! The "Name the I/O first" form (`crate::io_names`) in the Agent tab: it
//! is there for a new project, a click adds a point, and at the pane's
//! default width it neither widens the pane nor runs out of it.

use super::{App, Harness};
use crate::io_names::{Dir, Kind, Point};

#[test]
fn the_form_adds_a_point_and_leaves_the_pane_its_width() {
    let mut h = Harness::new(App::sim());
    h.wait(0.3);
    let left = h.rect("left");
    assert!(h.has("io names"), "a built-in example open: a new project");
    h.click(h.rect("io names").center());
    h.wait(0.3);
    h.click(h.rect("io add").center());
    h.wait(0.3);
    let sim = h.app.sim.as_mut().unwrap();
    assert_eq!(sim.new_io.points.len(), 1);
    // Long names and meanings, as a real list has.
    sim.new_io.points[0] = Point {
        name: "conveyor_jam_photo_eye_upstream".into(),
        dir: Dir::Input,
        kind: Kind::Bool,
        what: "on while a carton blocks the eye at the upstream end of the accumulation conveyor".into(),
    };
    sim.new_io.points.push(Point {
        name: "2nd pump".into(),
        dir: Dir::Output,
        kind: Kind::Real,
        what: String::new(),
    });
    h.wait(0.3);
    assert!(h.drew("cannot be a name"), "the bad name is said in the form");
    let grew = h.rect("left").right() - left.right();
    assert!((0.0..=8.0).contains(&grew), "the left pane grew {grew} pt");
    let edge = h.rect("left").right();
    for (key, r) in &h.app.chrome.hits {
        if key.starts_with("io ") {
            assert!(
                r.right() <= edge + 0.5,
                "{key} runs out of the pane: {r:?} past {edge}"
            );
        }
    }
    assert_eq!(h.small_text(), Vec::new());
    // In or out, and the type, change on a click.
    h.click(h.rect("io dir 1").center());
    h.click(h.rect("io kind 1").center());
    let p = &h.app.sim.as_ref().unwrap().new_io.points[0];
    assert_eq!((p.dir, p.kind), (Dir::Output, Kind::Int));
    h.click(h.rect("io remove 2").center());
    assert_eq!(h.app.sim.as_ref().unwrap().new_io.points.len(), 1);
}
