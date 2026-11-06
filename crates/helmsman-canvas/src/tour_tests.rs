//! The guided tour's steps as a state machine (`crate::tour`), with no
//! UI: what the app shows goes in, the step comes out.

use eframe::egui::{Pos2, Rect, Vec2};

use crate::tour::{Button, Effect, Seen, Step, Tour, place, words};

/// The playground as it opens: Blink, an example, running.
fn opened() -> Seen {
    Seen {
        editor: true,
        open: "blink".into(),
        example: true,
        source: 1,
        compiles: true,
        live: true,
        trend: 10,
        trend_open: true,
        ..Seen::default()
    }
}

/// A tour started and on the first action, with the app as `s`.
fn started(s: &Seen) -> Tour {
    let mut t = Tour::new();
    t.observe(s);
    assert_eq!(t.press(Button::Start, s), None);
    assert_eq!(t.step, Step::OpenBlink);
    t
}

#[test]
fn the_welcome_waits_for_start_and_nothing_else_moves_it() {
    let mut t = Tour::new();
    let mut s = opened();
    t.observe(&s);
    s.naming_blink = true;
    s.asks = 3;
    t.observe(&s);
    assert_eq!(t.step, Step::Welcome);
    t.press(Button::Start, &s);
    assert_eq!(t.step, Step::OpenBlink);
}

#[test]
fn the_whole_loop_advances_on_what_the_visitor_does() {
    let mut s = opened();
    let mut t = started(&s);
    // Nothing done: nothing moves, however many frames.
    for _ in 0..5 {
        t.observe(&s);
    }
    assert_eq!(t.step, Step::OpenBlink);
    // Click Blink: the name popup.
    s.naming_blink = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Name);
    // Name it: the copy opens, compiled and stopped.
    s.naming_blink = false;
    s.open = "my-blink".into();
    s.example = false;
    s.live = false;
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
    // A keystroke that does not compile yet is not the edit.
    s.source = 2;
    s.compiles = false;
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
    s.compiles = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Deploy);
    // Deploy: the edit runs.
    s.live = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Watch);
    // Pick the lamp.
    s.picked = Some("Node(3)".into());
    t.observe(&s);
    assert_eq!(t.step, Step::Trend);
    // Drag the window.
    s.trend = 11;
    t.observe(&s);
    assert_eq!(t.step, Step::Agent);
    // Ask.
    s.asks = 1;
    t.observe(&s);
    assert_eq!(t.step, Step::Done);
    assert!(!t.ended);
    assert_eq!(t.press(Button::Finish, &s), None);
    assert!(t.ended);
}

#[test]
fn one_frame_moves_one_step_at_most() {
    let mut s = opened();
    let mut t = started(&s);
    s.naming_blink = true;
    t.observe(&s);
    // Everything after it already true in the same frame.
    s.naming_blink = false;
    s.open = "mine".into();
    s.example = false;
    s.source = 9;
    s.live = false;
    t.observe(&s);
    assert_eq!(
        t.step,
        Step::Edit,
        "the edit is measured from the copy as it opened"
    );
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
}

#[test]
fn cancelling_the_name_goes_back_to_the_example() {
    let mut s = opened();
    let mut t = started(&s);
    s.naming_blink = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Name);
    s.naming_blink = false;
    t.observe(&s);
    assert_eq!(t.step, Step::OpenBlink);
}

#[test]
fn an_edit_already_deployed_skips_the_deploy_step() {
    let mut s = opened();
    s.open = "mine".into();
    s.example = false;
    let mut t = started(&s);
    s.naming_blink = true;
    t.observe(&s);
    s.naming_blink = false;
    s.open = "mine-2".into();
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
    s.source = 7;
    s.live = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Watch);
}

#[test]
fn an_edit_to_another_project_or_an_example_is_not_this_step() {
    let mut s = opened();
    let mut t = started(&s);
    s.naming_blink = true;
    t.observe(&s);
    s.naming_blink = false;
    s.open = "mine".into();
    s.example = false;
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
    // Opened another project: its files differ, but nothing was edited.
    s.open = "other".into();
    s.source = 99;
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
    s.open = "mine".into();
    s.example = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
}

#[test]
fn back_never_lands_on_a_step_that_is_already_done() {
    let mut s = opened();
    let mut t = started(&s);
    s.naming_blink = true;
    t.observe(&s);
    s.naming_blink = false;
    s.open = "mine".into();
    s.example = false;
    s.live = false;
    t.observe(&s);
    s.source = 2;
    t.observe(&s);
    s.live = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Watch);
    // Back to Deploy: it runs already, and the step waits for a new deploy.
    t.press(Button::Back, &s);
    assert_eq!(t.step, Step::Deploy);
    for _ in 0..3 {
        t.observe(&s);
    }
    assert_eq!(t.step, Step::Deploy);
    // Back from Deploy to Edit, from Edit to the example (naming is a popup).
    t.press(Button::Back, &s);
    assert_eq!(t.step, Step::Edit);
    t.observe(&s);
    assert_eq!(t.step, Step::Edit);
    t.press(Button::Back, &s);
    assert_eq!(t.step, Step::OpenBlink);
    t.press(Button::Back, &s);
    assert_eq!(t.step, Step::Welcome);
    t.press(Button::Back, &s);
    assert_eq!(t.step, Step::Welcome);
}

#[test]
fn back_from_naming_shuts_the_popup() {
    let mut s = opened();
    let mut t = started(&s);
    s.naming_blink = true;
    t.observe(&s);
    assert_eq!(t.press(Button::Back, &s), Some(Effect::CloseNaming));
    assert_eq!(t.step, Step::OpenBlink);
}

#[test]
fn the_agent_step_can_be_skipped_and_only_it() {
    let mut s = opened();
    let mut t = started(&s);
    t.press(Button::SkipStep, &s);
    assert_eq!(t.step, Step::OpenBlink, "Skip this step is the Agent's only");
    // Walk to the Agent step.
    s.naming_blink = true;
    t.observe(&s);
    s.naming_blink = false;
    s.open = "m".into();
    s.example = false;
    s.live = false;
    t.observe(&s);
    s.source = 3;
    t.observe(&s);
    s.live = true;
    t.observe(&s);
    s.picked = Some("x".into());
    t.observe(&s);
    s.trend = 1;
    t.observe(&s);
    assert_eq!(t.step, Step::Agent);
    t.press(Button::SkipStep, &s);
    assert_eq!(t.step, Step::Done);
    assert_eq!(t.press(Button::Requirements, &s), Some(Effect::Requirements));
    assert_eq!(t.press(Button::Help, &s), Some(Effect::Help));
    assert_eq!(t.step, Step::Done);
}

#[test]
fn skip_ends_it_from_any_step() {
    for k in 0..Step::ALL.len() {
        let mut t = Tour::new();
        let s = opened();
        t.observe(&s);
        t.step = Step::ALL[k];
        assert_eq!(t.press(Button::Skip, &s), None);
        assert!(t.ended, "{:?}", Step::ALL[k]);
    }
}

#[test]
fn off_the_editor_nothing_moves_and_the_card_points_back_to_it() {
    let mut s = opened();
    let mut t = started(&s);
    let away = Seen::default();
    t.observe(&away);
    assert_eq!(t.step, Step::OpenBlink);
    assert_eq!(t.anchors(&away), ["editor"]);
    assert_eq!(words(t.step, &away, "Ctrl+D").0, "Back to the Editor");
    // Back on it with the popup open: on it goes.
    s.naming_blink = true;
    t.observe(&s);
    assert_eq!(t.step, Step::Name);
}

#[test]
fn every_step_has_words_and_the_actions_are_numbered() {
    let s = opened();
    let mut numbers = Vec::new();
    for step in Step::ALL {
        let (title, body) = words(step, &s, "Ctrl+D");
        assert!(!title.is_empty() && body.len() > 40, "{step:?}");
        numbers.extend(step.number());
    }
    assert_eq!(numbers, (1..=Step::ACTIONS).collect::<Vec<_>>());
    assert!(words(Step::Deploy, &s, "⌘D").1.contains("⌘D"));
}

#[test]
fn anchors_fall_back_from_the_control_to_its_tab_to_its_pane() {
    let s = opened();
    let mut t = Tour::new();
    t.step = Step::OpenBlink;
    assert_eq!(t.anchors(&s), ["project blink", "pane files", "left"]);
    t.step = Step::Agent;
    assert_eq!(t.anchors(&s)[0], "agent key");
    let keyed = Seen {
        keyed: true,
        ..s.clone()
    };
    assert_eq!(t.anchors(&keyed)[0], "agent instruction");
    t.step = Step::Welcome;
    assert_eq!(t.anchors(&s), [] as [String; 0]);
}

#[test]
fn the_card_sits_beside_its_control_on_screen_and_off_it() {
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 860.0));
    let size = Vec2::new(380.0, 260.0);
    // A control on the left: the card to its right.
    let a = Rect::from_min_size(Pos2::new(20.0, 100.0), Vec2::new(200.0, 30.0));
    let c = place(Some(a), size, screen);
    assert!(c.left() > a.right() && !c.intersects(a));
    // On the right edge: to its left.
    let a = Rect::from_min_size(Pos2::new(1200.0, 100.0), Vec2::new(190.0, 30.0));
    let c = place(Some(a), size, screen);
    assert!(c.right() < a.left() && screen.contains_rect(c));
    // A full-width strip at the bottom: above it.
    let a = Rect::from_min_size(Pos2::new(0.0, 700.0), Vec2::new(1400.0, 150.0));
    let c = place(Some(a), size, screen);
    assert!(c.bottom() < a.top() && screen.contains_rect(c));
    // No control: the middle.
    let c = place(None, size, screen);
    assert!((c.center() - screen.center()).length() < 1.0);
    // Covering the whole screen: still on it.
    let c = place(Some(screen), size, screen);
    assert!(screen.contains_rect(c));
}
