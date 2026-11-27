//! Outputs as lamps (Nick, 2026-11-12: the program should show itself
//! without words). A BOOL output named after a colour (`red`, `lamp_green`,
//! `AMBER`) is a lamp of that colour wherever it is drawn: its value's text
//! in the I/O tab and its line on the trend. (The fixture in the diagram's
//! corner was removed, Nick, 2026-11-26.)

use eframe::egui::Color32;

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
}
