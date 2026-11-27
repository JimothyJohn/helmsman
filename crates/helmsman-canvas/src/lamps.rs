//! Outputs as lamps (Nick, 2026-11-12: the program should show itself
//! without words). A BOOL output named after a colour (`red`, `lamp_green`,
//! `AMBER`) is a lamp of that colour wherever it is drawn: its value's text
//! in the I/O tab and its line on the trend. (The fixture in the diagram's
//! corner was removed, Nick, 2026-11-26.)

use eframe::egui::Color32;
#[cfg(test)]
use eframe::egui::{Pos2, Rect, Vec2};
#[cfg(test)]
use helmsman_iec::Value;

use crate::st_view::brand::{GREEN, RED, WHITE, YELLOW};

/// Pilot-lamp blue and white (the brand has neither).
pub const BLUE: Color32 = Color32::from_rgb(0x5A, 0xA9, 0xFF);

/// The colour a name asks for: one of its words (split at anything that is
/// not a letter) is a colour's name. `None`: not a colour.
#[must_use]
pub fn colour(name: &str) -> Option<Color32> {
    name.split(|c: char| !c.is_ascii_alphabetic())
        .find_map(|w| match w.to_ascii_lowercase().as_str() {
            "red" => Some(RED),
            "yellow" | "amber" => Some(YELLOW),
            "green" => Some(GREEN),
            "blue" => Some(BLUE),
            "white" => Some(WHITE),
            _ => None,
        })
}

// 2026-11-26: the fixture of lamps in the diagram's corner was removed
// (Nick: the I/O tab shows it). Only tests use what follows; the tester
// drops it with them.

/// One output drawn as a lamp.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub struct Lamp {
    pub name: String,
    /// Its colour by name; a lamp with none lights in the brand yellow.
    pub colour: Option<Color32>,
    pub on: bool,
}

/// What the diagram's corner showed for a program's BOOL outputs.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub enum Fixture {
    /// A red, a yellow and a green output, top to bottom.
    Traffic([Lamp; 3]),
    /// Every BOOL output as a pilot lamp, in source order.
    Lamps(Vec<Lamp>),
}

/// The fixture for `outputs` (name, value): the BOOL ones become lamps;
/// a traffic light when a red, a yellow and a green are among them (any
/// further lamp is left to the I/O tab); `None` with no BOOL output.
#[cfg(test)]
#[must_use]
pub fn fixture<'a>(outputs: impl IntoIterator<Item = (&'a str, Option<Value>)>) -> Option<Fixture> {
    let lamps: Vec<Lamp> = outputs
        .into_iter()
        .filter_map(|(name, v)| match v {
            Some(Value::Bool(on)) => Some(Lamp {
                name: name.to_string(),
                colour: colour(name),
                on,
            }),
            _ => None,
        })
        .collect();
    if lamps.is_empty() {
        return None;
    }
    let of = |c: Color32| lamps.iter().find(|l| l.colour == Some(c)).cloned();
    if let (Some(r), Some(y), Some(g)) = (of(RED), of(YELLOW), of(GREEN)) {
        return Some(Fixture::Traffic([r, y, g]));
    }
    Some(Fixture::Lamps(lamps))
}

#[cfg(test)]
/// Inset of the fixture from the area's bottom-right corner.
const INSET: f32 = 16.0;
/// A traffic light's lamp radius.
#[cfg(test)]
const TRAFFIC_R: f32 = 15.0;
/// Room between lamps, and around them in the housing.
#[cfg(test)]
const GAP: f32 = 9.0;
/// A pilot lamp's cell: the lamp with its name under it.
#[cfg(test)]
const CELL: Vec2 = Vec2::new(64.0, 56.0);

/// The rect the fixture takes in `area`.
#[cfg(test)]
#[must_use]
pub fn rect(area: Rect, f: &Fixture) -> Rect {
    let size = match f {
        Fixture::Traffic(_) => Vec2::new(2.0 * TRAFFIC_R + 2.0 * GAP, 3.0 * 2.0 * TRAFFIC_R + 4.0 * GAP),
        #[allow(clippy::cast_precision_loss)]
        Fixture::Lamps(l) => Vec2::new(l.len() as f32 * CELL.x + GAP, CELL.y + GAP),
    };
    Rect::from_min_size(area.max - size - Vec2::splat(INSET), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_is_one_of_the_names_words() {
        assert_eq!(colour("red"), Some(RED));
        assert_eq!(colour("GREEN"), Some(GREEN));
        assert_eq!(colour("lamp_amber"), Some(YELLOW));
        assert_eq!(colour("Blue.Q"), Some(BLUE));
        assert_eq!(colour("white2"), Some(WHITE));
        // Inside a word is not a colour.
        assert_eq!(colour("redundant"), None);
        assert_eq!(colour("agreenhouse"), None);
        assert_eq!(colour("lamp"), None);
    }

    #[test]
    fn red_yellow_and_green_outputs_make_a_traffic_light() {
        let f = fixture([
            ("GREEN", Some(Value::Bool(true))),
            ("RED", Some(Value::Bool(false))),
            ("PHASE", Some(Value::Int(0))),
            ("YELLOW", Some(Value::Bool(false))),
        ])
        .expect("a fixture");
        let Fixture::Traffic(l) = f else { panic!("{f:?}") };
        assert_eq!(
            l.iter().map(|l| (l.name.as_str(), l.on)).collect::<Vec<_>>(),
            [("RED", false), ("YELLOW", false), ("GREEN", true)]
        );
    }

    #[test]
    fn other_bool_outputs_are_pilot_lamps_and_none_is_no_fixture() {
        let f = fixture([
            ("LAMP", Some(Value::Bool(true))),
            ("RED", Some(Value::Bool(false))),
        ]);
        let Some(Fixture::Lamps(l)) = f else {
            panic!("{f:?}")
        };
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].colour, None);
        assert_eq!(l[1].colour, Some(RED));
        assert_eq!(fixture([("COUNT", Some(Value::Int(3)))]), None);
        assert_eq!(fixture([("LAMP", None)]), None, "no scan: nothing to light");
        assert_eq!(fixture([]), None);
    }

    #[test]
    fn a_trend_lane_takes_the_lamps_colour_unless_one_was_picked() {
        use crate::editor::Chrome;
        use crate::playground_view::{lane_color, palette_color};
        use crate::theme::Theme;
        let t = Theme::dark();
        let mut chrome = Chrome::default();
        assert_eq!(lane_color(&t, &chrome, "RED", 3), RED);
        assert_eq!(lane_color(&t, &chrome, "lamp_green", 3), GREEN);
        assert_eq!(
            lane_color(&t, &chrome, "RUN", 3),
            palette_color(&t, 3),
            "no colour in the name: the lane's"
        );
        chrome.set_color("red", 1);
        assert_eq!(
            lane_color(&t, &chrome, "RED", 3),
            palette_color(&t, 1),
            "a picked colour wins"
        );
        let light = Theme::light();
        assert_eq!(
            lane_color(&light, &Chrome::default(), "GREEN", 0),
            light.ink(GREEN),
            "as ink on the light page"
        );
    }

    #[test]
    fn the_fixture_sits_inside_the_areas_corner() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let traffic = fixture([
            ("RED", Some(Value::Bool(true))),
            ("YELLOW", Some(Value::Bool(false))),
            ("GREEN", Some(Value::Bool(false))),
        ])
        .unwrap();
        let r = rect(area, &traffic);
        assert!(area.contains_rect(r), "{r:?}");
        assert!((area.max.x - r.max.x - INSET).abs() < 0.01);
        assert!(r.height() > r.width(), "upright: {r:?}");
        let lamps = fixture([("A", Some(Value::Bool(true))), ("B", Some(Value::Bool(true)))]).unwrap();
        let r = rect(area, &lamps);
        assert!(r.width() > r.height(), "a row: {r:?}");
    }
}
