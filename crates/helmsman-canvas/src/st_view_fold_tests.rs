//! Folds on the canvas: several blocks drawn as one, opened and folded
//! back, with picks and drags landing on the diagram underneath.

use eframe::egui::{self, Event, PointerButton, Pos2, RawInput, Rect, Vec2};
use helmsman_diagram::fold::{Fold, FoldKind};
use helmsman_diagram::{NodeId, NodeKind};

use super::{Pick, Scan, Viewer};
use crate::playground::Session;

const SRC: &str = "PROGRAM line
VAR
  start AT %IX0.0 : BOOL;
  other AT %IX0.2 : BOOL;
  motor AT %QX0.0 : BOOL;
  lamp AT %QX0.1 : BOOL;
  count : INT;
END_VAR
motor := start AND count > 10;
lamp := other;
IF other THEN count := count + 1; END_IF;
END_PROGRAM
";

struct Harness {
    ctx: egui::Context,
    session: Session,
    view: Viewer,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        Self::of(SRC)
    }

    fn of(src: &str) -> Self {
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

    /// The same, with the last two statements as one fold.
    fn with_section() -> Self {
        let mut h = Self::new();
        let d = h.view.diagram().clone();
        let fold = Fold::over_stmts(&d, "sec", FoldKind::Section, "Lamp and count", &d.top[1..]);
        // A section starts as its blocks: fold it, as its tab would.
        h.view.folds[0].push(fold);
        h.view.set_fold("sec", false);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
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

    fn press(&mut self, at: Pos2, times: usize) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![Event::PointerMoved(at)]);
        for _ in 0..times {
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
        }
        // Far enough from the next press not to join it.
        self.time += 1.0;
        self.frame(vec![]);
    }

    /// The labels of the folds drawn as one block.
    fn folded(&self) -> Vec<String> {
        self.view
            .shown()
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Fold { .. }))
            .map(|n| n.label.clone())
            .collect()
    }

    /// The header of the block drawn with `label`, on screen: the fold
    /// of that name, or else the first block.
    fn header(&self, label: &str) -> Pos2 {
        let v = &self.view;
        let named = || v.shown().nodes.iter().filter(|n| n.label == label);
        let n = named()
            .find(|n| matches!(n.kind, NodeKind::Fold { .. }))
            .or_else(|| named().next())
            .unwrap_or_else(|| panic!("no block {label} drawn"));
        let r = v.cams[v.scope].rect(&n.rect);
        Pos2::new(r.center().x, r.min.y + 8.0 * v.cams[v.scope].zoom)
    }
}

#[test]
fn a_comparison_is_drawn_with_its_literal_and_every_variable_keeps_its_block() {
    let h = Harness::new();
    assert_eq!(h.folded(), ["> 10"]);
    let shown = h.view.shown();
    assert!(
        !shown
            .nodes
            .iter()
            .any(|n| matches!(n.kind, NodeKind::Const { .. } if n.label == "10"))
    );
    assert_eq!(shown.nodes.len() + 1, h.view.diagram().nodes.len());
    // The inputs' controls go under the variables' blocks: all are drawn.
    let reads = h.view.reads_on_screen();
    let base = h
        .view
        .diagram()
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Read { .. }))
        .count();
    assert_eq!(reads.len(), base);
    assert!(reads.iter().all(|(_, r)| r.is_positive()), "{reads:?}");
}

#[test]
fn a_fold_opens_on_a_double_click_and_its_tab_folds_it_back() {
    let mut h = Harness::with_section();
    assert_eq!(h.folded(), ["> 10", "Lamp and count"]);
    assert!(h.view.shown().regions.is_empty(), "the IF is inside the fold");
    let at = h.header("Lamp and count");
    h.press(at, 2);
    assert_eq!(h.folded(), ["> 10"], "its blocks are drawn again");
    assert_eq!(h.view.shown().regions.len(), 1);
    // The tab over its blocks folds it back.
    let cam = h.view.cams[h.view.scope];
    let tabs = h.view.opened_tabs(cam);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].1, "Lamp and count");
    let tab = tabs[0].3;
    h.press(tab.center(), 2);
    assert_eq!(h.folded(), ["> 10", "Lamp and count"]);
    assert_eq!(h.view.opened_tabs(h.view.cams[h.view.scope]).len(), 0);
}

#[test]
fn a_click_on_a_fold_picks_a_block_of_the_diagram_under_it() {
    let mut h = Harness::with_section();
    let at = h.header("Lamp and count");
    h.press(at, 1);
    let Some(Pick::Node(n)) = h.view.pick else {
        panic!("a block is picked: {:?}", h.view.pick);
    };
    let d = h.view.diagram();
    let inside: Vec<NodeId> = d
        .nodes
        .iter()
        .filter(|m| d.stmts[d.top[0]].id != m.stmt)
        .map(|m| m.id)
        .collect();
    assert!(inside.contains(&n), "{:?} is one of the fold's blocks", d.node(n));
    assert!(h.view.picked().is_some(), "the editor has a line to go to");
    // A block outside the fold is itself.
    let at = h.header("START");
    h.press(at, 1);
    let Some(Pick::Node(n)) = h.view.pick else {
        panic!("a block is picked");
    };
    assert_eq!(h.view.diagram().node(n).label, "START");
}

#[test]
fn a_dragged_fold_stays_where_it_was_put() {
    let mut h = Harness::with_section();
    let rect = |h: &Harness| {
        h.view
            .shown()
            .nodes
            .iter()
            .find(|n| n.label == "Lamp and count")
            .unwrap()
            .rect
    };
    let before = rect(&h);
    let shown = h
        .view
        .shown()
        .nodes
        .iter()
        .find(|n| n.label == "Lamp and count")
        .unwrap()
        .id;
    h.view.move_shown(shown, Vec2::new(40.0, 25.0));
    let after = rect(&h);
    assert!((after.x - before.x - 40.0).abs() < 0.01 && (after.y - before.y - 25.0).abs() < 0.01);
    assert!(h.view.rearranged());
    // Moving another block rebuilds what is drawn: the fold keeps its place.
    let start = h.view.diagram().nodes[0].id;
    h.view.move_node(start, Vec2::new(-5.0, 0.0));
    assert_eq!(rect(&h), after);
    h.view.reset_arrangement();
    assert_eq!(rect(&h), before);
}

#[test]
fn what_the_reader_folded_stays_folded_across_a_new_version_of_the_program() {
    let h = Harness::with_section();
    let module = h.session.build.as_ref().unwrap().module.clone().unwrap();
    let mut next = Viewer::new(SRC, module);
    next.carry_view(&h.view, &[]);
    assert!(next.collapsed.contains(&("LINE".to_string(), "sec".to_string())));
}

/// A latch, a commented section and an IF with a two-statement arm.
const PLANT: &str = "PROGRAM plant
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  hot AT %IX0.2 : BOOL;
  motor AT %QX0.0 : BOOL;
  fan AT %QX0.1 : BOOL;
  horn AT %QX0.2 : BOOL;
  count : INT;
END_VAR
motor := (start OR motor) AND NOT stop;
(* Cooling *)
fan := motor AND hot;
horn := hot AND NOT motor;
(* Counting *)
IF motor THEN
  count := count + 1;
  horn := FALSE;
END_IF;
END_PROGRAM
";

fn labels(h: &Harness) -> Vec<String> {
    h.view.shown().nodes.iter().map(|n| n.label.clone()).collect()
}

#[test]
fn a_latch_opens_as_one_block_with_set_and_reset_and_its_inputs_beside_it() {
    let mut h = Harness::of(PLANT);
    assert_eq!(
        h.folded(),
        ["MOTOR"],
        "the latch; sections and arms start as their blocks"
    );
    let shown = h.view.shown();
    let latch = shown
        .nodes
        .iter()
        .find(|n| matches!(n.kind, NodeKind::Fold { .. }) && n.label == "MOTOR")
        .unwrap();
    assert_eq!(latch.ins, ["set", "reset"]);
    assert!(!labels(&h).contains(&"OR".to_string()));
    // The inputs keep their blocks, so their controls have a place.
    let reads = h.view.reads_on_screen();
    for input in ["START", "STOP", "HOT"] {
        assert!(
            reads.iter().any(|(p, r)| p == input && r.is_positive()),
            "{input}: {reads:?}"
        );
    }
    // A right click on its reset pin is a gesture on the AND's pin.
    let d = h.view.diagram().clone();
    let and = d
        .nodes
        .iter()
        .find(|n| matches!(&n.kind, NodeKind::Op { op } if op == "AND"))
        .unwrap()
        .id;
    let at = h.view.pin_on_screen(and, 1).expect("the reset pin is drawn");
    let cam = h.view.cams[h.view.scope];
    assert_eq!(
        h.view.hit_at(cam, at),
        Some(crate::diagram_edit::Hit::Pin { node: and, input: 1 })
    );
    // A double click shows its gates, and its tab folds them again.
    let at = h.header("MOTOR");
    h.press(at, 2);
    assert!(labels(&h).contains(&"OR".to_string()));
    let tab = h
        .view
        .opened_tabs(h.view.cams[h.view.scope])
        .into_iter()
        .find(|t| t.1 == "MOTOR")
        .expect("a tab over the opened latch")
        .3;
    h.press(tab.center(), 2);
    assert!(!labels(&h).contains(&"OR".to_string()));
}

#[test]
fn a_commented_section_is_outlined_by_name_and_folds_from_its_tab() {
    let mut h = Harness::of(PLANT);
    let tabs = h.view.opened_tabs(h.view.cams[h.view.scope]);
    let names: Vec<&str> = tabs.iter().map(|t| t.1.as_str()).collect();
    assert_eq!(names, ["Cooling", "Counting"]);
    let tab = tabs[0].3;
    h.press(tab.center(), 2);
    assert_eq!(h.folded(), ["MOTOR", "Cooling"]);
    let shown = h.view.shown();
    let fold = shown.nodes.iter().find(|n| n.label == "Cooling").unwrap();
    assert_eq!(fold.outs, ["FAN", "HORN"]);
    // What it reads comes in on wires: the latch's output (once as it is,
    // once through a bubble) and HOT's block.
    let from: Vec<(&str, bool)> = shown
        .inputs(fold.id)
        .map(|w| (shown.node(w.from).label.as_str(), w.negated))
        .collect();
    assert_eq!(from, [("MOTOR :=", false), ("HOT", false), ("MOTOR :=", true)]);
    // HOT is read by both statements: one block for it, and its control
    // has that block to sit under.
    let reads = h.view.reads_on_screen();
    let hot: Vec<bool> = reads
        .iter()
        .filter(|(p, _)| p == "HOT")
        .map(|(_, r)| r.is_positive())
        .collect();
    assert_eq!(hot, [true, false]);
    // The other section still has its tab; this one opens on a double click.
    let names: Vec<String> = h
        .view
        .opened_tabs(h.view.cams[h.view.scope])
        .into_iter()
        .map(|t| t.1)
        .collect();
    assert_eq!(names, ["Counting"]);
    let at = h.header("Cooling");
    h.press(at, 2);
    assert_eq!(h.folded(), ["MOTOR"]);
}

#[test]
fn an_arm_folds_from_its_label_and_opens_from_its_block() {
    let mut h = Harness::of(PLANT);
    let cam = h.view.cams[h.view.scope];
    let r = h.view.shown().regions[0].clone();
    assert_eq!(r.label, "IF MOTOR");
    let label = cam.to_screen(r.rect.x + 30.0, r.rect.y + 8.0);
    h.press(label, 2);
    assert_eq!(h.folded(), ["MOTOR", "IF MOTOR"]);
    let shown = h.view.shown();
    assert_eq!(
        shown.regions.len(),
        1,
        "the arm's box stays, with one block in it"
    );
    let fold = shown.nodes.iter().find(|n| n.label == "IF MOTOR").unwrap();
    assert_eq!(fold.outs, ["COUNT", "HORN"]);
    let at = h.header("IF MOTOR");
    h.press(at, 2);
    assert_eq!(h.folded(), ["MOTOR"]);
    assert_eq!(h.view.shown().regions.len(), 1);
}
