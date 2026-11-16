//! Folds on the canvas: several blocks drawn as one, opened and folded
//! back, with picks and drags landing on the diagram underneath.

use eframe::egui::{self, Event, PointerButton, Pos2, RawInput, Rect, Vec2};
use helmsman_diagram::fold::{Fold, FoldKind};
use helmsman_diagram::{NodeId, NodeKind};

use super::{Arrangement, Cam, Pick, Scan, Viewer};
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

// ---- a CASE on a state variable ------------------------------------------

/// A sequence: the scan sits in state 1 (`1, 2`).
const SEQ: &str = "PROGRAM seq
VAR
  step : INT := 1;
  go AT %IX0.0 : BOOL;
  red AT %QX0.0 : BOOL;
  green AT %QX0.1 : BOOL;
END_VAR
CASE step OF
  0: red := TRUE;
  1, 2: green := TRUE; red := FALSE;
  3..5: step := 0;
ELSE
  step := 0;
END_CASE;
END_PROGRAM
";

/// A state block's rows: the label and whether the arm ran.
type Rows = Vec<(String, Option<bool>)>;

impl Harness {
    /// The state block drawn: its block and what is drawn for its rows.
    fn states(&self) -> Option<(NodeId, Rows)> {
        let live = self
            .view
            .live(self.session.trace.as_ref().map(|t| t as &dyn Scan));
        let rows = self.view.state_rows(live.as_ref());
        let (n, rows) = rows.into_iter().next()?;
        Some((n, rows.into_iter().map(|r| (r.label, r.on)).collect()))
    }

    /// The middle of the chip of the state labelled `label`, on screen.
    fn chip(&self, label: &str) -> Pos2 {
        let (n, rows) = self.states().expect("a state block is drawn");
        let k = rows.iter().position(|(l, _)| l == label).expect(label);
        let node = self.view.shown().node(n);
        let cam = self.view.cams[self.view.scope];
        cam.rect(&helmsman_diagram::layout::body_row(node, k)).center()
    }
}

#[test]
fn a_case_on_a_state_variable_is_drawn_as_one_block_of_states() {
    let h = Harness::of(SEQ);
    assert_eq!(h.folded(), ["STEP"]);
    assert!(h.view.shown().regions.is_empty(), "the arms are inside the block");
    let (_, rows) = h.states().unwrap();
    let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["0", "1, 2", "3..5", "ELSE"]);
    // The variable keeps its block, to set and watch.
    assert!(
        h.view
            .shown()
            .nodes
            .iter()
            .any(|n| matches!(&n.kind, NodeKind::Read { path } if path == "STEP"))
    );
}

#[test]
fn the_state_the_scan_is_in_is_the_one_lit() {
    let h = Harness::of(SEQ);
    let (_, rows) = h.states().unwrap();
    let lit: Vec<&str> = rows
        .iter()
        .filter(|(_, on)| *on == Some(true))
        .map(|(l, _)| l.as_str())
        .collect();
    assert_eq!(lit, ["1, 2"]);
    assert!(rows.iter().all(|(_, on)| on.is_some()));
    // No scan: nothing is lit, nothing is claimed.
    let rows = h.view.state_rows(None);
    assert!(rows[0].1.iter().all(|r| r.on.is_none()));
}

#[test]
fn a_state_no_arm_has_lights_nothing() {
    let src = SEQ
        .replace("step : INT := 1;", "step : INT := 9;")
        .replace("ELSE\n  step := 0;\n", "");
    let h = Harness::of(&src);
    let (_, rows) = h.states().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|(_, on)| *on == Some(false)), "{rows:?}");
}

#[test]
fn the_values_on_the_block_are_the_diagrams_own() {
    let h = Harness::of(SEQ);
    let live = h
        .view
        .live(h.session.trace.as_ref().map(|t| t as &dyn Scan))
        .unwrap();
    let v = h.view.view().unwrap();
    let seen = v.live(&live);
    let d = h.view.diagram();
    let mut checked = 0;
    for n in &d.nodes {
        for (out, value) in live.values[n.id.0 as usize].iter().enumerate() {
            if let Some((to, k)) = v.port(n.id, out) {
                assert_eq!(seen.values[to.0 as usize][k], *value, "{}", n.text);
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

#[test]
fn a_tap_on_a_state_opens_the_sequence_on_that_state() {
    let mut h = Harness::of(SEQ);
    let arm = h.view.diagram().stmts[0].arms[2];
    let at = h.chip("3..5");
    h.press(at, 1);
    assert_eq!(h.folded(), Vec::<String>::new(), "its arms are drawn again");
    assert_eq!(h.view.shown().regions.len(), 4);
    assert_eq!(h.view.pick, Some(Pick::Region(arm)));
    // Values are those of the diagram it opened into.
    assert_eq!(h.view.shown(), h.view.diagram());
    // The view eases over to the arm; let it arrive.
    for _ in 0..3 {
        h.time += 2.0;
        h.frame(vec![]);
    }
    assert!(h.view.glide.is_none());
    // The tab over it folds it back, and a state can be tapped again.
    let tabs = h.view.opened_tabs(h.view.cams[h.view.scope]);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].1, "STEP");
    h.press(tabs[0].3.center(), 2);
    assert_eq!(
        h.folded(),
        ["STEP"],
        "{:?} {:?}",
        tabs[0],
        h.view.cams[h.view.scope]
    );
    h.press(h.chip("ELSE"), 1);
    let last = h.view.diagram().stmts[0].arms[3];
    assert_eq!(h.view.pick, Some(Pick::Region(last)));
}

/// Where the view is going after a tap: the arm's box on screen and the
/// zoom, at the end of the glide.
fn aimed(h: &Harness, arm: helmsman_diagram::RegionId) -> (Rect, f32, Option<Rect>) {
    let g = h.view.glide.as_ref().expect("the view eases over to the arm");
    let cam = Cam {
        offset: g.to.0,
        zoom: g.to.1,
        ..h.view.cams[h.view.scope]
    };
    let shown = h.view.view().map_or(Some(arm), |v| v.region_shown(arm)).unwrap();
    let rect = cam.rect(&h.view.shown().region(shown).rect);
    let tab = h.view.opened_tabs(cam).into_iter().next().map(|(.., tab)| tab);
    (rect, g.to.1, tab)
}

/// The part of the view the diagram is drawn in.
fn visible(h: &Harness) -> Rect {
    let cam = h.view.cams[h.view.scope];
    Rect::from_min_size(cam.origin, cam.area)
}

#[test]
fn after_a_tap_the_arm_is_whole_on_screen_with_the_tab_and_no_closer_than_before() {
    for zoom in [0.8, 1.0, 1.6] {
        let mut h = Harness::of(SEQ);
        h.view.cams[h.view.scope].zoom = zoom;
        h.frame(vec![]);
        let before = h.view.cams[h.view.scope].zoom;
        let arm = h.view.diagram().stmts[0].arms[2];
        let at = h.chip("3..5");
        h.press(at, 1);
        // Read the aim before the glide ends (a press ends with idle frames).
        let (rect, to, tab) = aimed_or_arrived(&h, arm);
        let view = visible(&h);
        assert!(to <= before + 1e-3, "zoom {to} after a tap at {before}");
        assert!(view.contains_rect(rect), "{rect:?} not in {view:?} at {before}");
        let tab = tab.expect("the fold's tab");
        assert!(view.contains_rect(tab), "{tab:?} not in {view:?} at {before}");
        // Centred: the middle of the arm is near the middle of the view.
        let off = (rect.center() - view.center()).abs();
        // (the frame centres the arm and its tab together).
        assert!(
            off.x < view.width() / 4.0 && off.y < view.height() / 2.0,
            "{off:?}"
        );
    }
}

/// The aim of the last tap, whether the glide is still going or arrived.
fn aimed_or_arrived(h: &Harness, arm: helmsman_diagram::RegionId) -> (Rect, f32, Option<Rect>) {
    if h.view.glide.is_some() {
        return aimed(h, arm);
    }
    let cam = h.view.cams[h.view.scope];
    let shown = h.view.view().map_or(Some(arm), |v| v.region_shown(arm)).unwrap();
    let tab = h.view.opened_tabs(cam).into_iter().next().map(|(.., t)| t);
    (cam.rect(&h.view.shown().region(shown).rect), cam.zoom, tab)
}

#[test]
fn the_last_of_thirty_states_is_framed_whole_and_not_zoomed_in() {
    use std::fmt::Write as _;
    let mut src = String::from("PROGRAM seq\nVAR step : INT; y : BOOL; END_VAR\nCASE step OF\n");
    for i in 0..30 {
        writeln!(src, "  {i}: y := {};", i % 2 == 0).unwrap();
    }
    src.push_str("END_CASE;\nEND_PROGRAM\n");
    let mut h = Harness::of(&src);
    let before = h.view.cams[h.view.scope].zoom;
    let arm = h.view.diagram().stmts[0].arms[29];
    let at = h.chip("29");
    h.press(at, 1);
    let (rect, to, _) = aimed_or_arrived(&h, arm);
    let view = visible(&h);
    assert!(to <= before + 1e-3, "{to} vs {before}");
    assert!(view.contains_rect(rect), "{rect:?} not in {view:?}");
    assert_eq!(h.view.pick, Some(Pick::Region(arm)));
}

#[test]
fn an_arm_wider_than_the_view_is_zoomed_out_to_the_legible_floor_and_shown_from_its_corner() {
    use std::fmt::Write as _;
    let mut src = String::from(
        "PROGRAM seq\nVAR step : INT; y : ARRAY[0..59] OF BOOL; END_VAR\nCASE step OF\n 0: step := 1;\n 1:\n",
    );
    for i in 0..60 {
        writeln!(src, "  y[{i}] := step > {i};").unwrap();
    }
    src.push_str("END_CASE;\nEND_PROGRAM\n");
    let mut h = Harness::of(&src);
    h.view.cams[h.view.scope].zoom = 2.0;
    h.frame(vec![]);
    let arm = h.view.diagram().stmts[0].arms[1];
    let at = h.chip("1");
    h.press(at, 1);
    let (rect, to, _) = aimed_or_arrived(&h, arm);
    let view = visible(&h);
    assert!((super::LEGIBLE - 1e-3..2.0).contains(&to), "{to}");
    // Whole when it fits at that zoom, else its top left corner in view.
    assert!(
        view.contains_rect(rect) || view.contains(rect.min),
        "{rect:?} not in {view:?}"
    );
}

#[test]
fn a_frame_is_never_closer_than_asked_and_says_whether_it_fits() {
    let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    let small = helmsman_diagram::Rect {
        x: 10.0,
        y: 10.0,
        w: 100.0,
        h: 50.0,
    };
    let mut cam = Cam::default();
    assert!(cam.frame_within(area, &small, 1.0));
    assert!((cam.zoom - 1.0).abs() < 1e-6, "{}", cam.zoom);
    let huge = helmsman_diagram::Rect {
        x: 0.0,
        y: 0.0,
        w: 5000.0,
        h: 300.0,
    };
    assert!(!cam.frame_within(area, &huge, 1.0));
    assert!((cam.zoom - super::LEGIBLE).abs() < 1e-6);
    // Asked for less than the floor, it gives no more than asked.
    cam.frame_within(area, &small, 0.5);
    assert!((cam.zoom - 0.5).abs() < 1e-6);
}

#[test]
fn a_tap_beside_the_states_does_not_open_them() {
    let mut h = Harness::of(SEQ);
    // The header picks, as any block's.
    let at = h.header("STEP");
    h.press(at, 1);
    assert_eq!(h.folded(), ["STEP"]);
    assert!(h.view.chip_at(h.view.cams[h.view.scope], at).is_none());
    // A double click on the block still opens it, as for any fold.
    h.press(at, 2);
    assert_eq!(h.folded(), Vec::<String>::new());
}

#[test]
fn the_jump_keys_stop_at_the_state_block() {
    let h = Harness::of(SEQ);
    let stops = h.view.stops();
    assert_eq!(stops.len(), 1);
    let (n, _) = h.states().unwrap();
    let block = h.view.shown().node(n).rect;
    let r = stops[0].rect;
    assert!(r.x <= block.x && r.y <= block.y && r.right() >= block.right() && r.bottom() >= block.bottom());
}

#[test]
fn a_sequence_of_many_states_grows_the_block_down_not_across() {
    use std::fmt::Write as _;
    let mut src = String::from("PROGRAM seq\nVAR step : INT; y : BOOL; END_VAR\nCASE step OF\n");
    for i in 0..30 {
        writeln!(src, "  {i}: y := {};", i % 2 == 0).unwrap();
    }
    src.push_str("END_CASE;\nEND_PROGRAM\n");
    let h = Harness::of(&src);
    let (n, rows) = h.states().unwrap();
    assert_eq!(rows.len(), 30);
    let r = h.view.shown().node(n).rect;
    assert!(r.h > 30.0 * 16.0 && r.w < 300.0, "{r:?}");
    // The last state is as tappable as the first.
    let cam = h.view.cams[h.view.scope];
    assert!(h.view.chip_at(cam, h.chip("29")).is_some());
    assert!(h.view.chip_at(cam, h.chip("0")).is_some());
}

// ---- an interlock as a table ----

/// An interlock: `run` as three ways, and the signals at `on`.
fn permissive([guard, estop, auto, manual, jog]: [bool; 5]) -> String {
    format!(
        "PROGRAM cell
VAR
  guard : BOOL := {guard};
  estop : BOOL := {estop};
  auto : BOOL := {auto};
  manual : BOOL := {manual};
  jog : BOOL := {jog};
  run : BOOL;
END_VAR
run := guard AND NOT estop AND auto OR manual AND NOT estop OR jog;
END_PROGRAM
"
    )
    .to_uppercase()
}

/// A table block's rows: per row, each cell's label and whether it stops
/// the row, and whether the row holds.
type Grid = Vec<(Vec<(String, bool)>, Option<bool>)>;

impl Harness {
    fn table(&self, live: bool) -> Option<(NodeId, Grid)> {
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        let live = if live { self.view.live(scan) } else { None };
        let (n, rows) = self.view.table_rows(live.as_ref()).into_iter().next()?;
        let grid = rows
            .into_iter()
            .map(|r| (r.cells.into_iter().map(|c| (c.label, c.stops)).collect(), r.holds))
            .collect();
        Some((n, grid))
    }

    /// The middle of cell `j` of row `k` of the table block, on screen.
    fn cell(&self, k: usize, j: usize) -> Pos2 {
        let (n, grid) = self.table(true).expect("a table block is drawn");
        let node = self.view.shown().node(n);
        let cam = self.view.cams[self.view.scope];
        cam.rect(&helmsman_diagram::layout::body_cell(node, k, j, grid[k].0.len()))
            .center()
    }
}

fn stopping(grid: &Grid) -> Vec<Vec<&str>> {
    grid.iter()
        .map(|(cells, _)| {
            cells
                .iter()
                .filter(|(_, s)| *s)
                .map(|(l, _)| l.as_str())
                .collect()
        })
        .collect()
}

#[test]
fn an_or_of_ands_is_drawn_as_one_block_of_rows_of_cells() {
    let h = Harness::of(&permissive([true, false, true, false, false]));
    assert_eq!(h.folded(), ["RUN"]);
    let (n, grid) = h.table(true).unwrap();
    let shape: Vec<Vec<&str>> = grid
        .iter()
        .map(|(c, _)| c.iter().map(|(l, _)| l.as_str()).collect())
        .collect();
    assert_eq!(
        shape,
        [
            vec!["GUARD", "ESTOP", "AUTO"],
            vec!["MANUAL", "ESTOP"],
            vec!["JOG"]
        ]
    );
    // One pin per signal, ESTOP once though two rows need it off.
    let block = h.view.shown().node(n);
    assert_eq!(block.ins, ["GUARD", "ESTOP", "AUTO", "MANUAL", "JOG"]);
    // The signals keep their blocks, and the gates are inside.
    assert!(
        !h.view
            .shown()
            .nodes
            .iter()
            .any(|n| matches!(&n.kind, NodeKind::Op { op } if op == "AND" || op == "OR"))
    );
}

#[test]
fn the_row_that_holds_is_lit_and_nothing_is_marked_while_the_output_is_on() {
    let h = Harness::of(&permissive([true, false, true, false, false]));
    let (_, grid) = h.table(true).unwrap();
    let holds: Vec<Option<bool>> = grid.iter().map(|(_, h)| *h).collect();
    assert_eq!(holds, [Some(true), Some(false), Some(false)]);
    assert!(stopping(&grid).iter().all(Vec::is_empty), "{grid:?}");
}

#[test]
fn with_the_output_off_each_row_marks_exactly_the_cells_that_stop_it() {
    let h = Harness::of(&permissive([false, true, true, true, false]));
    let (_, grid) = h.table(true).unwrap();
    assert!(grid.iter().all(|(_, holds)| *holds == Some(false)));
    assert_eq!(
        stopping(&grid),
        [vec!["GUARD", "ESTOP"], vec!["ESTOP"], vec!["JOG"]],
        "AUTO and MANUAL are as their rows need them"
    );
    // One missing cell per row: each says which.
    let h = Harness::of(&permissive([true, false, false, false, false]));
    let (_, grid) = h.table(true).unwrap();
    assert_eq!(stopping(&grid), [vec!["AUTO"], vec!["MANUAL"], vec!["JOG"]]);
}

#[test]
fn a_row_that_holds_beside_one_that_does_not_marks_nothing() {
    let h = Harness::of(&permissive([false, false, true, true, false]));
    let (_, grid) = h.table(true).unwrap();
    let holds: Vec<Option<bool>> = grid.iter().map(|(_, h)| *h).collect();
    assert_eq!(holds, [Some(false), Some(true), Some(false)]);
    assert!(stopping(&grid).iter().all(Vec::is_empty));
}

#[test]
fn with_no_scan_nothing_is_lit_and_nothing_marked() {
    let h = Harness::of(&permissive([false, true, true, true, false]));
    let (_, grid) = h.table(false).unwrap();
    assert!(grid.iter().all(|(_, holds)| holds.is_none()));
    assert!(stopping(&grid).iter().all(Vec::is_empty));
}

#[test]
fn the_values_on_a_table_block_are_the_diagrams_own() {
    let h = Harness::of(&permissive([true, false, true, false, false]));
    let live = h
        .view
        .live(h.session.trace.as_ref().map(|t| t as &dyn Scan))
        .unwrap();
    let v = h.view.view().unwrap();
    let seen = v.live(&live);
    let d = h.view.diagram();
    let mut checked = 0;
    for n in &d.nodes {
        for (out, value) in live.values[n.id.0 as usize].iter().enumerate() {
            if let Some((to, k)) = v.port(n.id, out) {
                assert_eq!(seen.values[to.0 as usize][k], *value, "{}", n.text);
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

#[test]
fn a_double_click_opens_the_table_into_its_gates_and_its_tab_folds_it_back() {
    let mut h = Harness::of(&permissive([true, false, true, false, false]));
    h.press(h.header("RUN"), 2);
    assert_eq!(h.folded(), Vec::<String>::new());
    assert_eq!(h.view.table_rows(None).len(), 0);
    assert!(
        h.view
            .shown()
            .nodes
            .iter()
            .any(|n| matches!(&n.kind, NodeKind::Op { op } if op == "OR"))
    );
    assert_eq!(h.view.shown(), h.view.diagram());
    let tabs = h.view.opened_tabs(h.view.cams[h.view.scope]);
    assert_eq!(tabs.len(), 1);
    h.press(tabs[0].3.center(), 2);
    assert_eq!(h.folded(), ["RUN"]);
}

#[test]
fn a_cell_says_what_its_row_needs_and_what_the_signal_is() {
    let h = Harness::of(&permissive([false, true, true, true, false]));
    let scan = h.session.trace.as_ref().map(|t| t as &dyn Scan);
    let live = h.view.live(scan);
    let tables = h.view.table_rows(live.as_ref());
    let cam = h.view.cams[h.view.scope];
    let tip = |k, j| h.view.cell_tip(cam, h.cell(k, j), &tables).unwrap().to_string();
    assert_eq!(
        tip(0, 1),
        "ESTOP: this row needs it off. It is TRUE now. This is what stops the row."
    );
    assert_eq!(
        tip(1, 0),
        "MANUAL: this row needs it on. It is TRUE now. It is as the row needs it."
    );
    // Beside the cells, the block says what it is.
    let block = h.view.shown().nodes.iter().find(|n| n.label == "RUN").unwrap();
    let corner = cam.rect(&block.rect).left_top() + Vec2::new(2.0, 2.0);
    assert_eq!(h.view.cell_tip(cam, corner, &tables), None);
}

#[test]
fn the_jump_keys_frame_a_condition_that_is_a_table_and_a_tap_on_a_cell_only_picks() {
    let src = permissive([true, false, true, false, false]).replace(
        "RUN := GUARD AND NOT ESTOP AND AUTO OR MANUAL AND NOT ESTOP OR JOG;",
        "IF GUARD AND NOT ESTOP AND AUTO OR MANUAL AND NOT ESTOP OR JOG THEN RUN := TRUE; AUTO := NOT AUTO; END_IF;",
    );
    let mut h = Harness::of(&src);
    assert_eq!(h.folded(), ["permissive"]);
    let stops = h.view.stops();
    assert_eq!(stops.len(), 1);
    let (n, _) = h.table(true).unwrap();
    let (block, r) = (h.view.shown().node(n).rect, stops[0].rect);
    assert!(
        r.x <= block.x && r.y <= block.y && r.right() >= block.right() && r.bottom() >= block.bottom(),
        "{r:?} does not frame {block:?}"
    );
    // A tap on a cell is a tap on the block: it picks, it never opens.
    h.press(h.cell(0, 0), 1);
    assert_eq!(h.folded(), ["permissive"]);
    assert!(h.view.pick.is_some());
}

#[test]
fn a_gesture_on_a_table_pin_is_refused_not_mapped_to_a_gate() {
    let h = Harness::of(&permissive([true, false, true, false, false]));
    let (n, _) = h.table(true).unwrap();
    for k in 0..h.view.shown().node(n).ins.len() {
        assert_eq!(h.view.base_pin(n, k), None, "pin {k}");
    }
    assert_eq!(h.view.base_block(n), None);
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
