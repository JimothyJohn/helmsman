//! The Describe tab's Send (Nick, 2026-11-26): while nothing can be sent
//! (the answer service out of reach, no relay, no program open, or any
//! other reason) Send is the arrow in red on gray and still takes a click,
//! which says why where refusals show, as its tooltip does, and sends
//! nothing. An empty field leaves Send as it is; sending it says "Say what
//! it should do first." and sends nothing. Driven by the pointer.

use eframe::egui::{Color32, Event, Rect, Shape};

use super::help_tab::with_program;
use super::proposal::open;
use super::{App, Harness};
use crate::st_view::brand::RED;

const EMPTY: &str = "Say what it should do first.";

/// The colours drawn inside `r`: text, fills, strokes and mesh vertices.
fn colours_in(h: &Harness, r: Rect) -> Vec<Color32> {
    fn walk(s: &Shape, r: Rect, out: &mut Vec<Color32>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, r, out)),
            Shape::Text(t) if r.contains(t.pos) => {
                out.extend(t.override_text_color);
                out.extend(t.galley.job.sections.iter().map(|s| s.format.color));
            }
            Shape::Rect(x) if r.expand(1.0).contains_rect(x.rect) => {
                out.push(x.fill);
                out.push(x.stroke.color);
            }
            Shape::Circle(c) if r.contains(c.center) => {
                out.push(c.fill);
                out.push(c.stroke.color);
            }
            Shape::LineSegment { points, stroke } if r.contains(points[0]) => out.push(stroke.color),
            Shape::Path(p) if p.points.first().is_some_and(|q| r.contains(*q)) => {
                out.push(p.fill);
            }
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

fn reddish(c: Color32) -> bool {
    c.r().abs_diff(RED.r()) < 40 && c.g().abs_diff(RED.g()) < 40 && c.b().abs_diff(RED.b()) < 40
}

/// A neutral gray of any lightness (the dark page's is dark).
fn grey(c: Color32) -> bool {
    let (r, g, b) = (i32::from(c.r()), i32::from(c.g()), i32::from(c.b()));
    (r - g).abs() < 12 && (g - b).abs() < 12 && r > 8 && r < 240
}

/// Every text the last frame drew.
fn texts(h: &Harness) -> Vec<String> {
    fn walk(s: &Shape, out: &mut Vec<String>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            Shape::Text(t) => out.push(t.galley.job.text.clone()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// What Send's tooltip says: the texts drawn once more while the pointer
/// rests on it (a reason the tab already shows is drawn twice then).
fn send_tip(h: &mut Harness) -> Vec<String> {
    let mut before = texts(h);
    h.frame(vec![Event::PointerMoved(h.rect("agent send").center())]);
    h.wait(1.5);
    let mut tip = Vec::new();
    for t in texts(h) {
        if let Some(i) = before.iter().position(|b| *b == t) {
            before.remove(i);
        } else {
            tip.push(t);
        }
    }
    h.frame(vec![Event::PointerMoved(eframe::egui::pos2(700.0, 600.0))]);
    h.wait(0.5);
    tip
}

/// Why a box page cannot send, set up as the box would report it.
#[derive(Clone, Copy, Debug)]
enum Cannot {
    /// No program file open in the Code tab.
    NoProgram,
    /// The box has no relay (`GET /api/chat/status`: unconfigured).
    NoRelay,
    /// The relay is configured but out of reach.
    Offline,
}

/// A box page signed in as an engineer, the Agent tab shown, `text` typed;
/// `cannot`: the reason nothing can be sent, `None` for a page that can.
fn boxed(text: &str, cannot: Option<Cannot>) -> Harness {
    use crate::box_agent::Status;
    let mut h = open("engineer");
    if !matches!(cannot, Some(Cannot::NoProgram)) {
        with_program(&mut h);
    }
    let status = match cannot {
        Some(Cannot::NoRelay) => Some(Status::Unconfigured(String::new())),
        Some(Cannot::Offline) => Some(Status::Offline("the relay did not answer".into())),
        _ => None,
    };
    if let Some(s) = status {
        h.app.boxv.as_mut().unwrap().agent.status = s;
    }
    h.press("tab claude");
    h.wait(0.3);
    h.app.boxv.as_mut().unwrap().instruct.draft = text.into();
    h.wait(0.2);
    h
}

fn nothing_sent_on_the_box(h: &Harness) {
    let v = h.app.boxv.as_ref().unwrap();
    assert!(
        v.chat_sent.is_empty(),
        "nothing went to the agent: {:?}",
        v.chat_sent.len()
    );
    assert!(v.help_sent.is_empty(), "nor to Help");
}

#[test]
fn on_a_box_that_cannot_send_send_is_red_on_gray_and_a_click_says_why_and_sends_nothing() {
    for why in [Cannot::NoProgram, Cannot::NoRelay, Cannot::Offline] {
        let mut h = boxed("make the lamp blink", Some(why));
        let send = h.rect("agent send");
        let c = colours_in(&h, send);
        assert!(c.iter().any(|&c| reddish(c)), "{why:?}: the arrow is red: {c:?}");
        assert!(c.iter().any(|&c| grey(c)), "{why:?}: on gray: {c:?}");
        let tip = send_tip(&mut h);
        assert!(!tip.is_empty(), "{why:?}: the tooltip says why");
        let said = tip.join(" ");
        assert!(
            !said.contains(EMPTY),
            "{why:?}: the reason is not the empty field's: {said}"
        );
        // Still clickable: a click says the same where refusals show.
        h.press("agent send");
        h.wait(0.3);
        assert!(
            tip.iter().any(|t| h.drew(t.trim())),
            "{why:?}: the click says why in the tab: {tip:?}"
        );
        nothing_sent_on_the_box(&h);
        assert_eq!(
            h.app.boxv.as_ref().unwrap().instruct.draft,
            "make the lamp blink",
            "{why:?}: what was typed stays"
        );
        if let Cannot::NoProgram = why {
            assert!(said.contains("Open a program file first"), "{said}");
        }
    }
}

#[test]
fn a_box_that_can_send_shows_send_as_it_is() {
    let h = boxed("make the lamp blink", None);
    let c = colours_in(&h, h.rect("agent send"));
    assert!(!c.iter().any(|&c| reddish(c)), "no red when it can send: {c:?}");
}

#[test]
fn enter_on_a_box_that_cannot_send_sends_nothing_either() {
    for why in [Cannot::NoRelay, Cannot::Offline] {
        let mut h = boxed("make the lamp blink", Some(why));
        h.click(h.rect("agent instruction").center());
        h.frame(vec![Event::Key {
            key: eframe::egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: eframe::egui::Modifiers::NONE,
        }]);
        h.wait(0.3);
        nothing_sent_on_the_box(&h);
    }
}

#[test]
fn an_empty_send_says_what_to_do_and_sends_nothing_in_the_simulation() {
    let mut h = Harness::new(App::sim());
    h.app.chrome.show(crate::editor::Pane::Claude);
    h.app
        .sim
        .as_mut()
        .unwrap()
        .chat
        .receive(crate::claude::Incoming::Proxy(true));
    h.wait(0.3);
    assert_eq!(h.app.sim.as_ref().unwrap().instruct.draft, "");
    // Send as it is: not the red of a Send that cannot reach anyone.
    let c = colours_in(&h, h.rect("agent send"));
    assert!(
        !c.iter().any(|&c| reddish(c)),
        "an empty field leaves Send as it is: {c:?}"
    );
    assert!(!h.drew(EMPTY), "nothing said before the click");
    h.press("agent send");
    h.wait(0.3);
    assert!(h.drew(EMPTY), "the click says what to do");
    let v = h.app.sim.as_ref().unwrap();
    assert!(v.instruct.run.is_none(), "no run started");
    assert_eq!(v.link.sent.len(), 0, "nothing sent");
    // Spaces and line breaks are empty too.
    h.app.sim.as_mut().unwrap().instruct.draft = "  \n\t ".into();
    h.wait(1.0);
    h.press("agent send");
    h.wait(0.3);
    let v = h.app.sim.as_ref().unwrap();
    assert!(
        v.instruct.run.is_none() && v.link.sent.is_empty(),
        "blank is empty"
    );
    assert!(h.drew(EMPTY));
}

#[test]
fn an_empty_send_on_a_box_says_what_to_do_and_sends_nothing() {
    let mut h = boxed("", None);
    let c = colours_in(&h, h.rect("agent send"));
    assert!(
        !c.iter().any(|&c| reddish(c)),
        "an empty field leaves Send as it is: {c:?}"
    );
    h.press("agent send");
    h.wait(0.3);
    nothing_sent_on_the_box(&h);
    assert!(h.drew(EMPTY), "the click says what to do");
    assert!(!h.has("agent status"), "no status dot row");
}
