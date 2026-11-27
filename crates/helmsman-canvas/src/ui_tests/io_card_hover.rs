//! docs/UI.md "An I/O card at rest is its point" (Nick, 2026-11-27): icon,
//! name and value on one line; the code, diagram and Trend glyphs only
//! while the pointer is over the card or it is open, drawn over the right
//! end of the first line on the card's fill; the head's layout never
//! depends on hover; a click on a glyph goes to the glyph, not to the
//! control under it; and a simulated input's icon is lit when on and
//! dimmed when off, as an output's is.

use eframe::egui::{Color32, Event, Pos2, Rect, Shape, TextureId};

use super::io_cards::{hover_card, icon};
use super::io_one_line::{disc_under, point_glyph};
use super::{App, Harness};
use crate::editor::Pane;

/// Two switches, a push button and a number in; a BOOL on, a BOOL off
/// and a number out.
const PLANT: &str = "\
PROGRAM p
VAR_INPUT run : BOOL; enable : BOOL; jog_button : BOOL; level : INT; END_VAR
VAR_OUTPUT motor : BOOL; lamp : BOOL; speed : INT; END_VAR
motor := (run AND enable) OR jog_button;
lamp := TRUE;
speed := level + 7;
END_PROGRAM
";

const CARDS: [&str; 7] = ["run", "enable", "jog_button", "level", "motor", "lamp", "speed"];

const GLYPHS: [&str; 3] = ["code", "diagram", "trend"];

/// The simulation with [`PLANT`] scanned, the I/O tab in the right pane at
/// its default width, the pointer off the cards.
fn sim() -> Harness {
    let mut h = Harness::new(App::sim());
    h.size.y = 1500.0;
    h.app.chrome.dock(Pane::Io, true);
    h.app.sim.as_mut().unwrap().load(PLANT, "test");
    for _ in 0..3 {
        h.app.sim.as_mut().unwrap().session.step();
    }
    h.wait(0.3);
    h.press("tab io");
    away(&mut h);
    h
}

/// The pointer on the diagram, off every card, long enough for a hover to
/// ease out.
fn away(h: &mut Harness) {
    h.frame(vec![Event::PointerMoved(Pos2::new(700.0, 600.0))]);
    h.wait(0.5);
}

/// The glyph hooks of card `name` on screen: (glyph, rect).
fn glyphs(h: &Harness, name: &str) -> Vec<(&'static str, Rect)> {
    GLYPHS
        .iter()
        .filter_map(|g| h.app.chrome.rect(&format!("io {g} {name}")).map(|r| (*g, r)))
        .collect()
}

/// The library glyphs (meshes on a texture other than the font's) drawn
/// inside `area`.
fn meshes_in(h: &Harness, area: Rect) -> Vec<Rect> {
    fn walk(s: &Shape, area: Rect, out: &mut Vec<Rect>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            Shape::Mesh(m) if m.texture_id != TextureId::default() => {
                let mut r = Rect::NOTHING;
                for v in &m.vertices {
                    r.extend_with(v.pos);
                }
                if area.contains_rect(r) {
                    out.push(r);
                }
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

/// The strokes, paths, circles and meshes (anything but a filled rect and
/// a text) the last frame drew inside `area`: what a glyph is drawn with.
fn ink_in(h: &Harness, area: Rect) -> Vec<Rect> {
    fn walk(s: &Shape, area: Rect, out: &mut Vec<Rect>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            Shape::Rect(_) | Shape::Text(_) | Shape::Noop => {}
            s => {
                let r = s.visual_bounding_rect();
                if r.is_positive() && area.contains_rect(r) {
                    out.push(r);
                }
            }
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, area, &mut out);
    }
    out
}

/// What the last frame drew, in paint order: a text (its words and rect)
/// or a filled rect (its rect and fill).
#[derive(Clone, Debug)]
enum Mark {
    Text(String, Rect),
    Fill(Rect, Color32),
}

fn marks(h: &Harness) -> Vec<Mark> {
    fn walk(s: &Shape, out: &mut Vec<Mark>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            Shape::Text(t) if !t.galley.job.text.trim().is_empty() => out.push(Mark::Text(
                t.galley.job.text.clone(),
                t.galley.rect.translate(t.pos.to_vec2()),
            )),
            Shape::Rect(r) if r.fill.a() > 0 => out.push(Mark::Fill(r.rect, r.fill)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.shapes {
        walk(&c.shape, &mut out);
    }
    out
}

/// The texts drawn on card `name` (centre inside it): words and rect.
fn texts_on(h: &Harness, name: &str) -> Vec<(String, Rect)> {
    let card = h.rect(&format!("io card {name}"));
    marks(h)
        .into_iter()
        .filter_map(|m| match m {
            Mark::Text(t, r) if card.contains(r.center()) => Some((t, r)),
            _ => None,
        })
        .collect()
}

/// The fill of card `name`: the filled rect that is the card.
fn card_fill(h: &Harness, name: &str) -> Color32 {
    let card = h.rect(&format!("io card {name}"));
    marks(h)
        .into_iter()
        .find_map(|m| match m {
            Mark::Fill(r, c) if (r.min - card.min).length() < 1.0 && (r.max - card.max).length() < 1.0 => {
                Some(c)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{name}: no fill under the card {card:?}"))
}

/// A number input's box: the filled rect on its card, narrower than half
/// the card, that holds its value.
fn number_box(h: &Harness, name: &str) -> Option<Rect> {
    let card = h.rect(&format!("io card {name}"));
    let value = texts_on(h, name)
        .into_iter()
        .find(|(t, _)| t.parse::<f64>().is_ok())?;
    marks(h).into_iter().find_map(|m| match m {
        Mark::Fill(r, _)
            if card.contains_rect(r) && r.width() < card.width() / 2.0 && r.contains(value.1.center()) =>
        {
            Some(r)
        }
        _ => None,
    })
}

/// The controls on input card `name` that a user acts on: its icon (a
/// switch or a push button since 2026-11-27) and, for a number, its box.
fn controls(h: &Harness, name: &str) -> Vec<(&'static str, Rect)> {
    let mut out = vec![("icon", icon(h, name))];
    if name == "level" {
        out.push(("number box", number_box(h, name).expect("level's number box")));
    }
    out
}

/// At rest a card is its point: no glyph hook and no glyph drawn on it but
/// its icon. Under the pointer the three glyphs are hooked and drawn, on
/// the card's first line at its right end, over a backdrop of the card's
/// fill painted after the name. Gone again when the pointer leaves; shown
/// while the card is open, the pointer elsewhere.
#[test]
fn a_cards_glyphs_show_only_while_the_pointer_is_on_it_or_it_is_open() {
    let mut h = sim();
    let mut problems = Vec::new();
    for name in CARDS {
        away(&mut h);
        let card = h.rect(&format!("io card {name}"));
        let at_rest = glyphs(&h, name);
        if !at_rest.is_empty() {
            problems.push(format!("{name}: glyphs hooked at rest: {at_rest:?}"));
        }
        let icon_at = icon(&h, name);
        let others: Vec<Rect> = meshes_in(&h, card)
            .into_iter()
            .filter(|r| !r.intersects(icon_at))
            .collect();
        if !others.is_empty() {
            problems.push(format!(
                "{name}: glyphs drawn at rest beside the icon: {others:?}"
            ));
        }
        hover_card(&mut h, name);
        h.wait(0.3);
        let shown = glyphs(&h, name);
        if shown.len() != 3 {
            problems.push(format!("{name}: under the pointer, glyphs {shown:?}"));
            continue;
        }
        for (g, r) in &shown {
            if !card.contains_rect(*r) {
                problems.push(format!("{name}: the {g} glyph {r:?} is off its card {card:?}"));
            }
            if r.center().y < icon_at.top() || r.center().y > icon_at.bottom() {
                problems.push(format!(
                    "{name}: the {g} glyph {r:?} is not on the first line (icon {icon_at:?})"
                ));
            }
            if ink_in(&h, r.expand(1.0)).is_empty() {
                problems.push(format!("{name}: the {g} glyph is hooked at {r:?} but not drawn"));
            }
        }
        let union = shown.iter().fold(Rect::NOTHING, |u, (_, r)| u.union(*r));
        // The line's end: the card's, or a number input's box, which
        // stays clear of them.
        let end = number_box(&h, name)
            .filter(|_| name == "level")
            .map_or(card.right(), |b| b.left());
        if end - union.right() > 12.0 || union.right() > end {
            problems.push(format!(
                "{name}: the glyphs {union:?} are not at the right end of the line, {end:.1} on {card:?}"
            ));
        }
        // The backdrop: the card's fill over the line's end, painted after
        // the name, so what is under the glyphs does not show through.
        let fill = card_fill(&h, name);
        let all = marks(&h);
        let name_at = all
            .iter()
            .position(|m| matches!(m, Mark::Text(t, r) if t == name && card.contains(r.center())));
        let backdrop = all.iter().enumerate().any(|(i, m)| {
            matches!(m, Mark::Fill(r, c) if *c == fill && r.expand(0.5).contains_rect(union) && r.width() < card.width())
                && name_at.is_none_or(|n| i > n)
        });
        if !backdrop {
            problems.push(format!(
                "{name}: no backdrop of the card's fill {fill:?} over {union:?} after the name"
            ));
        }
        away(&mut h);
        let after = glyphs(&h, name);
        if !after.is_empty() {
            problems.push(format!(
                "{name}: glyphs still up once the pointer left: {after:?}"
            ));
        }
        // Nor drawn where they were: no ink there now (the name and the
        // value are text).
        for (g, r) in &shown {
            let left: Vec<Rect> = ink_in(&h, r.expand(1.0))
                .into_iter()
                .filter(|i| !i.intersects(icon_at))
                .collect();
            if !left.is_empty() {
                problems.push(format!(
                    "{name}: the pointer gone, ink still where its {g} glyph was: {left:?}"
                ));
            }
        }
    }
    // Open, the pointer elsewhere: the glyphs stay.
    for name in ["run", "motor"] {
        h.wait(1.0);
        let card = h.rect(&format!("io card {name}"));
        h.click(Pos2::new(card.center().x, card.bottom() - 8.0));
        h.wait(0.3);
        assert!(
            h.drew("times in the code"),
            "{name}: the click opened its details"
        );
        away(&mut h);
        let open = glyphs(&h, name);
        if open.len() != 3 {
            problems.push(format!("{name}: open, the pointer away, glyphs {open:?}"));
        }
        h.wait(1.0);
        h.click(Pos2::new(card.center().x, card.bottom() - 8.0));
        h.wait(0.3);
        away(&mut h);
        if !glyphs(&h, name).is_empty() {
            problems.push(format!("{name}: closed again, glyphs still up"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The glyphs over an input's line cover none of its controls (its icon, a
/// number's box), and a click on each glyph does what the glyph does and
/// leaves the input as it was.
#[test]
fn a_glyph_on_hover_covers_no_control_and_takes_its_own_click() {
    let mut h = sim();
    let mut problems = Vec::new();
    for name in ["run", "enable", "jog_button", "level"] {
        away(&mut h);
        hover_card(&mut h, name);
        h.wait(0.3);
        let shown = glyphs(&h, name);
        assert_eq!(shown.len(), 3, "{name}: the glyphs are up: {shown:?}");
        for (what, c) in controls(&h, name) {
            for (g, r) in &shown {
                if r.intersects(c) {
                    problems.push(format!("{name}: the {g} glyph {r:?} covers its {what} {c:?}"));
                }
            }
        }
        for g in GLYPHS {
            let before = h.app.sim.as_ref().unwrap().session.value(name);
            h.wait(1.0);
            super::io_cards::press_glyph(&mut h, g, name);
            h.wait(0.3);
            let after = h.app.sim.as_ref().unwrap().session.value(name);
            if after != before {
                problems.push(format!(
                    "{name}: a click on its {g} glyph changed it: {before:?} -> {after:?}"
                ));
            }
            // The Code glyph shows the Code tab on the left; the I/O tab
            // stays on the right.
            if g == "code" && !h.app.chrome.showing(Pane::Code) {
                problems.push(format!("{name}: its code glyph did not show the Code tab"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Where everything on card `name` is: the card, its icon, its controls'
/// hooks on the card, and each text's words and top-left corner.
#[derive(Debug, PartialEq)]
struct Layout {
    card: Rect,
    icon: Rect,
    hooks: Vec<(String, Rect)>,
    texts: Vec<(String, Pos2)>,
}

fn layout(h: &Harness, name: &str) -> Layout {
    let card = h.rect(&format!("io card {name}"));
    let hooks = h
        .app
        .chrome
        .hits
        .iter()
        .filter(|(k, r)| {
            (*k == format!("input {name}") || *k == format!("io row {name}")) && card.contains_rect(*r)
        })
        .cloned()
        .collect();
    Layout {
        card,
        icon: icon(h, name),
        hooks,
        texts: texts_on(h, name).into_iter().map(|(t, r)| (t, r.min)).collect(),
    }
}

/// Within half a point.
fn same(a: Rect, b: Rect) -> bool {
    (a.min - b.min).length() <= 0.5 && (a.max - b.max).length() <= 0.5
}

/// "The head's layout never depends on hover": the card, its icon, its
/// controls' hooks and every text drawn at rest stay where they were with
/// the pointer on the card, on its glyphs, and gone again. (A text the
/// glyphs' backdrop covers may be left undrawn; one drawn is in place.)
#[test]
fn nothing_on_a_card_moves_when_the_pointer_enters_or_leaves_it() {
    let mut h = sim();
    let mut problems = Vec::new();
    for name in CARDS {
        away(&mut h);
        let rest = layout(&h, name);
        let mut seen = Vec::new();
        hover_card(&mut h, name);
        h.wait(0.3);
        seen.push(("on the card", layout(&h, name)));
        let trend = h.rect(&format!("io trend {name}"));
        h.frame(vec![Event::PointerMoved(trend.center())]);
        h.wait(0.3);
        seen.push(("on its trend glyph", layout(&h, name)));
        away(&mut h);
        seen.push(("left again", layout(&h, name)));
        for (when, now) in seen {
            if !same(now.card, rest.card) || !same(now.icon, rest.icon) {
                problems.push(format!(
                    "{name} {when}: card {:?} icon {:?}, at rest {:?} {:?}",
                    now.card, now.icon, rest.card, rest.icon
                ));
            }
            for (k, r) in &rest.hooks {
                if !now.hooks.iter().any(|(k2, r2)| k2 == k && same(*r, *r2)) {
                    problems.push(format!("{name} {when}: {k:?} {r:?} moved: {:?}", now.hooks));
                }
            }
            for (t, at) in &rest.texts {
                let moved = now
                    .texts
                    .iter()
                    .filter(|(t2, _)| t2 == t)
                    .all(|(_, at2)| (*at2 - *at).length() > 0.5);
                let drawn = now.texts.iter().any(|(t2, _)| t2 == t);
                if drawn && moved {
                    problems.push(format!("{name} {when}: {t:?} at {at:?} moved: {:?}", now.texts));
                }
            }
            // The name is the card's point: drawn, under the pointer too.
            if !now.texts.iter().any(|(t, _)| t == name) {
                problems.push(format!("{name} {when}: its name is not drawn"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Unmultiplied, within 40 a channel of brand green.
fn green(c: Color32) -> bool {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    let w = crate::st_view::brand::GREEN;
    r.abs_diff(w.r()) < 40 && g.abs_diff(w.g()) < 40 && b.abs_diff(w.b()) < 40
}

/// The icon of input `name`: Ok(true) lit (full colour, green, on a disc),
/// Ok(false) dimmed (35% of full, no disc), or what it is instead.
fn input_lit(h: &Harness, name: &str) -> Result<bool, String> {
    let (r, c) = point_glyph(h, name);
    let disc = disc_under(h, r);
    match (c.a(), disc) {
        (255, true) if green(c) => Ok(true),
        (a, false) if (i32::from(a) - 89).abs() <= 3 => Ok(false),
        (a, disc) => Err(format!(
            "{name}: icon {:?} alpha {a}, disc {disc}",
            c.to_srgba_unmultiplied()
        )),
    }
}

fn value(h: &Harness, name: &str) -> Option<helmsman_iec::Value> {
    h.app.sim.as_ref().unwrap().session.value(name)
}

/// docs/UI.md: "A simulated input's icon is lit when on and dimmed when
/// off, as an output's is": a switch flipped on by its icon, off again,
/// and a push button held and let go, the pointer on the card throughout.
#[test]
fn a_simulated_inputs_icon_is_lit_on_a_disc_when_on_and_dimmed_when_off() {
    use helmsman_iec::Value::Bool;
    let mut h = sim();
    assert_eq!(value(&h, "enable"), Some(Bool(false)));
    assert_eq!(input_lit(&h, "enable"), Ok(false), "off at rest: dimmed");
    let at = icon(&h, "enable").center();
    h.click(at);
    h.wait(0.3);
    assert_eq!(value(&h, "enable"), Some(Bool(true)), "its icon flipped it");
    assert_eq!(input_lit(&h, "enable"), Ok(true), "on, the pointer on it: lit");
    away(&mut h);
    assert_eq!(input_lit(&h, "enable"), Ok(true), "on at rest: lit");
    h.wait(1.0);
    h.click(at);
    h.wait(0.3);
    assert_eq!(value(&h, "enable"), Some(Bool(false)));
    assert_eq!(input_lit(&h, "enable"), Ok(false), "off again: dimmed");
    // The push button: lit while held, dimmed once let go.
    away(&mut h);
    assert_eq!(input_lit(&h, "jog_button"), Ok(false), "a button at rest: dimmed");
    let at = icon(&h, "jog_button").center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Harness::button(at, true)]);
    h.wait(0.3);
    assert_eq!(value(&h, "jog_button"), Some(Bool(true)), "held");
    assert_eq!(input_lit(&h, "jog_button"), Ok(true), "held: lit");
    h.frame(vec![Harness::button(at, false)]);
    h.wait(0.3);
    assert_eq!(value(&h, "jog_button"), Some(Bool(false)), "let go");
    assert_eq!(input_lit(&h, "jog_button"), Ok(false), "let go: dimmed");
}
