//! The Messages tab along the bottom, beside the Trend (Nick, 2026-11-01):
//! what the page had to say recently, as info, warnings and errors, so a
//! fault or a refused deploy is still there to read after the notice line
//! under the header has gone. Bounded ([`KEEP`]); newest last.

use std::collections::VecDeque;

use eframe::egui::{self, RichText};

use crate::st_view::brand::{GREEN, RED, WARN};
use crate::theme::theme;
use crate::tips::Tip;

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
    /// When the newest error came in, page time.
    error_at: Option<f64>,
    /// Errors that came in since the tab was last on screen.
    unseen: usize,
    /// An error came in and the Editor has not been told yet
    /// ([`Self::take_fresh`]).
    fresh: bool,
}

/// How long the Messages tab pulses after an error comes in, seconds.
pub const FLASH: f64 = 4.0;

impl Default for Log {
    fn default() -> Self {
        Self {
            items: VecDeque::new(),
            show: [true; 3],
            error_at: None,
            unseen: 0,
            fresh: false,
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
        if level == Level::Error {
            self.error_at = Some(at);
            self.unseen += 1;
            self.fresh = true;
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

    /// Whether an error came in since this was last asked.
    pub fn take_fresh(&mut self) -> bool {
        std::mem::take(&mut self.fresh)
    }

    /// Whether the tab is pulsing at `now`: an error came in within
    /// [`FLASH`].
    #[must_use]
    pub fn flashing(&self, now: f64) -> bool {
        self.error_at.is_some_and(|at| (0.0..FLASH).contains(&(now - at)))
    }

    /// How red the Messages tab is at `now`, 0 to 1: `None` with nothing
    /// wrong, a steady red while an error stands (`standing`) or one has
    /// come in since the tab was last on screen, and a slow pulse on top
    /// for [`FLASH`] after a new one.
    #[must_use]
    pub fn alarm(&self, now: f64, standing: bool) -> Option<f32> {
        const STEADY: f32 = 0.3;
        if let Some(at) = self.error_at.filter(|_| self.flashing(now)) {
            // Three even swells a second and a half apart, from steady.
            #[allow(clippy::cast_possible_truncation)]
            let swell = (0.5 - 0.5 * ((now - at) * std::f64::consts::TAU / 1.33).cos()) as f32;
            return Some(STEADY + (1.0 - STEADY) * swell);
        }
        (standing || self.unseen > 0).then_some(STEADY)
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

    /// The shown messages as plain text, one a line, as Copy all copies
    /// them: `  12.3s error   the text`.
    #[must_use]
    pub fn text(&self) -> String {
        self.shown()
            .map(|m| {
                let tag = match m.level {
                    Level::Info => "info",
                    Level::Warning => "warning",
                    Level::Error => "error",
                };
                format!("{:>8.1}s {tag:<7} {}", m.at, m.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The filter row (Nick, 2026-11-01: see any or all of info, warnings
    /// and errors) and the list, newest at the bottom and scrolled there.
    /// The rows are selectable and Copy all puts them on the clipboard
    /// (Nick, 2026-11-07: errors must paste into a bug report).
    pub fn ui(&mut self, ui: &mut egui::Ui, chrome: &mut crate::editor::Chrome) {
        self.unseen = 0;
        let counts = self.counts();
        ui.horizontal_wrapped(|ui| {
            for (k, (name, color)) in [
                ("info", crate::theme::theme(ui).ink(GREEN)),
                ("warnings", crate::theme::theme(ui).ink(WARN)),
                ("errors", crate::theme::theme(ui).ink(RED)),
            ]
            .into_iter()
            .enumerate()
            {
                // A tick box (Nick, 2026-11-11: what a toggle does must be
                // plain): ticked and in its colour while the level shows,
                // empty and grey while it is hidden.
                let ink = if self.show[k] { color } else { theme(ui).muted };
                let text = RichText::new(format!("{name} {}", counts[k])).color(ink);
                let r = ui.checkbox(&mut self.show[k], text).tip(format!(
                    "Ticked: the {name} are listed. Unticked: they are left out."
                ));
                chrome.hit(format!("messages {name}"), r.rect);
            }
            if !self.items.is_empty() {
                let r = ui
                    .button("Copy all")
                    .tip("Copies the messages shown, as text, to paste into a bug report or to the Agent.");
                chrome.hit("messages copy", r.rect);
                if r.clicked() {
                    ui.ctx().copy_text(self.text());
                }
            }
        });
        let t = theme(ui);
        if self.items.is_empty() {
            ui.label(RichText::new("Nothing to report yet.").color(t.muted));
            // The panel keeps the height it has on the Trend tab.
            ui.allocate_space(ui.available_size());
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("messages")
            // Down to the panel's edge however few the rows: the panel
            // keeps its height from one tab to the other.
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for m in self.shown() {
                    let (tag, color) = match m.level {
                        Level::Info => ("info", crate::theme::theme(ui).ink(GREEN)),
                        Level::Warning => ("warning", crate::theme::theme(ui).ink(WARN)),
                        Level::Error => ("error", crate::theme::theme(ui).ink(RED)),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("{:>8.1}s", m.at))
                                .monospace()
                                .color(t.muted),
                        );
                        ui.label(
                            RichText::new(format!("{tag:<7}"))
                                .monospace()
                                .strong()
                                .color(color),
                        );
                        ui.add(egui::Label::new(RichText::new(&m.text).color(t.fg)).selectable(true));
                    });
                }
            });
    }
}

/// An error or notice line someone may need to paste elsewhere: the text,
/// selectable, and a Copy button after it that puts the whole text on the
/// clipboard (Nick, 2026-11-07). Labels are otherwise not selectable
/// (theme.rs). The hits are `key` (the text) and `<key> copy`.
pub fn copyable(
    ui: &mut egui::Ui,
    chrome: &mut crate::editor::Chrome,
    key: &str,
    text: &str,
    color: egui::Color32,
) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        let r = ui.add(egui::Label::new(RichText::new(text).color(color)).selectable(true));
        chrome.hit(key, r.rect);
        let b = ui
            .button("Copy")
            .tip("Copies this message, to paste into a bug report or to the Agent.");
        chrome.hit(format!("{key} copy"), b.rect);
        if b.clicked() {
            ui.ctx().copy_text(text.to_string());
        }
        r
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::{KEEP, Level, Log};

    #[test]
    fn the_log_is_bounded_dedupes_repeats_and_counts_errors() {
        let mut l = Log::default();
        assert!(l.is_empty() && l.errors() == 0);
        l.info(1.0, "a");
        l.info(1.5, "a");
        assert_eq!(l.len(), 1, "a repeat is kept once");
        l.error(2.0, "b");
        l.error(2.5, "c");
        assert_eq!(l.errors(), 2);
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
