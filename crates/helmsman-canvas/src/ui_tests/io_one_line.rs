//! The I/O tab's cards on one line (Nick, 2026-11-27): a compact card is
//! the point's glyph and name, its code, diagram and trend glyphs over the
//! line's right end while the pointer is on it, in any pane; a BOOL says
//! its state by its colour and its glyph (lit on a soft disc when on,
//! dimmed when off), never by "= TRUE"; a number keeps its value, right
//! aligned at the card's end (finish 6), with "name = value" whole on
//! hover; the diagram glyph of a point the program never reads or writes
//! is off and says so; and ? opens Help with the caret in its question
//! field.

use eframe::egui::{Color32, Event, Key, Modifiers, Pos2, Rect, Shape, TextureId};

use super::{App, Harness};
use crate::editor::{Pane, RIGHT_W};

/// Two toggles, a push button, a number in; BOOLs on and off and a
/// number out; an input and an output the program never touches.
const PLANT: &str = "\
PROGRAM p
VAR_INPUT run : BOOL; enable : BOOL; jog_button : BOOL; level : INT; spare_in : BOOL; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; speed : INT; spare_out : BOOL; END_VAR
motor := (run AND enable) OR jog_button;
lamp := TRUE;
speed := level + 7;
END_PROGRAM
";

const CARDS: [&str; 9] = [
    "run",
    "enable",
    "jog_button",
    "level",
    "spare_in",
    "motor",
    "lamp",
    "speed",
    "spare_out",
];

/// The simulation with `src` loaded and scanned, the I/O tab shown in
/// the right pane (`right`) or the left one.
fn sim(src: &str, right: bool) -> Harness {
    let mut h = Harness::new(App::sim());
    h.size.y = 1500.0;
    if right {
        h.app.chrome.dock(Pane::Io, true);
    }
    h.app.sim.as_mut().unwrap().load(src, "test");
    for _ in 0..3 {
        h.app.sim.as_mut().unwrap().session.step();
    }
    h.wait(0.3);
    if right {
        h.press("tab io");
    } else {
        h.app.chrome.show(Pane::Io);
    }
    h.wait(0.3);
    h
}

/// A box page reading the bench's box (`ready` in and on; `conveyor`
/// and `green` out and on, `red` out and off), the I/O tab in the right
/// pane.
fn boxed() -> Harness {
    let bench = crate::box_tests::Bench::new();
    let (state, _) = crate::box_tests::watch(&bench);
    let mut h = Harness::new(App::boxed());
    h.app.chrome.dock(Pane::Io, true);
    h.app.boxv.as_mut().unwrap().state = state;
    h.frame(vec![]);
    h.frame(vec![]);
    h.press("tab io");
    h.wait(0.3);
    h
}

/// The texts the last frame drew whose position is inside `area`: the
/// text, its rows and whether it was cut short.
fn texts_in(h: &Harness, area: Rect) -> Vec<(String, usize, bool)> {
    fn walk(s: &Shape, area: Rect, out: &mut Vec<(String, usize, bool)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            Shape::Text(t) if area.contains(t.pos) && !t.galley.job.text.trim().is_empty() => {
                out.push((t.galley.job.text.clone(), t.galley.rows.len(), t.galley.elided));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, area, &mut out);
    }
    out
}

/// The tallest a card of one line is: a glyph's row and the card's
/// margins, with a few points to spare.
fn one_line() -> f32 {
    crate::glyphs::ROW + 2.0 * 8.0 + 6.0
}

/// Every card of `names` that is not one line, said as a line each.
fn not_one_line(h: &Harness, names: &[&str]) -> Vec<String> {
    let mut bad = Vec::new();
    for name in names {
        let card = h.rect(&format!("io card {name}"));
        let wrapped: Vec<_> = texts_in(h, card)
            .into_iter()
            .filter(|(_, rows, _)| *rows > 1)
            .collect();
        if card.height() > one_line() || !wrapped.is_empty() {
            bad.push(format!(
                "{name}: {:.0} pt tall (one line is at most {:.0}), wrapped {wrapped:?}",
                card.height(),
                one_line()
            ));
        }
    }
    bad
}

#[test]
fn every_card_is_one_line_at_the_right_panes_default_width() {
    let h = sim(PLANT, true);
    let edge = h.size.x - RIGHT_W - 8.0;
    for name in CARDS {
        let card = h.rect(&format!("io card {name}"));
        assert!(
            card.left() >= edge && card.right() <= h.size.x,
            "{name}: {card:?} inside the {RIGHT_W} pt pane"
        );
    }
    let bad = not_one_line(&h, &CARDS);
    assert!(bad.is_empty(), "cards that are not one line:\n{}", bad.join("\n"));
}

#[test]
fn every_card_is_one_line_in_the_left_pane_too() {
    let h = sim(PLANT, false);
    let bad = not_one_line(&h, &CARDS);
    assert!(bad.is_empty(), "cards that are not one line:\n{}", bad.join("\n"));
}

#[test]
fn on_a_box_every_card_is_one_line_at_the_right_panes_default_width() {
    let h = boxed();
    let names = ["ready", "conveyor", "green", "red"];
    let bad = not_one_line(&h, &names);
    assert!(bad.is_empty(), "cards that are not one line:\n{}", bad.join("\n"));
}

/// A BOOL card's texts that say its state in words.
fn state_words(h: &Harness, name: &str) -> Vec<String> {
    let card = h.rect(&format!("io card {name}"));
    texts_in(h, card)
        .into_iter()
        .map(|(t, _, _)| t)
        .filter(|t| {
            let u = t.to_ascii_uppercase();
            u.contains("TRUE")
                || u.contains("FALSE")
                || u.contains(": ON")
                || u.contains(": OFF")
                || t.contains('=')
        })
        .collect()
}

#[test]
fn a_bools_line_is_its_name_alone_and_a_number_keeps_its_value_in_the_simulation() {
    let mut h = sim(PLANT, true);
    let v = h.app.sim.as_ref().unwrap();
    assert_eq!(
        v.session.value("lamp"),
        Some(helmsman_iec::Value::Bool(true)),
        "lamp is on"
    );
    let mut said = Vec::new();
    for name in [
        "run",
        "enable",
        "jog_button",
        "spare_in",
        "motor",
        "lamp",
        "spare_out",
    ] {
        let words = state_words(&h, name);
        if !words.is_empty() {
            said.push(format!("{name}: {words:?}"));
        }
    }
    // An input flipped by its icon (its control since 2026-11-27) says it
    // no more than before, once the pointer has left the card (the hover
    // says "name = value": decided, docs/UI.md "An I/O card at rest").
    let before = h.app.sim.as_ref().unwrap().session.value("run");
    let at = super::io_cards::icon(&h, "run");
    h.click(at.center());
    h.wait(0.3);
    assert_ne!(
        h.app.sim.as_ref().unwrap().session.value("run"),
        before,
        "the click on run's icon flipped it"
    );
    h.frame(vec![Event::PointerMoved(Pos2::new(700.0, 600.0))]);
    h.wait(0.5);
    let words = state_words(&h, "run");
    if !words.is_empty() {
        said.push(format!("run, clicked: {words:?}"));
    }
    assert!(
        said.is_empty(),
        "BOOL lines that say their state in words:\n{}",
        said.join("\n")
    );
    let speed = texts_in(&h, h.rect("io card speed"));
    assert!(
        speed.iter().any(|(t, _, _)| t == "speed") && speed.iter().any(|(t, _, _)| t == "7"),
        "a number keeps its value beside its name: {speed:?}"
    );
}

#[test]
fn on_a_box_a_bools_line_is_its_name_alone() {
    let h = boxed();
    for name in ["ready", "conveyor", "green", "red"] {
        assert_eq!(state_words(&h, name), [] as [String; 0], "{name}");
        let card = h.rect(&format!("io card {name}"));
        assert!(
            texts_in(&h, card).iter().any(|(t, _, _)| t == name),
            "{name}: its name is its line"
        );
    }
}

/// A piece of text the last frame drew: what it says, where, whether it
/// was cut short, and whether it is in the code face.
fn drawn(h: &Harness) -> Vec<(String, Rect, bool, bool)> {
    fn walk(s: &Shape, out: &mut Vec<(String, Rect, bool, bool)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            Shape::Text(t) if !t.galley.job.text.trim().is_empty() => out.push((
                t.galley.job.text.clone(),
                t.galley.rect.translate(t.pos.to_vec2()),
                t.galley.elided,
                t.galley
                    .job
                    .sections
                    .first()
                    .is_some_and(|s| s.format.font_id.family == eframe::egui::FontFamily::Monospace),
            )),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// A number's value is what its card is for: at the pane's default width
/// a short name and its value are seen whole. Since 2026-11-27 they are
/// two texts: the name, and the value right-aligned in the code face at
/// the card's end with no " = " between (finish 5 and 6); the hover says
/// "speed = 7" whole.
#[test]
fn a_numbers_value_is_seen_at_the_right_panes_default_width() {
    let mut h = sim(PLANT, true);
    h.frame(vec![Event::PointerMoved(Pos2::new(700.0, 600.0))]);
    h.wait(0.5);
    let card = h.rect("io card speed");
    let on_card: Vec<_> = drawn(&h)
        .into_iter()
        .filter(|(_, r, _, _)| card.contains(r.center()))
        .collect();
    let name = on_card
        .iter()
        .find(|(t, ..)| t == "speed")
        .unwrap_or_else(|| panic!("speed's name on its card: {on_card:?}"));
    let value = on_card
        .iter()
        .find(|(t, ..)| t == "7")
        .unwrap_or_else(|| panic!("speed's value on its card: {on_card:?}"));
    assert!(!name.2 && !value.2, "neither is cut: {on_card:?}");
    assert!(value.3, "the value is a live number, in the code face: {value:?}");
    assert!(
        card.contains_rect(value.1) && card.right() - value.1.right() <= 12.0,
        "the value is whole and right-aligned at the card's end: {:?} in {card:?}",
        value.1
    );
    assert!(
        name.1.right() < value.1.left(),
        "the name ends before the value: {:?} {:?}",
        name.1,
        value.1
    );
    assert!(
        !on_card.iter().any(|(t, ..)| t.contains('=')),
        "no \" = \" on the line: {on_card:?}"
    );
    // The hover says it as one: name = value, whole.
    h.frame(vec![Event::PointerMoved(Pos2::new(
        name.1.center().x,
        name.1.center().y,
    ))]);
    h.wait(1.5);
    assert!(
        drawn(&h)
            .iter()
            .any(|(t, _, cut, _)| t.contains("speed = 7") && !cut),
        "the hover shows speed = 7 whole: {:?}",
        drawn(&h).iter().map(|d| &d.0).collect::<Vec<_>>()
    );
}

/// The point glyph of card `name`: its bounds and its colour (the
/// library glyphs are meshes on a texture other than the font's; the
/// point's is the one at the card's left edge,
/// [`super::io_cards::icon`]).
pub(super) fn point_glyph(h: &Harness, name: &str) -> (Rect, Color32) {
    fn walk(s: &Shape, area: Rect, out: &mut Vec<(Rect, Color32)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            Shape::Mesh(m) if m.texture_id != TextureId::default() => {
                let mut r = Rect::NOTHING;
                for v in &m.vertices {
                    r.extend_with(v.pos);
                }
                if let Some(v) = m.vertices.first()
                    && area.contains_rect(r)
                {
                    out.push((r, v.color));
                }
            }
            _ => {}
        }
    }
    let area = super::io_cards::icon(h, name).expand(0.5);
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, area, &mut out);
    }
    assert!(!out.is_empty(), "{name} has a glyph on its card");
    // A lit glyph may be painted more than once in its place.
    out.into_iter().max_by_key(|(_, c)| c.a()).unwrap()
}

/// Whether a filled disc is drawn under the glyph at `glyph`.
pub(super) fn disc_under(h: &Harness, glyph: Rect) -> bool {
    fn walk(s: &Shape, glyph: Rect) -> bool {
        match s {
            Shape::Vec(v) => v.iter().any(|s| walk(s, glyph)),
            Shape::Circle(c) => {
                c.fill.a() > 0 && glyph.contains(c.center) && c.radius * 2.0 >= glyph.width() * 0.8
            }
            _ => false,
        }
    }
    h.shapes.iter().any(|c| walk(&c.shape, glyph))
}

/// Full colour on a disc (on), or at 35% with no disc (off).
pub(super) fn lit(h: &Harness, name: &str) -> Result<bool, String> {
    let (r, c) = point_glyph(h, name);
    let disc = disc_under(h, r);
    // 35% of full: 89 of 255, give or take rounding.
    match (c.a(), disc) {
        (255, true) => Ok(true),
        (a, false) if (i32::from(a) - 89).abs() <= 3 => Ok(false),
        (a, disc) => Err(format!("{name}: glyph alpha {a}, disc {disc}")),
    }
}

#[test]
fn on_a_box_a_bools_glyph_is_lit_on_a_disc_when_on_and_dimmed_when_off() {
    let h = boxed();
    let v = h.app.boxv.as_ref().unwrap();
    let on = |n: &str| {
        v.state
            .signals
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(n))
            .and_then(|s| s.value)
    };
    assert_eq!(
        on("ready"),
        Some(helmsman_iec::Value::Bool(true)),
        "the bench as this test reads it"
    );
    assert_eq!(lit(&h, "ready"), Ok(true), "an input that is on");
    assert_eq!(lit(&h, "conveyor"), Ok(true), "an output that is on");
    assert_eq!(lit(&h, "red"), Ok(false), "an output that is off");
}

#[test]
fn in_the_simulation_an_outputs_glyph_is_lit_on_a_disc_when_on_and_dimmed_when_off() {
    let mut h = sim(PLANT, true);
    // The outputs in plain view under the folded inputs.
    h.press("io head inputs");
    h.wait(0.3);
    assert_eq!(lit(&h, "lamp"), Ok(true), "lamp := TRUE");
    assert_eq!(lit(&h, "motor"), Ok(false), "motor is off");
    assert_eq!(lit(&h, "spare_out"), Ok(false), "never set: off");
}

/// The texts drawn while the pointer rests on `key` that were not drawn
/// before: its tooltip.
fn tooltip(h: &mut Harness, key: &str) -> Vec<String> {
    fn all(h: &Harness) -> Vec<String> {
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
    h.frame(vec![Event::PointerMoved(Pos2::new(700.0, 600.0))]);
    h.wait(0.5);
    // A card's glyphs are up only while the pointer is on its card
    // (2026-11-27): onto the card first, then onto the glyph.
    if let Some(name) = ["io code ", "io diagram ", "io trend "]
        .iter()
        .find_map(|g| key.strip_prefix(g))
    {
        super::io_cards::hover_card(h, name);
    }
    let mut before = all(h);
    h.frame(vec![Event::PointerMoved(h.rect(key).center())]);
    h.wait(1.5);
    let mut tip = Vec::new();
    for t in all(h) {
        if let Some(i) = before.iter().position(|b| *b == t) {
            before.remove(i);
        } else {
            tip.push(t);
        }
    }
    h.frame(vec![Event::PointerMoved(Pos2::new(700.0, 600.0))]);
    h.wait(0.5);
    tip
}

fn lit_now(h: &Harness) -> bool {
    h.app
        .sim
        .as_ref()
        .unwrap()
        .viewer
        .as_ref()
        .unwrap()
        .highlighted()
        .is_some()
}

#[test]
fn the_diagram_glyph_of_a_point_the_program_never_reads_or_writes_is_off_and_says_why() {
    let mut h = sim(PLANT, true);
    {
        let v = h.app.sim.as_ref().unwrap().viewer.as_ref().unwrap();
        for (name, on) in [
            ("run", true),
            ("RUN", true),
            ("jog_button", true),
            ("motor", true),
            ("speed", true),
            ("spare_in", false),
            ("spare_out", false),
            ("SPARE_IN", false),
            ("", false),
            ("no_such_point", false),
        ] {
            assert_eq!(v.has_signal(name), on, "has_signal({name:?})");
        }
    }
    for name in ["spare_in", "spare_out"] {
        if name == "spare_out" {
            // The outputs in plain view under the folded inputs.
            h.press("io head inputs");
            h.wait(0.3);
        }
        let tip = tooltip(&mut h, &format!("io diagram {name}"));
        assert!(
            tip.iter().any(|t| t.contains("never reads or writes")),
            "{name}: the glyph says why it is off: {tip:?}"
        );
        h.wait(1.0);
        super::io_cards::press_glyph(&mut h, "diagram", name);
        h.wait(0.6);
        assert!(!lit_now(&h), "{name}: pressing an off glyph lights nothing");
    }
    h.press("io head inputs");
    h.wait(0.3);
    let tip = tooltip(&mut h, "io diagram run");
    assert!(
        !tip.is_empty() && !tip.iter().any(|t| t.contains("never")),
        "run is read: its glyph's tooltip says what it does: {tip:?}"
    );
    h.wait(1.0);
    super::io_cards::press_glyph(&mut h, "diagram", "run");
    h.wait(0.6);
    assert!(lit_now(&h), "a read input's glyph lights its blocks");
}

fn shift(key: Key, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::SHIFT,
    }
}

#[test]
fn the_question_mark_opens_help_with_the_caret_in_its_question_field() {
    let mut h = super::proposal::open("engineer");
    h.wait(0.2);
    assert!(!h.app.chrome.showing(Pane::Help));
    // A browser sends the key and the character it types in one frame.
    h.frame(vec![shift(Key::Questionmark, true), Event::Text("?".into())]);
    h.frame(vec![shift(Key::Questionmark, false)]);
    h.wait(0.2);
    assert!(h.app.chrome.showing(Pane::Help), "? shows Help");
    // The caret is in the question: what is typed next goes there, and
    // the ? that opened it is not in it.
    h.frame(vec![Event::Text("Why is the lamp off".into())]);
    // A question ends in ?: typed in the field, it is a character.
    h.frame(vec![shift(Key::Questionmark, true), Event::Text("?".into())]);
    h.frame(vec![shift(Key::Questionmark, false)]);
    h.wait(0.2);
    assert_eq!(h.app.boxv.as_ref().unwrap().help.draft, "Why is the lamp off?");
    assert!(h.app.chrome.showing(Pane::Help));
}

#[test]
fn a_question_mark_typed_in_the_describe_box_stays_there() {
    let mut h = super::proposal::open("engineer");
    h.press("tab claude");
    h.wait(0.2);
    h.click(h.rect("agent instruction").center());
    h.frame(vec![Event::Text("why is it off".into())]);
    h.frame(vec![shift(Key::Questionmark, true), Event::Text("?".into())]);
    h.frame(vec![shift(Key::Questionmark, false)]);
    h.wait(0.2);
    assert_eq!(h.app.boxv.as_ref().unwrap().instruct.draft, "why is it off?");
    assert!(!h.app.chrome.showing(Pane::Help), "typing ? is not the key");
    assert_eq!(h.app.boxv.as_ref().unwrap().help.draft, "");
}
