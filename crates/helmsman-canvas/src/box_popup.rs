//! What the page is doing about the box, in the middle of the screen
//! (Nick, 2026-11-13: "a popup that makes you highly aware of what's
//! happening"). Four states, in the middle of the screen: the two that
//! ask something (No box, Found) as modals, the two that only say
//! something (Connecting, Lost) as a window that leaves the page usable:
//!
//! - **Connecting**: the first look for the box is taking a moment.
//! - **No box**: the first look found nothing, so the page simulates;
//!   a second look runs in the background once the simulation is up, and
//!   the popup says how that went.
//! - **Found**: the second look found a box; connect, or keep simulating.
//! - **Lost**: a box that answered went away; the page is reconnecting.
//!
//! [`which`] decides from the link alone (tested natively); [`show`]
//! draws it and says what was pressed.

use eframe::egui::{self, RichText};

use crate::editor::Chrome;
use crate::remote::{Link, Probe};
use crate::theme::theme;
use crate::tips::Tip;

/// How long the first look may take before the Connecting popup shows,
/// s: a box that answers at once never flashes it.
pub const CONNECTING_AFTER: f64 = 0.4;

/// How long after the simulation starts the second look goes, s.
pub const PROBE_AFTER: f64 = 2.0;

/// The popup to show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Popup {
    Connecting,
    /// The page fell back to the simulation; the second look's state.
    NoBox(Probe),
    Found,
    /// Reconnecting (`at` the next try, page time), or `unreachable` after
    /// several failures.
    Lost {
        at: f64,
        failures: u32,
        unreachable: bool,
    },
}

/// What was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// The No box popup is read.
    Ok,
    /// Leave the simulation for the box that was found.
    Connect,
    /// Stay in the simulation.
    Keep,
    /// The Lost popup out of the way until the box is back.
    Hide,
    /// Leave a lost box for the simulation.
    Simulate,
}

/// Which of the two asking popups is open, while the page simulates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Asking {
    #[default]
    None,
    /// The page fell back to the simulation.
    NoBox,
    /// The second look found a box.
    Found,
}

/// What the App knows this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    /// The page watches the box (not the simulation).
    pub on_box: bool,
    pub link: Link,
    pub ever_connected: bool,
    /// Seconds since the page started watching the box.
    pub waited: f64,
    /// The asking popup open, if any.
    pub asking: Asking,
    pub probe: Probe,
    /// The Lost popup was hidden since the box was last connected.
    pub lost_hidden: bool,
}

/// The popup for `s`, if any.
#[must_use]
pub fn which(s: &Seen) -> Option<Popup> {
    if !s.on_box {
        return match s.asking {
            Asking::None => None,
            Asking::NoBox => Some(Popup::NoBox(s.probe)),
            Asking::Found => Some(Popup::Found),
        };
    }
    match s.link {
        Link::Connecting if !s.ever_connected && s.waited >= CONNECTING_AFTER => Some(Popup::Connecting),
        Link::Reconnecting { failures, at } if s.ever_connected && !s.lost_hidden => Some(Popup::Lost {
            at,
            failures,
            unreachable: false,
        }),
        Link::Unreachable { failures, at } if s.ever_connected && !s.lost_hidden => Some(Popup::Lost {
            at,
            failures,
            unreachable: true,
        }),
        _ => None,
    }
}

/// The popup's title and its sentences.
#[must_use]
pub fn text(p: Popup, now: f64, here: &str) -> (String, Vec<String>) {
    match p {
        Popup::Connecting => (
            "Connecting to the box".into(),
            vec![format!("Looking for a box at {here}.")],
        ),
        Popup::NoBox(probe) => {
            let mut lines = vec![
                format!("No box answered at {here}."),
                "Your program runs in this browser instead, against simulated inputs and outputs.".into(),
            ];
            lines.push(
                match probe {
                    Probe::Due(_) | Probe::Asked | Probe::Off => "Looking once more in the background.",
                    Probe::Missed => {
                        "A second look found no box either. View › Back to this box tries again."
                    }
                    Probe::Found => "A second look found a box.",
                }
                .into(),
            );
            ("Simulating in this browser".into(), lines)
        }
        Popup::Found => (
            "Box found".into(),
            vec![
                format!("A box answered at {here} on the second look."),
                "Your simulated program stays in this browser if you connect.".into(),
            ],
        ),
        Popup::Lost {
            at,
            failures,
            unreachable,
        } => {
            let secs = (at - now).max(0.0).ceil();
            let mut lines = vec![
                format!("The connection to the box at {here} dropped."),
                "Its programs keep running on the box. This page only lost sight of them.".into(),
                format!("Trying again in {secs:.0} s (try {}).", failures + 1),
            ];
            if unreachable {
                lines.push(
                    "It has not answered for a while: you can simulate in this browser meanwhile.".into(),
                );
            }
            (
                if unreachable {
                    "Box unreachable".into()
                } else {
                    "Reconnecting to the box".into()
                },
                lines,
            )
        }
    }
}

/// Draws `p` in the middle of the screen; what was pressed. `here` is
/// the box's address in words.
pub fn show(ctx: &egui::Context, chrome: &mut Chrome, p: Popup, here: &str) -> Option<Act> {
    let now = ctx.input(|i| i.time);
    let (title, lines) = text(p, now, here);
    let mut act = None;
    let body = |ui: &mut egui::Ui| {
        let t = theme(ui);
        ui.set_max_width(460.0);
        ui.horizontal(|ui| {
            if matches!(
                p,
                Popup::Connecting | Popup::Lost { .. } | Popup::NoBox(Probe::Due(_) | Probe::Asked)
            ) {
                ui.spinner();
            }
            ui.label(RichText::new(&title).strong().size(22.0).color(t.fg))
                .tip("What the page is doing about the box, right now.");
        });
        ui.add_space(6.0);
        for l in &lines {
            ui.label(RichText::new(l).size(18.0).color(t.fg));
        }
        ui.add_space(10.0);
        let mut button = |ui: &mut egui::Ui, key: &str, label: &str, tip: &str, a: Act| {
            let r = ui.button(RichText::new(label).size(18.0)).tip(tip);
            chrome.hit(key, r.rect);
            if r.clicked() {
                act = Some(a);
            }
        };
        ui.horizontal_wrapped(|ui| match p {
            Popup::Connecting => {}
            Popup::NoBox(_) => button(
                ui,
                "popup ok",
                "OK",
                "Closes this. The simulation is already running.",
                Act::Ok,
            ),
            Popup::Found => {
                button(
                    ui,
                    "popup connect",
                    "Connect to the box",
                    "Leaves the simulation and shows the box's own programs, live.",
                    Act::Connect,
                );
                button(
                    ui,
                    "popup keep",
                    "Keep simulating",
                    "Stays in the simulation. View › Back to this box connects later.",
                    Act::Keep,
                );
            }
            Popup::Lost { unreachable, .. } => {
                button(
                    ui,
                    "popup hide",
                    "Hide",
                    "Hides this until the box is back. The status line keeps counting down.",
                    Act::Hide,
                );
                if unreachable {
                    button(
                        ui,
                        "popup simulate",
                        "Simulate in this browser",
                        "Runs the program in this page instead. The box keeps running its own.",
                        Act::Simulate,
                    );
                }
            }
        });
    };
    if matches!(p, Popup::Connecting | Popup::Lost { .. }) {
        // Said, not asked: the page behind stays usable (the last diagram
        // of a lost box is still worth reading), so not a modal.
        egui::Window::new("box-status")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 90.0))
            .order(egui::Order::Foreground)
            .show(ctx, body);
    } else {
        egui::Modal::new(egui::Id::unique("box-popup")).show(ctx, body);
    }
    if matches!(p, Popup::Connecting | Popup::Lost { .. }) {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen() -> Seen {
        Seen {
            on_box: true,
            link: Link::Connecting,
            ever_connected: false,
            waited: 0.0,
            asking: Asking::None,
            probe: Probe::Off,
            lost_hidden: false,
        }
    }

    #[test]
    fn a_quick_box_never_flashes_the_connecting_popup() {
        assert_eq!(which(&seen()), None);
        let slow = Seen {
            waited: CONNECTING_AFTER,
            ..seen()
        };
        assert_eq!(which(&slow), Some(Popup::Connecting));
        let up = Seen {
            link: Link::Connected,
            ever_connected: true,
            ..slow
        };
        assert_eq!(which(&up), None);
    }

    #[test]
    fn a_lost_box_says_so_until_hidden_or_back() {
        let lost = Seen {
            link: Link::Reconnecting {
                failures: 1,
                at: 12.0,
            },
            ever_connected: true,
            ..seen()
        };
        assert_eq!(
            which(&lost),
            Some(Popup::Lost {
                at: 12.0,
                failures: 1,
                unreachable: false
            })
        );
        let (title, lines) = text(which(&lost).unwrap(), 10.5, "10.0.0.41:8081");
        assert_eq!(title, "Reconnecting to the box");
        assert!(
            lines.iter().any(|l| l == "Trying again in 2 s (try 2)."),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("keep running on the box")),
            "{lines:?}"
        );
        assert_eq!(
            which(&Seen {
                lost_hidden: true,
                ..lost
            }),
            None
        );
        let gone = Seen {
            link: Link::Unreachable {
                failures: 3,
                at: 14.0,
            },
            ..lost
        };
        assert!(matches!(
            which(&gone),
            Some(Popup::Lost {
                unreachable: true,
                ..
            })
        ));
        // A box that never answered is the fallback's business, not this.
        assert_eq!(
            which(&Seen {
                ever_connected: false,
                ..lost
            }),
            None
        );
    }

    #[test]
    fn the_simulation_shows_the_no_box_popup_then_what_the_second_look_found() {
        let sim = Seen {
            on_box: false,
            asking: Asking::NoBox,
            probe: Probe::Due(3.0),
            ..seen()
        };
        assert_eq!(which(&sim), Some(Popup::NoBox(Probe::Due(3.0))));
        let (_, lines) = text(which(&sim).unwrap(), 0.0, "this address");
        assert!(
            lines.iter().any(|l| l == "Looking once more in the background."),
            "{lines:?}"
        );
        let (_, lines) = text(Popup::NoBox(Probe::Missed), 0.0, "this address");
        assert!(
            lines.iter().any(|l| l.contains("found no box either")),
            "{lines:?}"
        );
        assert_eq!(
            which(&Seen {
                asking: Asking::Found,
                ..sim
            }),
            Some(Popup::Found)
        );
        assert_eq!(
            which(&Seen {
                asking: Asking::None,
                ..sim
            }),
            None
        );
    }
}
