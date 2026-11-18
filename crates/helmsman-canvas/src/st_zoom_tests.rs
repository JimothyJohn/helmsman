//! Semantic zoom on the canvas: the level follows the camera, the folds it
//! closes are never the reader's, and nothing it does reaches the layout.

use eframe::egui::{self, Event, Pos2, Rect, Vec2};
use helmsman_diagram::NodeKind;
use helmsman_diagram::fold::zoom::Level;

use super::super::{Cam, Pick, Scan, Viewer};
use crate::playground::Session;

const SRC: &str = "PROGRAM plant
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  other AT %IX0.2 : BOOL;
  motor AT %QX0.0 : BOOL;
  lamp AT %QX0.1 : BOOL;
  count : INT;
  state : INT;
END_VAR
(* Motor *)
motor := (start OR motor) AND NOT stop;
IF other THEN
  count := count + 1;
  lamp := TRUE;
ELSE
  lamp := FALSE;
  count := 0;
END_IF;
(* Sequence *)
CASE state OF
  0: IF start THEN state := 1; count := 0; END_IF;
  1: IF count > 5 THEN state := 2; END_IF;
  ELSE state := 0;
END_CASE;
count := count + 1;
END_PROGRAM
";

struct Harness {
    ctx: egui::Context,
    session: Session,
    view: Viewer,
    time: f64,
}

impl Harness {
    fn new(src: &str) -> Self {
        let mut session = Session::new(src);
        assert!(session.diags.is_empty(), "{:?}", session.diag_lines());
        session.step();
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        let mut h = Self {
            ctx: egui::Context::default(),
            session,
            view: Viewer::new(src, module),
            time: 0.0,
        };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += 1.0 / 60.0;
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        let view = &mut self.view;
        let mut out = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::no_frame().show(ui, |ui| view.canvas(ui, scan));
        });
        out.textures_delta.clear();
    }

    fn cam(&self) -> Cam {
        self.view.cams[self.view.scope]
    }

    /// Zooms the way the wheel does, about `at`, to `to`, in a few frames.
    fn zoom_to(&mut self, to: f32, at: Pos2) {
        for _ in 0..40 {
            let z = self.cam().zoom;
            if (z - to).abs() < 1e-3 {
                break;
            }
            let step = (to / z).clamp(0.9, 1.1);
            self.frame(vec![Event::PointerMoved(at), Event::Zoom(step)]);
        }
        self.frame(vec![Event::PointerMoved(at)]);
    }

    /// Labels of the blocks drawn as one.
    fn folded(&self) -> Vec<String> {
        self.view
            .shown()
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Fold { .. }))
            .map(|n| n.label.clone())
            .collect()
    }

    fn centre() -> Pos2 {
        Pos2::new(600.0, 400.0)
    }

    /// Sweeps the zoom from where it is to `to` and back, in steps.
    fn sweep(&mut self, to: f32) {
        let from = self.cam().zoom;
        self.zoom_to(to, Self::centre());
        self.zoom_to(from, Self::centre());
    }
}

fn layout_of(h: &Harness) -> String {
    h.view.arrangement().to_text()
}

#[test]
fn zoom_out_draws_blocks_then_lines_then_sections() {
    let mut h = Harness::new(SRC);
    assert_eq!(h.view.zoom_level(), Level::Near);
    let near = h.view.shown().nodes.len();
    let at = Harness::centre();
    h.zoom_to(0.5, at);
    assert_eq!(h.view.zoom_level(), Level::Middle);
    let lines = h.folded();
    // One line per statement at the top: the assignment, the IF, the CASE and the last one.
    assert!(lines.iter().any(|l| l.starts_with("motor := ")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("IF other THEN")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("CASE state OF")), "{lines:?}");
    assert!(h.view.shown().nodes.len() < near);
    let middle = h.view.shown().nodes.len();
    h.zoom_to(0.2, at);
    assert_eq!(h.view.zoom_level(), Level::Far);
    let far = h.folded();
    assert!(
        far.iter().any(|l| l == "Motor") && far.iter().any(|l| l == "Sequence"),
        "{far:?}"
    );
    assert!(h.view.shown().nodes.len() < middle);
    // And back in.
    h.zoom_to(1.0, at);
    assert_eq!(h.view.zoom_level(), Level::Near);
    assert_eq!(h.view.shown().nodes.len(), near);
}

#[test]
fn a_zoom_resting_at_a_threshold_does_not_flip_back_and_forth() {
    let mut h = Harness::new(SRC);
    let at = Harness::centre();
    h.zoom_to(0.5, at);
    let before = h.view.shown().nodes.len();
    // Wobble across the middle of the dead band at the near threshold.
    let centre = f32::midpoint(
        helmsman_diagram::fold::zoom::NEAR_BELOW,
        helmsman_diagram::fold::zoom::NEAR_ABOVE,
    );
    h.zoom_to(centre, at);
    let level = h.view.zoom_level();
    for k in 0..30 {
        let wobble = if k % 2 == 0 { 1.02 } else { 1.0 / 1.02 };
        h.frame(vec![Event::PointerMoved(at), Event::Zoom(wobble)]);
        assert_eq!(h.view.zoom_level(), level, "frame {k} at zoom {}", h.cam().zoom);
    }
    assert_eq!(level, Level::Middle);
    assert_eq!(h.view.shown().nodes.len(), before);
}

#[test]
fn zoom_never_writes_the_layout_or_counts_as_an_edit() {
    let mut h = Harness::new(SRC);
    let (text, edits) = (layout_of(&h), h.view.edits);
    assert!(h.view.arrangement().is_empty());
    h.sweep(0.2);
    h.sweep(0.5);
    h.sweep(0.2);
    assert_eq!(layout_of(&h), text);
    assert_eq!(h.view.edits, edits);
    assert!(h.view.arrangement().is_empty());
}

#[test]
fn a_section_the_reader_folded_stays_folded_at_every_level_and_is_persisted() {
    let mut h = Harness::new(SRC);
    let sections: Vec<String> = h.view.folds[0]
        .iter()
        .filter(|f| f.kind == helmsman_diagram::fold::FoldKind::Section)
        .map(|f| f.key.clone())
        .collect();
    assert_eq!(sections.len(), 2);
    h.view.set_fold(&sections[0], false);
    let text = layout_of(&h);
    assert!(text.contains("collapse"), "{text}");
    for z in [1.0, 0.5, 0.2, 0.5, 1.0] {
        let at = Harness::centre();
        h.zoom_to(z, at);
        assert!(
            h.folded().iter().any(|l| l == "Motor"),
            "at {z}: {:?}",
            h.folded()
        );
        assert_eq!(layout_of(&h), text, "at {z}");
    }
}

#[test]
fn a_fold_the_reader_opened_at_the_far_level_stays_open_through_a_sweep() {
    let mut h = Harness::new(SRC);
    let at = Harness::centre();
    h.zoom_to(0.2, at);
    assert_eq!(h.view.zoom_level(), Level::Far);
    let key = h.view.folds[0]
        .iter()
        .find(|f| f.title == "Motor")
        .unwrap()
        .key
        .clone();
    h.view.set_fold(&key, true);
    let text = layout_of(&h);
    assert!(!h.folded().iter().any(|l| l == "Motor"));
    // Its statements are lines at the far level, and the other section is a block.
    assert!(h.folded().iter().any(|l| l == "Sequence"));
    for z in [0.5, 1.0, 0.5, 0.2] {
        h.zoom_to(z, at);
        assert!(
            !h.folded().iter().any(|l| l == "Motor"),
            "at {z}: {:?}",
            h.folded()
        );
        assert_eq!(layout_of(&h), text, "at {z}");
    }
}

#[test]
fn an_opened_line_shows_its_blocks_and_a_tab_folds_it_back() {
    let mut h = Harness::new(SRC);
    let at = Harness::centre();
    h.zoom_to(0.5, at);
    let key = h.view.folds[0]
        .iter()
        .find(|f| f.title.starts_with("IF other"))
        .unwrap()
        .key
        .clone();
    let tabs = |h: &Harness| h.view.opened_tabs(h.cam()).len();
    // The sections are outlines; no line has a tab.
    let outlines = tabs(&h);
    assert!(!h.view.opened_tabs(h.cam()).iter().any(|t| t.0 == key));
    h.view.set_fold(&key, true);
    assert!(
        h.view.opened_tabs(h.cam()).iter().any(|t| t.0 == key),
        "a tab to fold it back"
    );
    h.zoom_to(1.0, at);
    // At the near level it is still the reader's: outlined with its tab.
    assert!(h.view.opened_tabs(h.cam()).iter().any(|t| t.0 == key));
    // The lines of the other statements have none.
    assert_eq!(tabs(&h), outlines + 1);
    // Folded back by hand, it stays one line at any zoom, and that is what the layout keeps.
    h.view.set_fold(&key, false);
    assert!(layout_of(&h).contains(&key));
    h.zoom_to(1.0, at);
    assert!(h.folded().iter().any(|l| l.starts_with("IF other")));
}

#[test]
fn the_diagram_under_the_pointer_stays_under_it_when_the_level_changes() {
    let mut h = Harness::new(SRC);
    // Whatever is drawn for the first section's statements, found by what it is.
    let motor: Vec<_> = h.view.folds[0]
        .iter()
        .find(|f| f.title == "Motor")
        .unwrap()
        .nodes
        .iter()
        .copied()
        .collect();
    let screen_of = |h: &Harness| {
        let r = h.view.drawn_bounds(&motor).unwrap();
        h.cam().rect(&r)
    };
    for (to, level) in [
        (0.5, Level::Middle),
        (0.2, Level::Far),
        (0.5, Level::Middle),
        (1.0, Level::Near),
    ] {
        // Point at the middle of what is drawn for them now.
        let at = screen_of(&h).center();
        h.zoom_to(to, at);
        assert_eq!(h.view.zoom_level(), level);
        let now = screen_of(&h);
        // What was under the pointer is still under it (the block may be
        // larger or smaller, being one of a fold).
        assert!(now.expand(2.0).contains(at), "{to}: {at:?} not in {now:?}");
    }
}

#[test]
fn q_and_e_still_step_through_stops_at_each_level_without_changing_it() {
    let mut h = Harness::new(SRC);
    let at = Harness::centre();
    for (z, level) in [(1.0, Level::Near), (0.5, Level::Middle), (0.2, Level::Far)] {
        h.zoom_to(z, at);
        assert_eq!(h.view.zoom_level(), level);
        assert!(!h.view.stops().is_empty(), "{level:?}");
        let mut picks = Vec::new();
        for _ in 0..3 {
            h.frame(vec![
                Event::PointerMoved(at),
                Event::Key {
                    key: egui::Key::E,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            // Let the glide finish.
            for _ in 0..40 {
                h.frame(vec![Event::PointerMoved(at)]);
            }
            assert_eq!(h.view.zoom_level(), level, "a jump keeps the level");
            picks.push(h.view.pick);
        }
        assert!(picks.iter().all(Option::is_some), "{level:?}");
        assert!(picks.windows(2).any(|w| w[0] != w[1]), "{level:?}: {picks:?}");
        assert!(matches!(picks[0], Some(Pick::Node(_) | Pick::Region(_))));
    }
}

#[test]
fn an_empty_program_and_a_single_statement_zoom_without_trouble() {
    for src in [
        "PROGRAM p\nVAR a : BOOL; END_VAR\nEND_PROGRAM\n",
        "PROGRAM p\nVAR a AT %QX0.0 : BOOL; END_VAR\na := TRUE;\nEND_PROGRAM\n",
    ] {
        let mut h = Harness::new(src);
        let at = Harness::centre();
        for z in [0.2, 0.5, 1.0, 0.2, 1.0] {
            h.zoom_to(z, at);
        }
        assert!(h.view.arrangement().is_empty());
    }
}

#[test]
fn a_scope_switch_or_an_edit_keeps_the_viewer_at_a_level_it_can_draw() {
    let mut h = Harness::new(SRC);
    let at = Harness::centre();
    h.zoom_to(0.2, at);
    // A new viewer of changed source (a redeploy) starts at the near level.
    let module = h.session.build.as_ref().unwrap().module.clone().unwrap();
    let v = Viewer::new(SRC, module);
    assert_eq!(v.zoom_level(), Level::Near);
    let _: Cam = v.cams[0];
}

#[test]
fn a_far_title_is_at_the_floor_size_and_inside_its_block_down_to_the_minimum_zoom() {
    use helmsman_diagram::fold::zoom::FAR_FLOOR_PT;
    let mut h = Harness::new(SRC);
    for z in [0.29, 0.2, 0.15] {
        h.zoom_to(z, Harness::centre());
        assert_eq!(h.view.zoom_level(), Level::Far, "zoom {z}");
        let titles = h.view.far_titles();
        let names: Vec<&str> = titles.iter().map(|t| t.text.as_str()).collect();
        assert!(
            names.contains(&"Motor") && names.contains(&"Sequence"),
            "{names:?}"
        );
        for t in &titles {
            assert!(t.size >= FAR_FLOOR_PT, "{t:?} at {z}");
            // A monospace character is about 0.6 em: the title must fit.
            let wide = t.text.chars().count() as f32 * t.size * 0.6;
            assert!(wide <= t.block.width(), "{t:?} at {z}: {wide} wide");
            assert!(t.size * 1.2 <= t.block.height(), "{t:?} at {z}");
        }
        // The blocks do not run into each other.
        for (i, a) in titles.iter().enumerate() {
            for b in &titles[i + 1..] {
                assert!(!a.block.intersects(b.block), "{a:?} {b:?} at {z}");
            }
        }
    }
}

#[test]
fn the_far_level_magnification_is_left_behind_going_in_and_is_no_edit() {
    let mut h = Harness::new(SRC);
    let near = h.view.shown().size;
    h.zoom_to(0.2, Harness::centre());
    assert_ne!(h.view.shown().size, near);
    h.zoom_to(1.0, Harness::centre());
    assert_eq!(h.view.shown().size, near);
    assert!(!h.view.rearranged());
}
