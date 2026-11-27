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

/// One click on card `key` (Nick, 2026-11-26: one click opens or closes
/// its details; a double-click before), low on the card: under its
/// control and its glyphs.
fn click_card(h: &mut Harness, key: &str) {
    // A click just before would make this a double click.
    h.wait(1.0);
    let r = h.rect(key);
    let at = eframe::egui::pos2(r.center().x, (r.bottom() - 8.0).min(h.size.y - 4.0));
    h.click(at);
    h.wait(0.2);
}

/// Nick, 2026-11-27: a compact card is one line, the point alone; what
/// drives an output and what an input affects are in its details.
#[test]
fn a_card_is_its_point_alone_and_its_details_say_every_link() {
    let mut h = sim_with(MIXER);
    for word in [
        "affects",
        "driven by",
        "nothing sets it",
        "kept",
        "named",
        "times in the code",
    ] {
        assert!(!h.drew(word), "no {word:?} until a card is opened");
    }
    assert!(!h.has("io more start"), "no summary line, so no …");
    click_card(&mut h, "io card motor");
    assert!(h.drew("driven by"), "an output's details say what drives it");
    assert!(h.drew("kept"), "motor latches on itself");
    assert!(h.drew("times in the code"), "and how often it is named");
    click_card(&mut h, "io card motor");
    assert!(
        !h.drew("driven by") && !h.drew("times in the code"),
        "a second click closes them"
    );
    // spare is never assigned: said in its details.
    click_card(&mut h, "io card spare");
    assert!(h.drew("nothing sets it"), "spare is never assigned");
    click_card(&mut h, "io card spare");
    assert!(!h.drew("nothing sets it"));
    click_card(&mut h, "io card start");
    assert!(h.drew("affects"), "an input's details say what it affects");
    assert!(
        h.drew("named 5 times in the code"),
        "start as a whole name, not start_button"
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
    click_card(&mut h, "io card horn");
    h.wait(0.1);
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
            "details" | "grid details" => click_card(&mut h, first),
            "grid" => {
                click_card(&mut h, first);
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

/// The pieces of text the last frame drew inside `area` whose text
/// contains `needle`: (rows, cut short with an ellipsis).
fn text_in(h: &Harness, area: eframe::egui::Rect, needle: &str) -> Vec<(usize, bool)> {
    use eframe::egui::Shape;
    fn walk(s: &Shape, area: eframe::egui::Rect, needle: &str, out: &mut Vec<(usize, bool)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, needle, out)),
            Shape::Text(t) if area.contains(t.pos) && t.galley.job.text.contains(needle) => {
                out.push((t.galley.rows.len(), t.galley.elided));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, area, needle, &mut out);
    }
    out
}

/// How many pieces of text the last frame drew that are `name` exactly
/// and whole (not cut short).
fn whole(h: &Harness, name: &str) -> usize {
    use eframe::egui::Shape;
    fn walk(s: &Shape, name: &str) -> usize {
        match s {
            Shape::Vec(v) => v.iter().map(|s| walk(s, name)).sum(),
            Shape::Text(t) => usize::from(t.galley.job.text == name && !t.galley.elided),
            _ => 0,
        }
    }
    h.shapes.iter().map(|c| walk(&c.shape, name)).sum()
}

/// Nick, 2026-11-27: a long name is cut to the room on the card's one
/// line, and the whole of it is in its tooltip.
#[test]
fn a_long_name_is_cut_to_the_cards_one_line_and_shown_whole_on_hover() {
    let mut h = sim_with(LONG);
    let right = h.rect("right");
    for name in ["feeder_belt_motor_contactor_closed", "feeder_belt_run_request"] {
        let card = h.rect(&format!("io card {name}"));
        assert!(
            card.left() >= h.size.x - RIGHT_W - 8.0 && card.right() <= h.size.x,
            "{name}: {card:?} stays in the {RIGHT_W} pt panel"
        );
        let drawn = text_in(&h, card, name);
        assert!(!drawn.is_empty(), "{name} is on its card");
        assert!(
            drawn.iter().all(|&(rows, _)| rows == 1),
            "{name} is on one line, not wrapped: {drawn:?} in {card:?}"
        );
        assert!(
            drawn.iter().any(|&(_, cut)| cut),
            "{name} is cut to the room: {drawn:?}"
        );
        // The whole name on hover.
        let before = whole(&h, name);
        let at = eframe::egui::pos2(card.left() + 50.0, card.top() + 15.0);
        h.frame(vec![Event::PointerMoved(at)]);
        h.wait(1.5);
        assert!(
            whole(&h, name) > before,
            "{name}: hovering its card shows the whole name"
        );
        h.frame(vec![Event::PointerMoved(eframe::egui::pos2(700.0, 600.0))]);
        h.wait(0.5);
    }
    assert!(
        (h.rect("right").left() - right.left()).abs() <= 8.0,
        "the panel kept its width"
    );
    // A tile has no room for it either: cut to the tile.
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

/// An input read once and wired to three uses.
const FAN: &str = "\
PROGRAM fan
VAR_INPUT start : BOOL; other : BOOL; END_VAR
VAR_OUTPUT a : BOOL; b : BOOL; c : BOOL; END_VAR
a := start AND other;
b := start OR other;
c := NOT start;
END_PROGRAM
";

/// What is lit on the simulation's diagram for `name`: the reads of it,
/// every wire into or out of one, the block at each such wire's other
/// end, and (to check nothing else is) the reads of `other` with their
/// wires.
/// The reads of `start` lit; per wire of start, the wire and its far
/// block lit; the reads of `other` lit; other's own wires lit.
type FanLit = (Vec<bool>, Vec<(bool, bool)>, Vec<bool>, Vec<bool>);

fn fan_lit(h: &Harness) -> FanLit {
    use helmsman_diagram::NodeKind;
    let v = super::sim_view(h);
    let on = v.highlighted().expect("something is lit");
    let d = v.diagram();
    let reads_of = |name: &str| -> Vec<helmsman_diagram::NodeId> {
        d.nodes
            .iter()
            .filter(|n| matches!(&n.kind, NodeKind::Read { path } if path == name))
            .map(|n| n.id)
            .collect()
    };
    let start = reads_of("START");
    let other = reads_of("OTHER");
    assert!(!start.is_empty() && !other.is_empty(), "both inputs are drawn");
    let mut wires = Vec::new();
    let mut ends = Vec::new();
    for (k, w) in d.wires.iter().enumerate() {
        if start.contains(&w.from) || start.contains(&w.to) {
            let end = if start.contains(&w.from) { w.to } else { w.from };
            wires.push((on.wire(k), on.node(end)));
            ends.push(end);
        }
    }
    let other_wires: Vec<bool> = d
        .wires
        .iter()
        .enumerate()
        .filter(|(_, w)| other.contains(&w.from) && !ends.contains(&w.from))
        .map(|(k, _)| on.wire(k))
        .collect();
    (
        start.iter().map(|n| on.node(*n)).collect(),
        wires,
        other.iter().map(|n| on.node(*n)).collect(),
        other_wires,
    )
}

/// Nick, 2026-11-26: "an input lit one leg, not all its places". The I/O
/// card's diagram glyph lights every wire into or out of a block that
/// reads the point, and the block at each wire's other end: `start`, read
/// once and wired to three uses, lights all three wires and all three
/// blocks it feeds.
#[test]
fn the_diagram_glyph_lights_every_wire_of_a_point_and_every_use_it_feeds() {
    let mut h = sim_with(FAN);
    h.press("io diagram start");
    h.wait(0.6);
    let (reads, wires, others, other_wires) = fan_lit(&h);
    assert!(reads.iter().all(|&l| l), "the read of start is lit: {reads:?}");
    assert!(wires.len() >= 3, "start is wired to its three uses: {wires:?}");
    assert!(
        wires.iter().all(|&(wire, end)| wire && end),
        "every wire of start and the block at its other end are lit: {wires:?}"
    );
    assert!(others.iter().all(|&l| !l), "other is not start: {others:?}");
    assert!(
        other_wires.iter().all(|&l| !l),
        "nor are other's wires: {other_wires:?}"
    );
}

/// The same through Cross references (Alt Shift X) on the picked read.
#[test]
fn cross_references_light_every_wire_of_a_variable_and_every_use_it_feeds() {
    use eframe::egui::{Key, Modifiers};
    let mut h = sim_with(FAN);
    h.app.chrome.show(Pane::Code);
    h.wait(0.3);
    let read = |k: &helmsman_diagram::NodeKind| matches!(k, helmsman_diagram::NodeKind::Read { path } if path == "START");
    // A click on the read's header, its name: since the compact blocks of
    // 2026-11-27 a read's middle is its input switch's top edge, and a
    // click there flips the input instead of picking the block.
    let at = {
        let v = super::sim_view(&h);
        let n = v
            .diagram()
            .nodes
            .iter()
            .find(|n| read(&n.kind))
            .expect("the read");
        let r = v.block_on_screen(n.id).expect("drawn");
        let zoom = v.cams[v.scope].zoom;
        eframe::egui::pos2(
            r.center().x,
            r.top() + helmsman_diagram::layout::HEADER * zoom / 2.0,
        )
    };
    h.click(at);
    h.frame(vec![]);
    assert_eq!(
        super::sim_view(&h).picked_var().as_deref(),
        Some("START"),
        "the read is picked"
    );
    h.key(Modifiers::ALT.plus(Modifiers::SHIFT), Key::X);
    h.wait(0.3);
    let (reads, wires, others, other_wires) = fan_lit(&h);
    assert!(reads.iter().all(|&l| l), "{reads:?}");
    assert!(wires.len() >= 3, "{wires:?}");
    assert!(wires.iter().all(|&(wire, end)| wire && end), "{wires:?}");
    assert!(others.iter().all(|&l| !l), "{others:?}");
    assert!(other_wires.iter().all(|&l| !l), "{other_wires:?}");
}

// ---- Nick, 2026-11-26: the trend glyph, one click, inputs to the diagram,
// ---- an input's BOOL in green and red, the diagram glyph steps ----------

/// The simulation's camera on the diagram: where it looks and how close.
fn cam(h: &Harness) -> (eframe::egui::Vec2, f32) {
    let v = h.app.sim.as_ref().unwrap().viewer.as_ref().unwrap();
    let c = v.cams[v.scope];
    (c.offset, c.zoom)
}

#[test]
fn the_trend_glyph_pins_the_point_and_opens_the_trend_and_again_takes_it_off() {
    let mut h = sim_with(MIXER);
    h.app.chrome.trend = false;
    h.wait(0.2);
    assert!(h.has("io trend level"), "a graph glyph on the card");
    let src = h.app.sim.as_ref().unwrap().proj.build_src();
    let dirty = h.app.sim.as_ref().unwrap().proj.dirty();
    h.press("io trend level");
    h.wait(0.3);
    assert!(
        h.app.chrome.trend && !h.app.chrome.msgs,
        "the bottom panel opens on the Trend"
    );
    assert!(
        h.has("trend lane level"),
        "level is on the Trend: {:?}",
        h.app.chrome.hits
    );
    h.press("io trend level");
    h.wait(0.3);
    assert!(!h.has("trend lane level"), "again takes it off");
    // Nothing of it is an edit of the program.
    assert_eq!(h.app.sim.as_ref().unwrap().proj.build_src(), src);
    assert_eq!(h.app.sim.as_ref().unwrap().proj.dirty(), dirty);
}

#[test]
fn one_click_on_an_output_card_opens_its_details_and_another_closes_them() {
    let mut h = sim_with(MIXER);
    // The inputs folded: the outputs' cards are in view.
    h.press("io head inputs");
    h.wait(0.2);
    assert!(!h.drew("times in the code"), "closed to start");
    click_card(&mut h, "io card motor");
    assert!(h.drew("times in the code"), "one click opens the details");
    click_card(&mut h, "io card motor");
    assert!(!h.drew("times in the code"), "one more closes them");
}

#[test]
fn a_click_on_an_inputs_card_goes_to_it_on_the_diagram() {
    let mut h = sim_with(MIXER);
    assert!(
        h.app
            .sim
            .as_ref()
            .unwrap()
            .viewer
            .as_ref()
            .unwrap()
            .highlighted()
            .is_none(),
        "nothing lit to start"
    );
    click_card(&mut h, "io card stop");
    h.wait(0.6);
    let v = h.app.sim.as_ref().unwrap().viewer.as_ref().unwrap();
    let on = v.highlighted().expect("the input is lit on the diagram");
    let d = v.diagram();
    let reads = |name: &str| {
        d.nodes
            .iter()
            .filter(|n| matches!(&n.kind, helmsman_diagram::NodeKind::Read { path } if path == name))
            .map(|n| on.node(n.id))
            .collect::<Vec<_>>()
    };
    assert!(
        !reads("STOP").is_empty() && reads("STOP").iter().all(|&lit| lit),
        "stop's blocks"
    );
    assert!(reads("LEVEL").iter().all(|&lit| !lit), "not another input's");
    // And its blocks are in view.
    let screen = h.app.chrome.diagram;
    for n in d
        .nodes
        .iter()
        .filter(|n| matches!(&n.kind, helmsman_diagram::NodeKind::Read { path } if path == "STOP"))
    {
        let r = v.block_on_screen(n.id).expect("drawn");
        assert!(screen.intersects(r), "STOP's block {r:?} in the view {screen:?}");
    }
}

#[test]
fn the_diagram_glyph_pressed_again_frames_the_next_use_and_comes_round() {
    let mut h = sim_with(MIXER);
    // start is read by three statements: three uses, then the first again.
    let mut seen = Vec::new();
    for _ in 0..4 {
        h.wait(1.0);
        h.press("io diagram start");
        h.wait(1.0);
        seen.push(cam(&h));
    }
    let far = |a: (eframe::egui::Vec2, f32), b: (eframe::egui::Vec2, f32)| {
        (a.0 - b.0).length() > 4.0 || (a.1 - b.1).abs() > 0.01
    };
    assert!(
        far(seen[0], seen[1]) || far(seen[1], seen[2]),
        "a press frames another use: {seen:?}"
    );
    assert!(!far(seen[0], seen[3]), "round the end to the first: {seen:?}");
}

/// The colours the last frame drew inside `r`: text, fills and strokes.
fn colours_in(h: &Harness, r: eframe::egui::Rect) -> Vec<eframe::egui::Color32> {
    use eframe::egui::Shape;
    fn walk(s: &Shape, r: eframe::egui::Rect, out: &mut Vec<eframe::egui::Color32>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, r, out)),
            Shape::Text(t) if r.contains(t.pos) => {
                out.extend(t.override_text_color);
                out.extend(t.galley.job.sections.iter().map(|s| s.format.color));
            }
            Shape::Rect(x) if r.contains_rect(x.rect) => {
                out.push(x.fill);
                out.push(x.stroke.color);
            }
            Shape::Circle(c) if r.contains(c.center) => {
                out.push(c.fill);
                out.push(c.stroke.color);
            }
            Shape::LineSegment { points, stroke } if r.contains(points[0]) => out.push(stroke.color),
            Shape::Path(p) if p.points.first().is_some_and(|q| r.contains(*q)) => out.push(p.fill),
            Shape::Mesh(m) if m.vertices.first().is_some_and(|v| r.contains(v.pos)) => {
                out.extend(m.vertices.iter().map(|v| v.color));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, r, &mut out);
    }
    out.retain(|c| c.a() > 0);
    out
}

/// docs/UI.md, "TRUE glows, FALSE is flat, everywhere" (Nick, 2026-11-27):
/// FALSE is a flat muted grey on every surface, an I/O card's too; it was
/// red until then.
#[test]
fn an_inputs_bool_is_green_when_on_and_grey_when_off() {
    use crate::st_view::brand::{GREEN, OFF_RED, RED};
    use helmsman_iec::Value::Bool;
    let mut h = sim_with(DRIVE);
    let card = h.rect("io card enable");
    let near = |c: &eframe::egui::Color32, w: eframe::egui::Color32| {
        c.r().abs_diff(w.r()) < 40 && c.g().abs_diff(w.g()) < 40 && c.b().abs_diff(w.b()) < 40
    };
    let has = |h: &Harness, w| colours_in(h, card).iter().any(|c| near(c, w));
    assert_eq!(value(&h, "enable"), Some(Bool(false)));
    assert!(
        !has(&h, RED) && !has(&h, OFF_RED) && !has(&h, GREEN),
        "enable is off: flat grey, no red and no green {:?}",
        colours_in(&h, card)
    );
    let at = control(&h, "enable");
    h.click(eframe::egui::pos2(at.right() - 30.0, at.center().y));
    h.wait(0.3);
    assert_eq!(value(&h, "enable"), Some(Bool(true)), "the switch turned it on");
    assert!(
        has(&h, GREEN) && !has(&h, RED) && !has(&h, OFF_RED),
        "enable is on: green {:?}",
        colours_in(&h, card)
    );
}
