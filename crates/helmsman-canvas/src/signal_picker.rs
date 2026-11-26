//! The signal picker the box page's Trend and Monitor share: the signals
//! start folded into one chip a device (and "Simulated" for placeholder
//! points, "Other" for what no map line names); a click on a device opens
//! its signals, a click on a signal flips it. What is picked is the
//! caller's: this holds only which devices are open.

use std::collections::BTreeSet;

use eframe::egui::{self, RichText};

use crate::editor::Chrome;
use crate::remote::SignalRow;
use crate::theme::theme;
use crate::tips::Tip;

/// The box's signals by where they live: one group a device of the map
/// (in the map's order), then "Simulated" (the `sim` device) and
/// "Other" (named in no map line), each with its signals' names.
#[must_use]
pub fn signal_groups(signals: &[SignalRow], map: &str) -> Vec<(String, Vec<String>)> {
    let skim = helmsman_project::iomap::skim(map);
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let (mut sim, mut other) = (Vec::new(), Vec::new());
    for s in signals {
        let dev = skim
            .signals
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(&s.name))
            .map(|m| m.device.as_str());
        match dev {
            Some("sim") => sim.push(s.name.clone()),
            None => other.push(s.name.clone()),
            Some(d) => match out.iter_mut().find(|(g, _)| g == d) {
                Some((_, v)) => v.push(s.name.clone()),
                None => out.push((d.to_string(), vec![s.name.clone()])),
            },
        }
    }
    out.sort_by_key(|(g, _)| {
        skim.devices
            .iter()
            .position(|d| d.name == *g)
            .unwrap_or(usize::MAX)
    });
    if !sim.is_empty() {
        out.push(("Simulated".into(), sim));
    }
    if !other.is_empty() {
        out.push(("Other".into(), other));
    }
    out
}

/// Which groups are open.
#[derive(Debug, Default, Clone)]
pub struct Picker {
    pub open: BTreeSet<String>,
    /// Whether a first group was already offered open (the Monitor does
    /// it once); after that the user's folds and unfolds are all there is.
    pub seeded: bool,
}

impl Picker {
    /// Draws the heading and one chip a group, then the open groups'
    /// signals, in two wrapped rows. `on` says whether a signal is picked;
    /// `heading_tip` is the heading's. Hits: `{key} group <device>` and
    /// `{key} signal <name>` (`pin <name>` is kept for the Trend's own
    /// when `key` is `trend`). Returns the signal clicked, if one was.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &mut Chrome,
        key: &str,
        groups: &[(String, Vec<String>)],
        on: &dyn Fn(&str) -> bool,
        heading_tip: &str,
    ) -> Option<String> {
        let t = theme(ui);
        let mut flip = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Signals").strong().color(t.muted))
                .tip(heading_tip);
            for (g, names) in groups {
                let open = self.open.contains(g);
                let picked = names.iter().filter(|n| on(n)).count();
                let label = if picked > 0 {
                    format!("{} {} ({picked} on)", g.to_ascii_lowercase(), names.len())
                } else {
                    format!("{} {}", g.to_ascii_lowercase(), names.len())
                };
                // A framed button with a fold arrow, so it reads as
                // something to press, not as a caption.
                let arrow = if open { "▾" } else { "▸" };
                let text = RichText::new(format!("{arrow} {label}")).color(t.fg);
                let r = ui
                    .add(egui::Button::new(text).selected(open)) // tip: given below
                    .tip(if open {
                        "Click to fold this device's signals away."
                    } else {
                        "Click to see this device's signals."
                    });
                chrome.hit(format!("{key} group {}", g.to_ascii_lowercase()), r.rect);
                if r.clicked() && !self.open.remove(g) {
                    self.open.insert(g.clone());
                }
            }
        });
        self.open.retain(|g| groups.iter().any(|(n, _)| n == g));
        let shown: Vec<&String> = groups
            .iter()
            .filter(|(g, _)| self.open.contains(g))
            .flat_map(|(_, names)| names)
            .collect();
        if !shown.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for name in shown {
                    let is_on = on(name);
                    let r = crate::theme::tab(ui, is_on, &name.to_ascii_lowercase(), egui::Vec2::ZERO).tip(
                        if is_on {
                            "Picked. Click to take it off."
                        } else {
                            "Click to pick it."
                        },
                    );
                    let hit = if key == "trend" {
                        format!("pin {name}")
                    } else {
                        format!("{key} signal {}", name.to_ascii_lowercase())
                    };
                    chrome.hit(hit, r.rect);
                    if r.clicked() {
                        flip = Some(name.clone());
                    }
                }
            });
        }
        flip
    }
}
