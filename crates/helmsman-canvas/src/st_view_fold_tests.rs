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
        let mut session = Session::new(SRC);
        session.step();
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        let mut h = Self {
            ctx: egui::Context::default(),
            session,
            view: Viewer::new(SRC, module),
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
        h.view.folds[0].push(fold);
        h.view.reshow();
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

    /// The header of the block drawn with `label`, on screen.
    fn header(&self, label: &str) -> Pos2 {
        let v = &self.view;
        let n = v
            .shown()
            .nodes
            .iter()
            .find(|n| n.label == label)
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
fn an_opened_fold_stays_open_across_a_new_version_of_the_program() {
    let mut h = Harness::with_section();
    h.view.set_fold("sec", true);
    let module = h.session.build.as_ref().unwrap().module.clone().unwrap();
    let mut next = Viewer::new(SRC, module);
    next.carry_view(&h.view, &[]);
    assert!(next.opened.contains(&("LINE".to_string(), "sec".to_string())));
}
