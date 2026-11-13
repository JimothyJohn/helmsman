//! Outputs as lamps (Nick, 2026-11-12: the program should show itself
//! without words). A BOOL output named after a colour (`red`, `lamp_green`,
//! `AMBER`) is a lamp of that colour wherever it is drawn: its value's text
//! in the I/O tab, its line on the trend, and the fixture this module paints
//! in a corner of the diagram, a real traffic light when the outputs hold a
//! red, a yellow and a green, and a row of pilot lamps otherwise.

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Vec2};
use helmsman_iec::Value;

use crate::st_view::brand::{BLACK, GRAPHITE, GREEN, RED, STEEL, WHITE, YELLOW};

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

/// One output drawn as a lamp.
#[derive(Debug, Clone, PartialEq)]
pub struct Lamp {
    pub name: String,
    /// Its colour by name; a lamp with none lights in the brand yellow.
    pub colour: Option<Color32>,
    pub on: bool,
}

impl Lamp {
    fn lit(&self) -> Color32 {
        self.colour.unwrap_or(YELLOW)
    }
}

/// What the diagram's corner shows for a program's BOOL outputs.
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

/// Inset of the fixture from the area's bottom-right corner.
const INSET: f32 = 16.0;
/// A traffic light's lamp radius, and a pilot lamp's.
const TRAFFIC_R: f32 = 15.0;
const PILOT_R: f32 = 11.0;
/// Room between lamps, and around them in the housing.
const GAP: f32 = 9.0;
/// A pilot lamp's cell: the lamp with its name under it.
const CELL: Vec2 = Vec2::new(64.0, 56.0);

/// The rect the fixture takes in `area`.
#[must_use]
pub fn rect(area: Rect, f: &Fixture) -> Rect {
    let size = match f {
        Fixture::Traffic(_) => Vec2::new(2.0 * TRAFFIC_R + 2.0 * GAP, 3.0 * 2.0 * TRAFFIC_R + 4.0 * GAP),
        #[allow(clippy::cast_precision_loss)]
        Fixture::Lamps(l) => Vec2::new(l.len() as f32 * CELL.x + GAP, CELL.y + GAP),
    };
    Rect::from_min_size(area.max - size - Vec2::splat(INSET), size)
}

/// Paints `f` in the bottom-right corner of `area`: a graphite housing on
/// either page, each lamp its colour at full strength with a halo when on
/// and nearly out when off. Returns what it painted on.
pub fn paint(painter: &egui::Painter, area: Rect, f: &Fixture) -> Rect {
    let r = rect(area, f);
    painter.rect(
        r,
        CornerRadius::same(10),
        GRAPHITE,
        Stroke::new(1.5, BLACK),
        egui::StrokeKind::Outside,
    );
    match f {
        Fixture::Traffic(lamps) => {
            for (k, l) in lamps.iter().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let y = r.top() + GAP + TRAFFIC_R + k as f32 * (2.0 * TRAFFIC_R + GAP);
                lamp(painter, Pos2::new(r.center().x, y), TRAFFIC_R, l);
            }
        }
        Fixture::Lamps(lamps) => {
            for (k, l) in lamps.iter().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let cell =
                    Rect::from_min_size(r.min + Vec2::new(GAP / 2.0 + k as f32 * CELL.x, GAP / 2.0), CELL);
                lamp(
                    painter,
                    Pos2::new(cell.center().x, cell.top() + PILOT_R + 4.0),
                    PILOT_R,
                    l,
                );
                let mut job = egui::text::LayoutJob::simple(
                    l.name.to_ascii_lowercase(),
                    FontId::monospace(11.0),
                    if l.on { WHITE } else { STEEL },
                    CELL.x - 6.0,
                );
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                job.wrap.overflow_character = Some('…');
                let galley = painter.layout_job(job);
                let at = Pos2::new(
                    cell.center().x - galley.size().x / 2.0,
                    cell.bottom() - galley.size().y - 2.0,
                );
                painter.galley(at, galley, WHITE);
            }
        }
    }
    r
}

/// One lamp: a halo and the full colour on, the colour nearly out off.
fn lamp(painter: &egui::Painter, c: Pos2, radius: f32, l: &Lamp) {
    let colour = l.lit();
    if l.on {
        painter.circle_filled(c, radius * 1.45, colour.gamma_multiply(0.28));
        painter.circle_filled(c, radius, colour);
    } else {
        painter.circle(c, radius, colour.linear_multiply(0.18), Stroke::new(1.0, BLACK));
    }
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
