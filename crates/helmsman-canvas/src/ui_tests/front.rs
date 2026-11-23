//! The front door (`crate::front`) on the screen: its three parts and
//! nothing of the editor, the faults thrown by a click, the way on to the
//! editor, and its layout at a laptop's, a tablet's and a phone's width.

use eframe::egui::Vec2;

use super::{App, Harness, Screen};
use crate::app::wants_front;
use crate::front::{ASKED, Fault, Front, TESTS};

fn front() -> Harness {
    let mut app = App::sim();
    app.front = Some(Front::new());
    Harness::new(app)
}

fn state(h: &Harness) -> &Front {
    h.app.front.as_ref().expect("the front door is up")
}

#[test]
fn a_first_visit_sees_the_request_the_machine_and_the_proofs_and_no_editor() {
    let mut h = front();
    assert!(h.drew("A control program that proves itself"));
    // The first run plays back: the request, the writing, the proofs.
    assert!(h.has("front asked 1") && !h.has("front asked 4"), "line by line");
    assert!(h.has("front machine"));
    assert!(h.drew("The proofs run once the program is written."));
    h.wait(2.5);
    assert!(h.drew("The Agent is writing the program"));
    h.wait(7.0);
    for k in 1..=ASKED.len() {
        assert!(h.has(&format!("front asked {k}")));
    }
    assert!(h.drew("Helmsman wrote the program from this on 20"));
    for k in 1..=TESTS.len() {
        assert!(h.has(&format!("front proof {k}")));
    }
    assert!(h.drew("All 5 proved"), "every proof green");
    // Nothing of the editor: no header switch, no tabs, no panes.
    let editor: Vec<&String> = h
        .app
        .chrome
        .hits
        .iter()
        .map(|(k, _)| k)
        .filter(|k| !k.starts_with("front "))
        .collect();
    assert_eq!(editor, Vec::<&String>::new());
    assert!(h.app.sim.is_none(), "the editor is not even built yet");
}

#[test]
fn a_click_fails_the_high_switch_and_the_page_says_what_the_program_did() {
    let mut h = front();
    h.click(h.rect("front fault HighStuck").center());
    assert_eq!(state(&h).fault, Fault::HighStuck);
    h.wait(25.0);
    assert!(state(&h).alarm() && !state(&h).filling());
    assert!(h.drew("the inlet closed below the brim and the alarm came on"));
    assert!(h.drew("Peak 1."), "below the 2 m brim");
    // Mended and reset: back to work.
    let reset = h.rect("front reset");
    h.click(h.rect("front fault None").center());
    assert_eq!(
        h.rect("front reset"),
        reset,
        "the buttons stay put when the sentence changes"
    );
    h.click(reset.center());
    h.wait(1.0);
    assert!(!state(&h).alarm());
}

#[test]
fn open_the_editor_lands_on_the_same_tank_with_its_tests_green() {
    let mut h = front();
    h.click(h.rect("front open editor").center());
    h.wait(0.3);
    assert!(h.app.front.is_none());
    assert_eq!(h.app.screen, Screen::Editor);
    let v = h.app.sim.as_mut().expect("the simulation");
    assert_eq!(v.title(), "tank");
    assert!(v.session.running, "running, as the front door left it");
    v.run_tests();
    assert_eq!(v.tests.rows.len(), TESTS.len());
    assert_eq!(v.tests.red(), Vec::<String>::new());
    h.header_is_up();
    assert!(!h.has("front headline"));
}

#[test]
fn the_button_at_the_foot_goes_on_too() {
    let mut h = front();
    h.click(h.rect("front change").center());
    h.wait(0.2);
    assert!(h.app.front.is_none());
}

/// Nothing runs off the side at a laptop's, a tablet's or a phone's
/// width, and no text is under the smallest size.
#[test]
fn it_fits_a_laptop_a_tablet_and_a_phone() {
    for width in [1400.0, 1024.0, 760.0, 390.0] {
        let mut h = front();
        h.size = Vec2::new(width, 860.0);
        h.wait(9.0);
        for (key, r) in &h.app.chrome.hits {
            assert!(
                r.left() >= 0.0 && r.right() <= width + 0.5,
                "{key} runs off a {width} pt window: {r:?}"
            );
        }
        assert_eq!(h.small_text(), Vec::new(), "at {width} pt");
        assert!(h.has("front machine"), "at {width} pt");
        if width >= 1024.0 {
            // On a laptop the machine and its buttons are above the fold.
            assert!(
                h.rect("front reset").bottom() <= h.size.y,
                "at {width} pt: {:?}",
                h.rect("front reset")
            );
        }
    }
}

#[test]
fn a_first_visit_opens_on_the_front_door_and_a_link_for_something_else_does_not() {
    assert!(wants_front("", false));
    assert!(!wants_front("", true), "once on to the editor, the editor");
    assert!(wants_front("?front", true), "the link always does");
    for q in [
        "?editor",
        "?tour",
        "?monitor",
        "?andon",
        "?box=http://10.0.0.5:8080",
    ] {
        assert!(!wants_front(q, false), "{q}");
    }
}
