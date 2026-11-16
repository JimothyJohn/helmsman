//! Folds on the canvas: several blocks drawn as one, opened and folded
//! back, with picks and drags landing on the diagram underneath.

use eframe::egui::{self, Event, PointerButton, Pos2, RawInput, Rect, Vec2};
use helmsman_diagram::fold::{Fold, FoldKind};
use helmsman_diagram::{NodeId, NodeKind};

use super::{Arrangement, Pick, Scan, Viewer};
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

#[test]
fn a_comment_over_the_whole_program_outlines_nothing() {
    let src = PLANT
        .replace("(* Cooling *)\n", "")
        .replace("(* Counting *)\n", "")
        .replace(
            "motor := (start OR motor)",
            "(* The plant *)\nmotor := (start OR motor)",
        );
    let h = Harness::of(&src);
    assert_eq!(h.view.opened_tabs(h.view.cams[h.view.scope]).len(), 0);
    assert_eq!(h.folded(), ["MOTOR"]);
}

// ---- fold state kept in `diagram.layout` ----

fn key_of(h: &Harness, kind: FoldKind) -> String {
    h.view.folds[0]
        .iter()
        .find(|f| f.kind == kind)
        .unwrap()
        .key
        .clone()
}

fn pou(h: &Harness) -> String {
    h.view.module.pous[0].name.clone()
}

/// What a reader does to PLANT: opens the latch, folds the Cooling
/// section and drags it.
fn leave_it_changed(h: &mut Harness) {
    h.view.set_fold(&key_of(h, FoldKind::Latch), true);
    let section = h.view.folds[0]
        .iter()
        .find(|f| f.kind == FoldKind::Section && f.title == "Cooling")
        .unwrap()
        .key
        .clone();
    h.view.set_fold(&section, false);
    let shown = h
        .view
        .shown()
        .nodes
        .iter()
        .find(|n| n.label == "Cooling")
        .unwrap()
        .id;
    h.view.move_shown(shown, Vec2::new(30.0, 12.0));
}

fn cooling(h: &Harness) -> (f32, f32) {
    let r = h
        .view
        .shown()
        .nodes
        .iter()
        .find(|n| n.label == "Cooling")
        .unwrap()
        .rect;
    (r.x, r.y)
}

#[test]
fn a_fresh_viewer_shows_the_folds_the_last_one_left() {
    let mut a = Harness::of(PLANT);
    // Where the folded section sits when only folded.
    let before = {
        let mut h = Harness::of(PLANT);
        let key = key_of(&h, FoldKind::Section);
        let title = h.view.folds[0]
            .iter()
            .find(|f| f.key == key)
            .unwrap()
            .title
            .clone();
        assert_eq!(title, "Cooling");
        h.view.set_fold(&key, false);
        h.view.set_fold(&key_of(&h, FoldKind::Latch), true);
        cooling(&h)
    };
    leave_it_changed(&mut a);
    let text = a.view.arrangement().to_text();
    assert!(text.starts_with("helmsman-layout 2\n"), "{text}");
    let p = pou(&a);
    assert!(text.contains(&format!("open\t{p}\t")), "{text}");
    assert!(text.contains(&format!("collapse\t{p}\t")), "{text}");
    assert!(text.contains(&format!("foldmove\t{p}\t")), "{text}");

    let mut b = Harness::of(PLANT);
    b.view.arrange(&Arrangement::from_text(&text).unwrap());
    b.frame(vec![]);
    assert_eq!(b.folded(), a.folded());
    assert!(labels(&b).contains(&"OR".to_string()), "the latch is open");
    let (x, y) = (cooling(&b).0 - before.0, cooling(&b).1 - before.1);
    assert!((x - 30.0).abs() < 0.01 && (y - 12.0).abs() < 0.01, "{x} {y}");
    assert_eq!(cooling(&b), cooling(&a));
    assert_eq!(
        b.view.arrangement().to_text(),
        text,
        "and it writes the same back"
    );
}

#[test]
fn a_viewer_nobody_folded_writes_a_version_1_file() {
    let mut h = Harness::of(PLANT);
    assert_eq!(h.view.arrangement().to_text(), "helmsman-layout 1\n");
    let latch = key_of(&h, FoldKind::Latch);
    h.view.set_fold(&latch, true);
    h.view.set_fold(&latch, false);
    assert_eq!(
        h.view.arrangement().to_text(),
        "helmsman-layout 1\n",
        "back where it started"
    );
}

#[test]
fn a_fold_key_the_program_lacks_is_dropped_on_load_and_not_written_back() {
    let h = Harness::of(PLANT);
    let p = pou(&h);
    let (latch, section) = (key_of(&h, FoldKind::Latch), key_of(&h, FoldKind::Section));
    let file = format!(
        "helmsman-layout 2\nopen\t{p}\tgone\nopen\tNOPE\t{latch}\ncollapse\t{p}\tgone\nfoldmove\t{p}\tgone\t5\t5\nopen\t{p}\t{section}\ncollapse\t{p}\t{latch}\n"
    );
    let a = Arrangement::from_text(&file).unwrap();
    let mut b = Harness::of(PLANT);
    b.view.arrange(&a);
    b.frame(vec![]);
    assert_eq!(
        b.view.arrangement().to_text(),
        "helmsman-layout 1\n",
        "unknown keys, another POU, and a state the fold does not have"
    );
    assert_eq!(b.folded(), Harness::of(PLANT).folded());
}

/// A browser's storage two page loads share.
#[derive(Clone, Default)]
struct Kept(std::rc::Rc<std::cell::RefCell<Option<String>>>);

impl crate::sim_projects::Storage for Kept {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.0.borrow().clone())
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(text.to_string());
        Ok(())
    }
}

#[test]
fn folds_survive_a_reload_as_tweaks_and_a_save_as_the_shared_default() {
    let store = Kept::default();
    let mut a = Harness::of(PLANT);
    let mut layers = crate::arrange::Layers::load(Box::new(store.clone()));
    layers.show(&mut a.view, "k", "", true);
    leave_it_changed(&mut a);
    layers.track(&a.view);
    assert!(layers.tweaked());

    // A page load later: the same tweaks, no shared default.
    let mut b = Harness::of(PLANT);
    let mut again = crate::arrange::Layers::load(Box::new(store));
    again.show(&mut b.view, "k", "", true);
    b.frame(vec![]);
    assert_eq!(b.folded(), a.folded());
    assert_eq!(cooling(&b), cooling(&a));

    // Saved as the default, a third page starts there with no tweaks, and
    // a reader who folds the latch back leaves only that as a tweak.
    let default = a.view.arrangement().to_text();
    let mut c = Harness::of(PLANT);
    let mut third = crate::arrange::Layers::load(Box::new(Kept::default()));
    third.show(&mut c.view, "k", &default, true);
    c.frame(vec![]);
    assert_eq!(c.folded(), a.folded());
    assert_eq!(cooling(&c), cooling(&a));
    assert!(!third.tweaked());
    c.view.set_fold(&key_of(&c, FoldKind::Latch), false);
    third.track(&c.view);
    assert!(third.tweaked());
    assert!(c.folded().contains(&"MOTOR".to_string()));
}

/// A card's tap with the output's write inside a closed fold lights the
/// fold that stands for it and leaves it closed (the diagram stays as the
/// reader arranged it).
#[test]
fn a_card_whose_write_is_inside_a_closed_fold_lights_the_fold_and_keeps_it_closed() {
    let mut h = Harness::with_section();
    let folds = |h: &Harness| {
        h.view
            .shown()
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Fold { .. }))
            .count()
    };
    let before = folds(&h);
    assert!(before > 0, "the section is drawn as one block");
    h.view.show_output("lamp");
    for _ in 0..60 {
        h.frame(vec![]);
    }
    assert_eq!(folds(&h), before, "no fold opened");
    let on = h.view.highlighted().cloned().expect("the path is lit");
    let seen = h.view.view().expect("a fold is closed").drivers(&on);
    let fold_lit = h
        .view
        .shown()
        .nodes
        .iter()
        .any(|n| matches!(n.kind, NodeKind::Fold { .. }) && seen.node(n.id));
    assert!(fold_lit, "the fold that holds the write is lit");
    let bounds = h.view.lit_bounds(&seen).expect("something to frame");
    let cam = h.view.cams[0];
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
    assert!(screen.contains_rect(cam.rect(&bounds)), "{:?}", cam.rect(&bounds));
}
