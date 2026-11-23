//! The Messages tab along the bottom, beside the Trend (Nick, 2026-11-01):
//! what the page had to say recently, as info, warnings and errors, so a
//! fault or a refused deploy is still there to read after the notice line
//! under the header has gone. Bounded ([`KEEP`]); newest last.

use std::collections::VecDeque;

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use crate::glyphs::Icon;

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
    /// The line the tab shows.
    pub text: String,
    /// The whole of it when the line is a summary (an Agent outcome):
    /// the row's tooltip, Copy all and the Help request carry this.
    pub detail: Option<String>,
}

impl Message {
    /// The whole text: [`Self::detail`], else the line.
    #[must_use]
    pub fn full(&self) -> &str {
        self.detail.as_deref().unwrap_or(&self.text)
    }
}

/// The bounded log.
#[derive(Debug)]
pub struct Log {
    items: VecDeque<Message>,
    /// Which levels the tab shows: info, warning, error (errors only, to
    /// start: Nick, 2026-11-23). Hidden levels are still kept and counted.
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
            show: [false, false, true],
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
        self.push_message(at, level, text.into(), None);
    }

    /// [`Self::push`] a short `text` that stands for `detail`, the whole
    /// of it.
    pub fn push_detail(&mut self, at: f64, level: Level, text: impl Into<String>, detail: impl Into<String>) {
        let text = text.into();
        let detail = detail.into();
        let detail = (detail != text).then_some(detail);
        self.push_message(at, level, text, detail);
    }

    /// An Agent outcome: a warning when its checks did not pass, an error
    /// when it was refused or failed (Nick, 2026-11-13: errors go there),
    /// else info; the short line shown, the whole text in the detail.
    pub fn outcome(&mut self, at: f64, o: &crate::instruct::Outcome) {
        use crate::instruct::Kind;
        let (line, full) = o.log_entry();
        let level = match o.kind {
            Kind::Applied if !o.problems.is_empty() => Level::Warning,
            Kind::Applied | Kind::Undone | Kind::Said => Level::Info,
            Kind::Refused | Kind::Failed => Level::Error,
        };
        self.push_detail(at, level, line, full);
    }

    fn push_message(&mut self, at: f64, level: Level, text: String, detail: Option<String>) {
        if self
            .items
            .back()
            .is_some_and(|m| m.level == level && m.text == text && m.detail == detail)
        {
            return;
        }
        if level == Level::Error {
            self.error_at = Some(at);
            self.unseen += 1;
            self.fresh = true;
        }
        self.items.push_back(Message {
            at,
            level,
            text,
            detail,
        });
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

    /// Empties the log and its counts: what Clear does. A pulse in
    /// progress ends with it.
    pub fn clear(&mut self) {
        self.items.clear();
        self.error_at = None;
        self.unseen = 0;
        self.fresh = false;
    }

    /// The shown messages as plain text, one a line, as Copy all copies
    /// them: `  12.3s error   the text`, the whole text of each.
    #[must_use]
    pub fn text(&self) -> String {
        Self::lines(self.shown())
    }

    /// Every message, shown or not, as [`Self::text`]: what the box's
    /// Help gets with a question.
    #[must_use]
    pub fn full_text(&self) -> String {
        Self::lines(self.items.iter())
    }

    fn lines<'a>(items: impl Iterator<Item = &'a Message>) -> String {
        items
            .map(|m| {
                let tag = match m.level {
                    Level::Info => "info",
                    Level::Warning => "warning",
                    Level::Error => "error",
                };
                format!("{:>8.1}s {tag:<7} {}", m.at, m.full())
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
            for (k, (name, icon, color)) in [
                ("info", Icon::Info, crate::theme::theme(ui).ink(GREEN)),
                ("warnings", Icon::Warning, crate::theme::theme(ui).ink(WARN)),
                ("errors", Icon::Error, crate::theme::theme(ui).ink(RED)),
            ]
            .into_iter()
            .enumerate()
            {
                // Its icon and count (Nick, 2026-11-13: icons, not words),
                // in its colour while the level is listed, grey while it
                // is left out; a click switches it.
                let ink = if self.show[k] { color } else { theme(ui).muted };
                let r = level_toggle(ui, icon, counts[k], ink, self.show[k]).tip(format!(
                    "{}: {} of them. Click to {} them.",
                    capital(name),
                    counts[k],
                    if self.show[k] { "leave out" } else { "list" }
                ));
                if r.clicked() {
                    self.show[k] = !self.show[k];
                }
                chrome.hit(format!("messages {name}"), r.rect);
            }
            if !self.items.is_empty() {
                let r = crate::glyphs::button_sized(
                    ui,
                    Icon::Copy,
                    FILTER_ICON + 8.0,
                    true,
                    false,
                    "Copy all: copies the messages shown, in full, as text, to paste into a bug report or into the Describe tab.",
                );
                chrome.hit("messages copy", r.rect);
                if r.clicked() {
                    ui.ctx().copy_text(self.text());
                }
                let r = crate::glyphs::button_sized(
                    ui,
                    Icon::Clear,
                    FILTER_ICON + 8.0,
                    true,
                    false,
                    "Clear: empties the log, hidden levels too, and its counts.",
                );
                chrome.hit("messages clear", r.rect);
                if r.clicked() {
                    self.clear();
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
                    let (tag, icon, color) = match m.level {
                        Level::Info => ("Info", Icon::Info, crate::theme::theme(ui).ink(GREEN)),
                        Level::Warning => ("Warning", Icon::Warning, crate::theme::theme(ui).ink(WARN)),
                        Level::Error => ("Error", Icon::Error, crate::theme::theme(ui).ink(RED)),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("{:>8.1}s", m.at))
                                .monospace()
                                .color(t.muted),
                        );
                        let (rect, r) = ui.allocate_exact_size(Vec2::splat(ROW_ICON), Sense::hover());
                        crate::glyphs::paint(ui.painter(), rect.shrink(1.0), icon, color);
                        r.tip(tag);
                        let row =
                            ui.add(egui::Label::new(RichText::new(&m.text).color(t.fg)).selectable(true));
                        if m.detail.is_some() {
                            row.tip(m.full());
                        }
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
            .tip("Copies this message, to paste into a bug report or into the Describe tab.");
        chrome.hit(format!("{key} copy"), b.rect);
        if b.clicked() {
            ui.ctx().copy_text(text.to_string());
        }
        r
    })
    .inner
}

/// The level icon's side in a message row, points.
const ROW_ICON: f32 = 20.0;
/// The level icon's side in the filter row, points.
const FILTER_ICON: f32 = 24.0;

/// A level's filter: its icon and its count side by side, one click
/// target, tinted under the pointer, underlined while the level shows.
fn level_toggle(ui: &mut egui::Ui, icon: Icon, count: usize, ink: Color32, on: bool) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(count.to_string(), font, ink));
    let pad = 4.0;
    let size = Vec2::new(
        pad + FILTER_ICON + 4.0 + galley.size().x + pad,
        FILTER_ICON.max(galley.size().y) + 2.0 * pad,
    );
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    if r.hovered() {
        let tint = ui.style().interact(&r).bg_fill;
        ui.painter().rect_filled(rect, CornerRadius::same(4), tint);
    }
    let icon_rect = Rect::from_min_size(
        Pos2::new(rect.left() + pad, rect.center().y - FILTER_ICON / 2.0),
        Vec2::splat(FILTER_ICON),
    );
    crate::glyphs::paint(ui.painter(), icon_rect, icon, ink);
    let text_at = Pos2::new(icon_rect.right() + 4.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(text_at, galley, ink);
    if on {
        ui.painter().hline(
            (rect.left() + pad)..=(rect.right() - pad),
            rect.bottom() - 1.0,
            Stroke::new(2.0, ink),
        );
    }
    r
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

#[cfg(test)]
mod tests {
    use super::{KEEP, Level, Log};
    use crate::instruct::{Kind, Outcome};

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
        assert_eq!(l.show, [false, false, true], "errors only to start");
        let shown: Vec<&str> = l.shown().map(|m| m.text.as_str()).collect();
        assert_eq!(shown, ["c", "d"]);
        l.show = [true; 3];
        assert_eq!(l.shown().count(), 4);
        l.show = [false, false, true];
        let shown: Vec<&str> = l.shown().map(|m| m.text.as_str()).collect();
        assert_eq!(shown, ["c", "d"]);
        l.show = [true, true, false];
        let shown: Vec<&str> = l.shown().map(|m| m.text.as_str()).collect();
        assert_eq!(shown, ["a", "b"]);
        l.show = [false; 3];
        assert_eq!(l.shown().count(), 0);
    }

    /// The log keeps the whole text of an outcome; the line is short.
    #[test]
    fn an_outcome_line_is_short_and_the_whole_is_kept() {
        let mut l = Log::default();
        let mut o = Outcome::new(Kind::Refused, "Not changed:");
        o.why = Some("Why it was tried.".into());
        o.problems = vec!["x".repeat(500), "second".into(), "third".into()];
        l.outcome(1.0, &o);
        let m = l.iter().next().unwrap();
        assert_eq!(m.level, Level::Error);
        assert!(
            m.text.chars().count() <= crate::instruct::LOG_SUMMARY,
            "{}",
            m.text
        );
        let full = m.full();
        assert!(full.contains("Why: Why it was tried.") && full.contains("third"));
        assert!(full.contains(&"x".repeat(500)), "nothing cut from the whole");
        assert!(l.text().contains("third"), "Copy all has the whole");
        // Hidden levels still reach the box's Help request.
        let mut a = Outcome::new(Kind::Applied, "Updated main.st.");
        a.why = Some("Because.".into());
        l.outcome(2.0, &a);
        assert_eq!(l.shown().count(), 1, "info is not shown");
        assert!(l.full_text().contains("Updated main.st. Why: Because."));
        assert_eq!(l.counts(), [1, 0, 1]);
    }

    #[test]
    fn clear_empties_the_log_and_its_counts() {
        let mut l = Log::default();
        l.info(1.0, "a");
        l.error(2.0, "b");
        assert!(l.flashing(2.5) && l.errors() == 1);
        l.clear();
        assert!(l.is_empty() && l.counts() == [0; 3] && l.errors() == 0);
        assert!(!l.flashing(2.5) && l.alarm(2.5, false).is_none());
        assert!(!l.take_fresh());
        assert!(l.text().is_empty());
        l.error(3.0, "b");
        assert_eq!(l.errors(), 1, "an error after a clear counts again");
        assert!(l.alarm(3.1, false).is_some());
    }
}
