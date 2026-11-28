//! The outputs as cards in the I/O tab (`crate::io_cards`): inside the
//! pane's default width however many there are and however long their
//! names, and a tap lights the output's path on the diagram.

use std::fmt::Write as _;

use super::{App, Harness};
use crate::editor::{Pane, RIGHT_W};

/// A program of `n` outputs with long names, each set from two inputs.
fn many(n: usize) -> String {
    let mut vars = String::from(
        "VAR_INPUT\n  a_rather_long_input_name_for_the_first_switch : BOOL;\n  second_input_that_is_also_long_to_wrap : BOOL;\nEND_VAR\nVAR_OUTPUT\n",
    );
    let mut body = String::new();
    for k in 0..n {
        writeln!(
            vars,
            "  conveyor_section_{k}_motor_contactor_with_a_long_name : BOOL;"
        )
        .unwrap();
        writeln!(
            body,
            "conveyor_section_{k}_motor_contactor_with_a_long_name := a_rather_long_input_name_for_the_first_switch AND second_input_that_is_also_long_to_wrap;"
        )
        .unwrap();
    }
    vars.push_str("END_VAR\n");
    format!("PROGRAM wide\n{vars}{body}END_PROGRAM\n")
}

fn sim_with(src: &str) -> Harness {
    let mut h = Harness::new(App::sim());
    h.app.chrome.dock(Pane::Io, true);
    h.app.sim.as_mut().unwrap().load(src, "test");
    h.app.sim.as_mut().unwrap().session.step();
    h.wait(0.3);
    h.press("tab io");
    h.wait(0.3);
    h
}

#[test]
fn fifty_outputs_with_long_names_keep_the_panel_at_its_default_width() {
    let mut h = Harness::new(App::sim());
    h.app.chrome.dock(Pane::Io, true);
    h.wait(0.3);
    let right = h.rect("right");
    h.app.sim.as_mut().unwrap().load(&many(50), "test");
    h.wait(0.3);
    h.press("tab io");
    h.wait(0.3);
    let after = h.rect("right");
    assert!(
        (right.left() - after.left()).abs() <= 8.0,
        "the panel grew: {right:?} -> {after:?}"
    );
    let outs = h.rect("io outputs");
    assert!(
        outs.left() >= h.size.x - RIGHT_W - 8.0 && outs.right() <= h.size.x,
        "the cards {outs:?} stay in the {RIGHT_W} pt panel"
    );
    h.press("io head inputs");
    h.wait(0.2);
    // One line each (Nick, 2026-11-27): what drives an output is in its
    // details, and they open inside the width too.
    assert!(!h.drew("driven by"), "no links on a closed card");
    let first = "io card conveyor_section_0_motor_contactor_with_a_long_name";
    h.wait(1.0);
    let r = h.rect(first);
    h.click(eframe::egui::pos2(r.center().x, r.bottom() - 6.0));
    h.wait(0.3);
    assert!(h.drew("driven by"), "its details say what drives it");
    let open = h.rect(first);
    assert!(
        open.left() >= h.size.x - RIGHT_W - 8.0 && open.right() <= h.size.x,
        "the open card {open:?} stays in the {RIGHT_W} pt panel"
    );
    assert!(
        (h.rect("right").left() - right.left()).abs() <= 8.0,
        "the panel grew with a card open"
    );
    // One card to a row, at the pane's width: from the ribbon that holds
    // the pane's hide button along its inner edge (2026-11-26) to the
    // pane's right margin.
    let ribbon = h.rect("right");
    assert!(
        (outs.left() - ribbon.right()).abs() <= 4.0 && h.size.x - outs.right() <= 30.0,
        "one card to a row, at the pane's width: {outs:?} beside the ribbon {ribbon:?}"
    );
}

#[test]
fn a_tap_on_a_card_lights_that_outputs_path() {
    let mut h = sim_with(&many(3));
    // Folded, the inputs leave the first output card in plain view.
    h.press("io head inputs");
    h.wait(0.3);
    let outs = h.rect("io outputs");
    h.click(outs.left_top() + eframe::egui::vec2(60.0, 14.0));
    h.wait(0.6);
    let v = h.app.sim.as_ref().unwrap().viewer.as_ref().unwrap();
    assert_eq!(
        v.because.as_deref(),
        Some("CONVEYOR_SECTION_0_MOTOR_CONTACTOR_WITH_A_LONG_NAME")
    );
    let on = v.highlighted().expect("the path is lit");
    let d = v.diagram();
    let lit_writes = d
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, helmsman_diagram::NodeKind::Write { .. }) && on.node(n.id))
        .count();
    assert_eq!(lit_writes, 1, "only that output's write");
}

#[test]
fn on_a_box_the_cards_come_from_its_module_and_a_tap_lights_from_its_scan() {
    let bench = crate::box_tests::Bench::new();
    let (state, _) = crate::box_tests::watch(&bench);
    let mut h = Harness::new(App::boxed());
    h.app.chrome.dock(Pane::Io, true);
    h.app.boxv.as_mut().unwrap().state = state;
    h.frame(vec![]);
    h.frame(vec![]);
    h.press("tab io");
    h.wait(0.3);
    assert!(
        h.has("io card conveyor") && h.has("io outputs"),
        "cards on a box's I/O tab"
    );
    assert!(
        !h.drew("driven by"),
        "one line each: the links are in the details"
    );
    let cards = h.app.boxv.as_ref().unwrap().view.as_ref().unwrap().output_cards();
    let names: Vec<&str> = cards.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"CONVEYOR"), "{names:?}");
    h.app
        .boxv
        .as_mut()
        .unwrap()
        .view
        .as_mut()
        .unwrap()
        .show_output("conveyor");
    h.wait(0.6);
    let v = h.app.boxv.as_ref().unwrap().view.as_ref().unwrap();
    assert!(v.highlighted().is_some(), "lit from the box's scan");
    assert!(!v.frame_lit_pending(), "and framed once");
}
