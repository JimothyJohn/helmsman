//! docs/UI.md "Lamps are green on, red off" (Nick, 2026-11-27): a lamp (an
//! output's card in the I/O tab, a lamp on the Monitor) is green when TRUE
//! and red when FALSE, whatever its name says (a lamp named `yellow` is
//! green when on); lit on a soft disc when on, dimmed when off.

use eframe::egui::{Color32, Pos2, Rect, Shape};

use super::io_one_line::{disc_under, point_glyph};
use super::{App, Harness};
use crate::editor::Pane;

/// Lamps named for another colour, all following one input.
const LAMPS: &str = "\
PROGRAM p
VAR_INPUT go : BOOL; END_VAR
VAR_OUTPUT yellow : BOOL; red : BOOL; amber_lamp : BOOL; fault_red : BOOL; END_VAR
yellow := go;
red := go;
amber_lamp := go;
fault_red := go;
END_PROGRAM
";

const NAMES: [&str; 4] = ["yellow", "red", "amber_lamp", "fault_red"];

/// Green: brand green's hue, unmultiplied (on the light page a saturated
/// green: the green channel well above the others).
fn is_green(c: Color32) -> bool {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    g > r.saturating_add(40) && g > b.saturating_add(40)
}

/// Red: the red channel well above the others, unmultiplied.
fn is_red(c: Color32) -> bool {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    r > g.saturating_add(60) && r > b.saturating_add(60)
}

/// Dimmed: the light it gives (its premultiplied channels) at most 60% of
/// full, whether by its alpha or by a darker shade.
fn is_dim(c: Color32) -> bool {
    c.r().max(c.g()).max(c.b()) <= 153
}

/// What lamp card `name` shows: Ok(()) when it is the look for `on`.
fn card_look(h: &Harness, name: &str, on: bool) -> Result<(), String> {
    let (r, c) = point_glyph(h, name);
    let disc = disc_under(h, r);
    let ok = if on {
        is_green(c) && c.a() == 255 && disc
    } else {
        is_red(c) && is_dim(c) && !disc
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{name} {}: {:?} alpha {} disc {disc}",
            if on { "TRUE" } else { "FALSE" },
            c.to_srgba_unmultiplied(),
            c.a()
        ))
    }
}

/// The I/O tab, dark and light: every lamp red and dimmed with `go` off,
/// green and lit on a disc with it on, whatever colour its name says.
#[test]
fn a_lamp_card_is_green_when_true_and_red_dimmed_when_false_whatever_its_name() {
    use helmsman_iec::Value::Bool;
    let mut problems = Vec::new();
    for light in [false, true] {
        let mut h = Harness::new(App::sim());
        h.size.y = 1500.0;
        h.app.chrome.light = light;
        h.app.chrome.dock(Pane::Io, true);
        h.app.sim.as_mut().unwrap().load(LAMPS, "test");
        for _ in 0..3 {
            h.app.sim.as_mut().unwrap().session.step();
        }
        h.wait(0.3);
        h.press("tab io");
        h.frame(vec![eframe::egui::Event::PointerMoved(Pos2::new(700.0, 600.0))]);
        h.wait(0.5);
        for name in NAMES {
            assert_eq!(h.app.sim.as_ref().unwrap().session.value(name), Some(Bool(false)));
            if let Err(e) = card_look(&h, name, false) {
                problems.push(format!("light {light}: {e}"));
            }
        }
        let at = super::io_cards::icon(&h, "go").center();
        h.click(at);
        h.wait(0.3);
        for _ in 0..3 {
            h.app.sim.as_mut().unwrap().session.step();
        }
        h.frame(vec![eframe::egui::Event::PointerMoved(Pos2::new(700.0, 600.0))]);
        h.wait(0.5);
        for name in NAMES {
            assert_eq!(
                h.app.sim.as_ref().unwrap().session.value(name),
                Some(Bool(true)),
                "{name} follows go"
            );
            if let Err(e) = card_look(&h, name, true) {
                problems.push(format!("light {light}: {e}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Lamps on the Monitor's outputs tile, two on and two off.
const BOARD: &str = "\
PROGRAM p
VAR_OUTPUT yellow : BOOL; red : BOOL; green : BOOL; amber_lamp : BOOL; END_VAR
yellow := TRUE;
red := FALSE;
green := FALSE;
amber_lamp := TRUE;
END_PROGRAM
";

/// The filled circles the last frame drew inside `area`: centre, radius,
/// fill.
fn circles(h: &Harness, area: Rect) -> Vec<(Pos2, f32, Color32)> {
    fn walk(s: &Shape, area: Rect, out: &mut Vec<(Pos2, f32, Color32)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, area, out)),
            Shape::Circle(c) if c.fill.a() > 0 && area.contains(c.center) => {
                out.push((c.center, c.radius, c.fill));
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

/// The rect of the text `name` drawn inside `area`.
fn text_at(h: &Harness, area: Rect, name: &str) -> Option<Rect> {
    fn walk(s: &Shape, area: Rect, name: &str) -> Option<Rect> {
        match s {
            Shape::Vec(v) => v.iter().find_map(|s| walk(s, area, name)),
            Shape::Text(t) if t.galley.job.text == name => {
                let r = t.galley.rect.translate(t.pos.to_vec2());
                area.contains(r.center()).then_some(r)
            }
            _ => None,
        }
    }
    h.shapes.iter().find_map(|c| walk(&c.shape, area, name))
}

/// The Monitor, the same rule: TRUE green and lit on a soft disc (a fainter,
/// wider circle under the lamp), FALSE red and dimmed with no disc,
/// whatever the name says.
#[test]
fn a_monitor_lamp_is_green_when_true_and_red_dimmed_when_false_whatever_its_name() {
    let mut h = Harness::new(App::sim());
    h.app.sim.as_mut().unwrap().load(BOARD, "test");
    h.app.sim.as_mut().unwrap().session.set_running(true);
    h.wait(0.5);
    h.press("monitor");
    h.wait(1.0);
    let tile = h.rect("outputs p/");
    let all = circles(&h, tile);
    let mut problems = Vec::new();
    for (name, on) in [
        ("yellow", true),
        ("red", false),
        ("green", false),
        ("amber_lamp", true),
    ] {
        let label = text_at(&h, tile, name).unwrap_or_else(|| panic!("{name} on the outputs tile"));
        // The lamp: the strongest circle left of the name, on its row.
        let row: Vec<_> = all
            .iter()
            .filter(|(c, _, _)| c.x < label.left() && (c.y - label.center().y).abs() < label.height() / 2.0)
            .collect();
        let Some(&&(at, radius, lamp)) = row.iter().max_by_key(|(_, _, f)| f.a()) else {
            problems.push(format!("{name}: no lamp beside it: {all:?}"));
            continue;
        };
        let disc = row
            .iter()
            .any(|(c, r, f)| (*c - at).length() < 1.0 && *r > radius && f.a() < lamp.a());
        let ok = if on {
            is_green(lamp) && lamp.a() == 255 && disc
        } else {
            is_red(lamp) && is_dim(lamp) && !disc
        };
        if !ok {
            problems.push(format!(
                "{name} {}: lamp {:?} alpha {} (light {}), soft disc {disc}",
                if on { "TRUE" } else { "FALSE" },
                lamp.to_srgba_unmultiplied(),
                lamp.a(),
                lamp.r().max(lamp.g()).max(lamp.b())
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
