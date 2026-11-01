//! The Messages tab along the bottom, beside the Trend (Nick, 2026-11-01):
//! what the page had to say recently, as info, warnings and errors, so a
//! fault or a refused deploy is still there to read after the notice line
//! under the header has gone. Bounded ([`KEEP`]); newest last.

use std::collections::VecDeque;

use eframe::egui::{self, RichText};

use crate::st_view::brand::{GREEN, RED, STEEL, WARN, WHITE};

/// How many messages the log keeps.
pub const KEEP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Page time, seconds.
    pub at: f64,
    pub level: Level,
    pub text: String,
}

/// The bounded log.
#[derive(Debug)]
pub struct Log {
    items: VecDeque<Message>,
    /// Which levels the tab shows: info, warning, error (all, to start).
    pub show: [bool; 3],
}

impl Default for Log {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            show: [true; 3],
        }
    }
}

const fn slot(level: Level) -> usize {
    match level {
        Level::Info => 0,
        Level::Warning => 1,
        Level::Error => 2,
    }
}

impl Log {
    /// Adds a message; the oldest goes once [`KEEP`] is passed. The same
    /// text twice in a row is kept once (a notice repeats every frame).
    pub fn push(&mut self, at: f64, level: Level, text: impl Into<String>) {
        let text = text.into();
        if self
            .items
            .back()
            .is_some_and(|m| m.level == level && m.text == text)
        {
            return;
        }
        self.items.push_back(Message { at, level, text });
        while self.items.len() > KEEP {
            self.items.pop_front();
        }
    }

    pub fn info(&mut self, at: f64, text: impl Into<String>) {
        self.push(at, Level::Info, text);
    }

    pub fn warning(&mut self, at: f64, text: impl Into<String>) {
        self.push(at, Level::Warning, text);
    }

    pub fn error(&mut self, at: f64, text: impl Into<String>) {
        self.push(at, Level::Error, text);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn iter(&self) -> impl Iterator<Item = &Message> {
        self.items.iter()
    }

    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many are errors, for the tab's badge.
    #[must_use]
    pub fn errors(&self) -> usize {
        self.items.iter().filter(|m| m.level == Level::Error).count()
    }

    /// The tab's label: "Messages", or "Messages · 2 errors".
    #[must_use]
    pub fn label(&self) -> String {
        match self.errors() {
            0 => "Messages".into(),
            1 => "Messages · 1 error".into(),
            n => format!("Messages · {n} errors"),
        }
    }

    /// The messages the filters let through.
    pub fn shown(&self) -> impl Iterator<Item = &Message> {
        self.items.iter().filter(|m| self.show[slot(m.level)])
    }

    /// How many of each level: info, warning, error.
    #[must_use]
    pub fn counts(&self) -> [usize; 3] {
        let mut n = [0; 3];
        for m in &self.items {
            n[slot(m.level)] += 1;
        }
        n
    }

    /// The filter row (Nick, 2026-11-01: see any or all of info, warnings
    /// and errors) and the list, newest at the bottom and scrolled there.
    pub fn ui(&mut self, ui: &mut egui::Ui, chrome: &mut crate::editor::Chrome) {
        let counts = self.counts();
        ui.horizontal(|ui| {
            for (k, (name, color)) in [("info", GREEN), ("warnings", WARN), ("errors", RED)]
                .into_iter()
                .enumerate()
            {
                let text = RichText::new(format!("{name} {}", counts[k])).color(color);
                let r = ui.add(egui::Button::selectable(self.show[k], text));
                chrome.hit(format!("messages {name}"), r.rect);
                if r.clicked() {
                    self.show[k] = !self.show[k];
                }
            }
        });
        if self.items.is_empty() {
            ui.label(RichText::new("Nothing to report yet.").color(STEEL));
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("messages")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for m in self.shown() {
                    let (tag, color) = match m.level {
                        Level::Info => ("info", GREEN),
                        Level::Warning => ("warning", WARN),
                        Level::Error => ("error", RED),
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{:>8.1}s", m.at)).monospace().color(STEEL));
                        ui.label(
                            RichText::new(format!("{tag:<7}"))
                                .monospace()
                                .strong()
                                .color(color),
                        );
                        ui.label(RichText::new(&m.text).color(WHITE));
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{KEEP, Level, Log};

    #[test]
    fn the_log_is_bounded_dedupes_repeats_and_counts_errors() {
        let mut l = Log::default();
        assert!(l.is_empty() && l.label() == "Messages");
        l.info(1.0, "a");
        l.info(1.5, "a");
        assert_eq!(l.len(), 1, "a repeat is kept once");
        l.error(2.0, "b");
        l.error(2.5, "c");
        assert_eq!(l.label(), "Messages · 2 errors");
        l.warning(3.0, "d");
        assert_eq!(l.errors(), 2);
        for k in 0..(KEEP + 50) {
            l.info(4.0, format!("m{k}"));
        }
        assert_eq!(l.len(), KEEP, "the oldest go");
        assert_eq!(l.iter().next().unwrap().level, Level::Info);
        assert!(l.iter().next().unwrap().text.starts_with('m'), "b, c, d are gone");
    }

    /// Nick, 2026-11-01: see any or all of info, warnings and errors.
    #[test]
    fn the_filters_pick_levels_and_count_each() {
        let mut l = Log::default();
        l.info(1.0, "a");
        l.warning(2.0, "b");
        l.error(3.0, "c");
        l.error(4.0, "d");
        assert_eq!(l.counts(), [1, 1, 2]);
        assert_eq!(l.shown().count(), 4, "all to start");
        l.show = [false, false, true];
        let shown: Vec<&str> = l.shown().map(|m| m.text.as_str()).collect();
        assert_eq!(shown, ["c", "d"]);
        l.show = [true, true, false];
        let shown: Vec<&str> = l.shown().map(|m| m.text.as_str()).collect();
        assert_eq!(shown, ["a", "b"]);
        l.show = [false; 3];
        assert_eq!(l.shown().count(), 0);
    }
}
