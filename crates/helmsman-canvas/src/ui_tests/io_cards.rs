//! The I/O tab's cards (`crate::io_cards`): what a card says, its details,
//! its two glyphs, the list and the grid, the folding headings, and a
//! push button that is also on the diagram.

use std::fmt::Write as _;

use eframe::egui::Event;

use super::{App, Harness};
use crate::editor::{Chrome, Pane, RIGHT_W};

/// One input read by three outputs and by nothing else of its name; a
/// latch, a timer and a name that only looks like the input's.
const MIXER: &str = "\
PROGRAM mixer
VAR_INPUT start : BOOL; stop : BOOL; start_button : BOOL; level : INT; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; horn : BOOL; spare : BOOL; END_VAR
VAR t1 : TON; END_VAR
(* start runs the motor *)
motor := (start OR motor) AND NOT stop;
t1(IN := start, PT := T#2s);
lamp := t1.Q AND start_button;
horn := start AND level > 3;
END_PROGRAM
";

fn sim_with(src: &str) -> Harness {
    let mut h = Harness::new(App::sim());
    // Each simulated card carries a SIM line: room for the outputs too.
    h.size.y = 1500.0;
    h.app.chrome.dock(Pane::Io, true);
    h.app.sim.as_mut().unwrap().load(src, "test");
    h.wait(0.3);
    h.press("tab io");
    h.wait(0.3);
    h
}

fn double_click(h: &mut Harness, key: &str) {
    // Clicks made just before would make this a triple click.
    h.wait(1.0);
    // Low on the card: under its control and its glyphs.
    let r = h.rect(key);
    let at = eframe::egui::pos2(r.center().x, (r.bottom() - 8.0).min(h.size.y - 4.0));
    h.click(at);
    h.click(at);
    h.wait(0.2);
}

#[test]
fn a_card_says_the_first_link_and_its_details_say_them_all() {
    let mut h = sim_with(MIXER);
    assert!(h.drew("affects"), "an input says what it affects");
    assert!(h.drew("driven by"), "an output says what drives it");
    // The last output's card is under the fold of the inputs.
    h.press("io head inputs");
    h.wait(0.2);
    assert!(h.drew("nothing sets it"), "spare is never assigned");
    h.press("io head inputs");
    h.wait(0.2);
    assert!(
        h.has("io more start"),
        "start affects three outputs: the first and …"
    );
    assert!(!h.has("io more stop"), "stop affects one: no …");
    assert!(!h.drew("named"), "no details until asked");
    assert!(!h.drew("kept"), "the groups are in the details");
    // The details open below the fold of the inputs unless they are folded.
    h.press("io head inputs");
    h.wait(0.2);
    double_click(&mut h, "io card motor");
    assert!(h.drew("kept"), "motor latches on itself");
    assert!(h.drew("times in the code"), "and how often it is named");
    assert!(!h.has("io more motor"), "all of them are shown");
    double_click(&mut h, "io card motor");
    assert!(!h.drew("times in the code"), "a second double-click closes them");
    h.press("io head inputs");
    h.wait(0.2);
    h.press("io more start");
    h.wait(0.2);
    assert!(
        h.drew("named 5 times in the code"),
        "the … opens them: start as a whole name"
    );
    assert!(
        h.has("io simulate start"),
        "an input's details offer to simulate it"
    );
    h.press("io simulate start");
    h.frame(vec![]);
    assert!(h.app.sim.as_ref().unwrap().sim_popup.is_some());
}

#[test]
fn the_code_glyph_finds_the_whole_name_and_steps_through_its_uses() {
    let mut h = sim_with(MIXER);
    h.press("io code start");
    h.wait(0.3);
    assert!(h.app.chrome.showing(Pane::Code), "the Code tab is showing");
    let f = h.app.chrome.find.clone().expect("the find box is open");
    assert_eq!((f.query.as_str(), f.word, f.current), ("start", true, 0));
    let src = h.app.sim.as_ref().unwrap().proj.build_src();
    assert_eq!(f.all(&src).len(), 5, "not start_button; the comment counts");
    for want in [1, 2, 3, 4, 0] {
        h.press("io code start");
        h.wait(0.3);
        assert_eq!(
            h.app.chrome.find.as_ref().unwrap().current,
            want,
            "the next use, round again"
        );
    }
    h.press("io code stop");
    h.wait(0.3);
    let f = h.app.chrome.find.clone().unwrap();
    assert_eq!(
        (f.query.as_str(), f.current),
        ("stop", 0),
        "another point starts over"
    );
}

#[test]
fn the_diagram_glyph_lights_a_points_blocks_with_nothing_running() {
    let mut h = sim_with(MIXER);
    assert_eq!(h.app.sim.as_ref().unwrap().session.scans, 0, "stopped");
    h.press("io diagram start");
    h.wait(0.6);
    let v = h.app.sim.as_ref().unwrap().viewer.as_ref().unwrap();
    let on = v.highlighted().expect("lit without a scan");
    let d = v.diagram();
    let reads = |name: &str| {
        d.nodes
            .iter()
            .filter(|n| matches!(&n.kind, helmsman_diagram::NodeKind::Read { path } if path == name))
            .map(|n| on.node(n.id))
            .collect::<Vec<_>>()
    };
    assert!(!reads("START").is_empty() && reads("START").iter().all(|&lit| lit));
    assert!(reads("START_BUTTON").iter().all(|&lit| !lit), "only that name");
    assert!(!v.frame_lit_pending(), "and framed once");
}

#[test]
fn tiles_are_narrower_keep_the_panel_and_open_their_details_under_the_grid() {
    let mut h = sim_with(MIXER);
    let right = h.rect("right");
    let wide = h.rect("io card stop");
    h.press("io grid");
    h.wait(0.3);
    assert!(h.app.chrome.io_grid);
    let tile = h.rect("io card stop");
    assert!(tile.width() < wide.width() * 0.6, "{tile:?} against {wide:?}");
    assert!(!h.drew("affects"), "a tile is the point alone");
    assert!(
        (h.rect("right").left() - right.left()).abs() <= 8.0,
        "the panel kept its width"
    );
    let at = h.rect("io card horn").center();
    h.click(at);
    h.click(at);
    h.wait(0.3);
    assert!(h.drew("driven by"), "the details of the tile, under the grid");
    assert!((h.rect("right").left() - right.left()).abs() <= 8.0);
    h.press("io list");
    h.wait(0.3);
    assert!(!h.app.chrome.io_grid);
    let back = Chrome::restored(&{
        let mut c = Chrome::default();
        c.io_grid = true;
        c.io_fold_out = true;
        c.saved()
    });
    assert!(back.io_grid && back.io_fold_out && !back.io_fold_in, "remembered");
    let odd = Chrome::restored("iofold=1 iogrid=yes iofold=111");
    assert!(!odd.io_grid && odd.io_fold_in && !odd.io_fold_out);
}

#[test]
fn a_heading_folds_its_section_away() {
    let mut h = sim_with(MIXER);
    assert!(h.has("io card start") && h.has("io card motor"));
    h.press("io head inputs");
    h.wait(0.2);
    assert!(!h.has("io card start"), "the inputs are folded");
    assert!(h.has("io card motor"), "the outputs are not");
    h.press("io head outputs");
    h.wait(0.2);
    assert!(!h.has("io card motor") && !h.has("io outputs"));
    h.press("io head inputs");
    h.wait(0.2);
    assert!(h.has("io card start"));
}

#[test]
fn forty_inputs_with_long_names_keep_the_panel_in_the_list_and_in_the_grid() {
    let mut vars = String::from("VAR_INPUT\n");
    let mut body = String::new();
    for k in 0..40 {
        writeln!(
            vars,
            "  proximity_sensor_{k}_at_the_far_end_of_conveyor_section : BOOL;"
        )
        .unwrap();
        writeln!(vars, "  setpoint_{k}_for_the_zone_temperature_controller : INT;").unwrap();
        writeln!(
            body,
            "out_{k} := proximity_sensor_{k}_at_the_far_end_of_conveyor_section AND setpoint_{k}_for_the_zone_temperature_controller > 2;"
        )
        .unwrap();
    }
    vars.push_str("END_VAR\nVAR_OUTPUT\n");
    for k in 0..40 {
        writeln!(vars, "  out_{k} : BOOL;").unwrap();
    }
    vars.push_str("END_VAR\n");
    let mut h = Harness::new(App::sim());
    h.app.chrome.dock(Pane::Io, true);
    h.wait(0.3);
    let right = h.rect("right");
    h.app
        .sim
        .as_mut()
        .unwrap()
        .load(&format!("PROGRAM wide\n{vars}{body}END_PROGRAM\n"), "test");
    h.wait(0.3);
    h.press("tab io");
    h.wait(0.3);
    let first = "io card proximity_sensor_0_at_the_far_end_of_conveyor_section";
    for step in ["list", "details", "grid", "grid details"] {
        match step {
            "details" | "grid details" => double_click(&mut h, first),
            "grid" => {
                double_click(&mut h, first);
                h.press("io grid");
            }
            _ => {}
        }
        h.wait(0.3);
        let now = h.rect("right");
        assert!(
            (right.left() - now.left()).abs() <= 8.0,
            "{step}: the panel grew: {right:?} -> {now:?}"
        );
        let card = h.rect(first);
        assert!(
            card.left() >= h.size.x - RIGHT_W - 8.0 && card.right() <= h.size.x,
            "{step}: the card {card:?} stays in the {RIGHT_W} pt panel"
        );
        assert!(h.small_text().is_empty(), "{step}: {:?}", h.small_text());
    }
}

#[test]
fn a_push_button_held_in_the_tab_stays_on_while_its_block_is_on_the_diagram() {
    let mut h = sim_with(
        "PROGRAM p\nVAR_INPUT jog_button : BOOL; END_VAR\nVAR_OUTPUT motor : BOOL; END_VAR\nmotor := jog_button;\nEND_PROGRAM\n",
    );
    let v = h.app.sim.as_ref().unwrap();
    assert!(
        v.on_map().iter().any(|n| n.eq_ignore_ascii_case("jog_button")),
        "its block is on the diagram too: {:?}",
        v.on_map()
    );
    // Of the two controls drawn under its name, the one on the card.
    let card = h.rect("io card jog_button");
    let control = h
        .app
        .chrome
        .hits
        .iter()
        .find(|(k, r)| k == "input jog_button" && card.contains_rect(*r))
        .map(|(_, r)| *r)
        .expect("the control on the card");
    assert!(
        control.height() < 90.0,
        "the button sits beside its glyph, a name that needs it on a second line: {control:?}"
    );
    let at = eframe::egui::pos2(control.right() - 30.0, control.center().y);
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Harness::button(at, true)]);
    for _ in 0..10 {
        h.frame(vec![]);
        assert_eq!(
            h.app.sim.as_ref().unwrap().session.value("jog_button"),
            Some(helmsman_iec::Value::Bool(true)),
            "held"
        );
    }
    h.frame(vec![Harness::button(at, false)]);
    h.wait(0.1);
    assert_eq!(
        h.app.sim.as_ref().unwrap().session.value("jog_button"),
        Some(helmsman_iec::Value::Bool(false)),
        "released"
    );
}

#[test]
fn a_right_click_on_a_card_chats_about_its_point_and_sends_nothing() {
    let mut h = sim_with(MIXER);
    for (card, name) in [("io card stop", "stop"), ("io card start_button", "start_button")] {
        h.wait(1.0);
        let r = h.rect(card);
        h.right_click(eframe::egui::pos2(r.center().x, r.bottom() - 8.0));
        h.wait(0.2);
        h.press("menu chat about");
        h.wait(0.3);
        let v = h.app.sim.as_ref().unwrap();
        assert!(
            v.instruct.chips.iter().any(|c| c.name.eq_ignore_ascii_case(name)),
            "a chip for {name}: {:?}",
            v.instruct.chips
        );
        assert!(!v.instruct.busy(), "nothing is sent");
        assert!(h.app.chrome.showing(Pane::Claude), "the Agent tab is showing");
        h.press("tab io");
        h.wait(0.3);
    }
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.chips.len(), 2);
}

const DRIVE: &str = "PROGRAM p\nVAR_INPUT enable : BOOL; jog_button : BOOL; END_VAR\nVAR_OUTPUT motor : BOOL; END_VAR\nmotor := enable OR jog_button;\nEND_PROGRAM\n";

/// Of the controls drawn under an input's name, the one on its card.
fn control(h: &Harness, name: &str) -> eframe::egui::Rect {
    let card = h.rect(&format!("io card {name}"));
    let key = format!("input {name}");
    h.app
        .chrome
        .hits
        .iter()
        .find(|(k, r)| *k == key && card.contains_rect(*r))
        .map(|(_, r)| *r)
        .expect("the control on the card")
}

fn value(h: &Harness, name: &str) -> Option<helmsman_iec::Value> {
    h.app.sim.as_ref().unwrap().session.value(name)
}

#[test]
fn an_input_set_while_stopped_is_the_start_state_and_a_quiet_line_says_nothing_reacts() {
    use crate::playground::RunState;
    use helmsman_iec::Value::Bool;
    let mut h = sim_with(DRIVE);
    assert_eq!(h.app.sim.as_ref().unwrap().session.state(), RunState::Stopped);
    let at = control(&h, "enable");
    h.click(eframe::egui::pos2(at.right() - 30.0, at.center().y));
    h.wait(0.2);
    assert_eq!(
        value(&h, "enable"),
        Some(Bool(true)),
        "the click sets it all the same"
    );
    assert_eq!(value(&h, "motor"), Some(Bool(false)), "and nothing reacts");
    let note = h.app.sim.as_ref().unwrap().notice.clone().expect("a quiet line");
    assert!(
        note.contains("is stopped, so nothing reacts yet") && note.contains("starts with enable at"),
        "{note}"
    );
    assert!(h.drew("nothing reacts yet"), "on the page");
    let line = h.rect("idle note");
    assert!(
        h.app.chrome.diagram.contains_rect(line),
        "low on the diagram, inside it: {line:?} in {:?}",
        h.app.chrome.diagram
    );
    // Run: the line goes, and the program starts from what was set.
    h.press("run");
    h.wait(0.3);
    assert_eq!(h.app.sim.as_ref().unwrap().session.state(), RunState::Running);
    assert_eq!(h.app.sim.as_ref().unwrap().notice, None, "gone once it runs");
    assert!(!h.has("idle note"));
    assert_eq!(value(&h, "motor"), Some(Bool(true)), "it started with enable on");
    // Running: a click is just a click.
    let at = control(&h, "enable");
    h.wait(1.0);
    h.click(eframe::egui::pos2(at.right() - 30.0, at.center().y));
    h.wait(0.2);
    assert_eq!(value(&h, "enable"), Some(Bool(false)));
    assert_eq!(h.app.sim.as_ref().unwrap().notice, None, "no line while it runs");
    // Paused: said as paused.
    h.app.sim.as_mut().unwrap().session.set_running(false);
    h.wait(1.0);
    assert_eq!(h.app.sim.as_ref().unwrap().session.state(), RunState::Paused);
    let at = control(&h, "enable");
    h.click(eframe::egui::pos2(at.right() - 30.0, at.center().y));
    h.wait(0.2);
    let note = h.app.sim.as_ref().unwrap().notice.clone().expect("a quiet line");
    assert!(
        note.contains("is paused") && note.contains("goes on with enable at"),
        "{note}"
    );
    assert_eq!(
        value(&h, "motor"),
        Some(Bool(false)),
        "held as the last scan left it"
    );
    // Stop keeps what the inputs were set to: the next run starts there.
    h.press("stop");
    h.wait(0.3);
    assert_eq!(h.app.sim.as_ref().unwrap().session.state(), RunState::Stopped);
    assert_eq!(value(&h, "enable"), Some(Bool(true)), "still set after Stop");
}

#[test]
fn a_push_button_pressed_while_stopped_says_it_is_on_only_while_held() {
    let mut h = sim_with(DRIVE);
    let at = control(&h, "jog_button");
    let at = eframe::egui::pos2(at.right() - 30.0, at.center().y);
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Harness::button(at, true)]);
    h.wait(0.2);
    let note = h.app.sim.as_ref().unwrap().notice.clone().expect("a quiet line");
    assert!(
        note.contains("nothing reacts to jog_button yet") && note.contains("only while it is held"),
        "{note}"
    );
    h.frame(vec![Harness::button(at, false)]);
    h.wait(0.2);
    assert_eq!(value(&h, "jog_button"), Some(helmsman_iec::Value::Bool(false)));
}

/// A card with names that are long but that a row has the room for.
const LONG: &str = "\
PROGRAM belt
VAR_INPUT feeder_belt_run_request : BOOL; END_VAR
VAR_OUTPUT feeder_belt_motor_contactor_closed : BOOL; END_VAR
feeder_belt_motor_contactor_closed := feeder_belt_run_request;
END_PROGRAM
";

#[test]
fn a_row_gives_its_long_names_the_room_they_need_inside_the_default_width() {
    let mut h = sim_with(LONG);
    let right = h.rect("right");
    // What was cut to an ellipsis before shows whole, wrapped on the card.
    for name in ["feeder_belt_run_request", "feeder_belt_motor_contactor_closed"] {
        assert!(h.drew(name), "{name}");
        assert!(!h.elided(name), "{name} is not cut");
        let card = h.rect(&format!("io card {name}"));
        assert!(
            card.left() >= h.size.x - RIGHT_W - 8.0 && card.right() <= h.size.x,
            "{name}: {card:?} stays in the {RIGHT_W} pt panel"
        );
    }
    assert!(
        !h.elided("driven by") && !h.elided("affects"),
        "the summary lines too"
    );
    assert!(
        (h.rect("right").left() - right.left()).abs() <= 8.0,
        "the panel kept its width"
    );
    // A tile has no room for it: there the name is still cut to the tile.
    h.press("io grid");
    h.wait(0.3);
    assert!(h.elided("feeder_belt_motor_contactor_closed"));
    let card = h.rect("io card feeder_belt_motor_contactor_closed");
    assert!(card.right() <= h.size.x, "{card:?}");
}

#[test]
fn a_tile_is_flat_the_same_fill_and_a_thin_outline_whether_open_or_not() {
    use crate::io_cards::look;
    use crate::theme::Theme;
    for t in [Theme::dark(), Theme::light()] {
        let (fill, rest) = look(true, false, &t);
        let (fill_on, open) = look(true, true, &t);
        assert_eq!(fill, fill_on, "no shaded fill on a selected tile");
        assert!((rest.width - 1.0).abs() < f32::EPSILON && (open.width - 1.0).abs() < f32::EPSILON);
        assert_ne!(
            rest.color, open.color,
            "the open tile is told by its outline colour"
        );
        // A row keeps its heavier outline for the output whose path is lit.
        let (_, lit) = look(false, true, &t);
        assert!(lit.width > 1.0);
    }
}

#[test]
fn a_tile_paints_nothing_that_moves_while_the_pointer_is_over_it() {
    let mut h = sim_with(MIXER);
    h.press("io grid");
    h.wait(0.3);
    let card = h.rect("io card stop");
    h.frame(vec![Event::PointerMoved(
        card.left_top() + eframe::egui::vec2(4.0, 4.0),
    )]);
    let still = h.rect("io card stop");
    h.wait(0.5);
    assert_eq!(
        h.rect("io card stop"),
        still,
        "hovering does not grow or move a tile"
    );
    assert_eq!(still, card, "and it is where it was");
}
