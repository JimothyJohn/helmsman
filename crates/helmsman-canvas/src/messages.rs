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
#[derive(Debug, Default)]
pub struct Log {
    items: VecDeque<Message>,
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

    /// The list, newest at the bottom and scrolled there.
    pub fn ui(&self, ui: &mut egui::Ui) {
        if self.items.is_empty() {
            ui.label(RichText::new("Nothing to report yet.").color(STEEL));
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("messages")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for m in &self.items {
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
}
