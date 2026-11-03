//! A project's history in the Files tab (Nick 2026-11-01,
//! `project-history`; interview 2026-11-02: revisions on the box, not
//! git): a "History" row that folds the revisions, newest first, each
//! with who, when and what; a click opens the revision in the middle of
//! the screen with its Agent page and a line diff of each file against
//! the project as it is, and Restore for an engineer. The same rows and
//! modal serve the box (`crate::box_projects`, from `/api/projects/{p}/
//! history`) and the simulation (`crate::sim_projects`, revisions in the
//! browser's storage).

use std::collections::BTreeMap;

use eframe::egui::{self, Color32, RichText, Vec2};

use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;

const RED: Color32 = Color32::from_rgb(255, 110, 100);
const GREEN: Color32 = Color32::from_rgb(90, 200, 120);
/// Longest file a diff is drawn for, lines each side; past it the file is
/// shown as replaced.
pub const MAX_DIFF_LINES: usize = 3000;
/// Longest Agent page shown in the modal, characters.
const MAX_PAGE_SHOWN: usize = 4000;

/// One revision, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionRow {
    pub n: u32,
    /// Unix time, milliseconds (0: unknown).
    pub at_ms: u64,
    pub by: String,
    pub what: String,
    /// It carries an Agent page.
    pub page: bool,
}

/// One revision, read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionView {
    pub row: RevisionRow,
    pub files: BTreeMap<String, String>,
    pub page: Option<String>,
}

/// What the modal asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAct {
    Close,
    Restore(u32),
}

/// `at_ms` as `2026-11-03 04:12 UTC` (`?` for 0).
#[must_use]
pub fn when(at_ms: u64) -> String {
    if at_ms == 0 {
        return "?".into();
    }
    let secs = at_ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02} UTC")
}

/// One line of a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    Same(&'a str),
    /// In the revision, not now.
    Gone(&'a str),
    /// Now, not in the revision.
    New(&'a str),
}

/// A line diff of `old` (the revision) against `new` (now): the longest
/// common subsequence, so a moved block reads as gone and new. Files past
/// [`MAX_DIFF_LINES`] a side are given as all gone, all new.
#[must_use]
pub fn diff<'a>(old: &'a str, new: &'a str) -> Vec<Line<'a>> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        return a
            .iter()
            .map(|l| Line::Gone(l))
            .chain(b.iter().map(|l| Line::New(l)))
            .collect();
    }
    // lcs[i][j]: the common length of a[i..] and b[j..].
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Line::Same(a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(Line::Gone(a[i]));
            i += 1;
        } else {
            out.push(Line::New(b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| Line::Gone(l)));
    out.extend(b[j..].iter().map(|l| Line::New(l)));
    out
}

/// How many lines a diff adds and removes.
#[must_use]
pub fn counts(lines: &[Line<'_>]) -> (usize, usize) {
    let new = lines.iter().filter(|l| matches!(l, Line::New(_))).count();
    let gone = lines.iter().filter(|l| matches!(l, Line::Gone(_))).count();
    (new, gone)
}

/// The History row and, unfolded, the revisions newest first. Returns
/// the revision clicked.
pub fn rows_ui(ui: &mut egui::Ui, chrome: &mut Chrome, rows: &[RevisionRow], open: &mut bool) -> Option<u32> {
    if rows.is_empty() {
        return None;
    }
    let t = theme(ui);
    let arrow = if *open { "▾" } else { "▸" };
    let r = ui.selectable_label(
        false,
        RichText::new(format!("{arrow} history ({})", rows.len()))
            .monospace()
            .color(t.muted),
    );
    chrome.hit("history", r.rect);
    if r.on_hover_text("every save, as it was: who, when, what")
        .clicked()
    {
        *open = !*open;
    }
    if !*open {
        return None;
    }
    let mut picked = None;
    for row in rows.iter().rev() {
        let text = format!("{}  {}  {}", row.n, when(row.at_ms), row.what);
        let r = ui.add(
            egui::Label::new(RichText::new(text).monospace().color(t.fg))
                .truncate()
                .sense(egui::Sense::click()),
        );
        chrome.hit(format!("revision {}", row.n), r.rect);
        let hover = format!(
            "{} · {}{}",
            row.by,
            when(row.at_ms),
            if row.page { " · from an Agent page" } else { "" }
        );
        if r.on_hover_text(hover).clicked() {
            picked = Some(row.n);
        }
    }
    picked
}

/// The revision in the middle of the screen: who, when, what, its Agent
/// page, each file's diff against `current` (the project as it is now),
/// Restore (when `restore` allows) and Close.
pub fn modal(
    ctx: &egui::Context,
    chrome: &mut Chrome,
    view: &RevisionView,
    current: &BTreeMap<String, String>,
    restore: &Result<(), String>,
) -> Option<HistoryAct> {
    let mut out = None;
    let m = egui::Modal::new(egui::Id::unique("history-revision")).show(ctx, |ui| {
        let t = theme(ui);
        let screen = ctx.content_rect();
        ui.set_max_width((screen.width() - 80.0).clamp(320.0, 900.0));
        ui.set_max_height((screen.height() - 80.0).max(240.0));
        ui.label(
            RichText::new(format!("Revision {}: {}", view.row.n, view.row.what))
                .strong()
                .size(16.0),
        );
        ui.label(
            RichText::new(format!("{} · {}", view.row.by, when(view.row.at_ms)))
                .small()
                .color(t.muted),
        );
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .id_salt("history-body")
            .max_height((screen.height() - 220.0).max(120.0))
            .show(ui, |ui| {
                if let Some(page) = &view.page {
                    ui.label(RichText::new("The Agent page").strong());
                    let shown: String = page.chars().take(MAX_PAGE_SHOWN).collect();
                    let cut = shown.len() < page.len();
                    ui.label(RichText::new(shown).monospace().small());
                    if cut {
                        ui.label(RichText::new("(cut)").small().color(t.muted));
                    }
                    ui.separator();
                }
                let names: Vec<&String> = view.files.keys().chain(current.keys()).collect();
                let mut seen = Vec::new();
                for name in names {
                    if seen.contains(&name) {
                        continue;
                    }
                    seen.push(name);
                    let then = view.files.get(name).map_or("", String::as_str);
                    let now = current.get(name).map_or("", String::as_str);
                    let lines = diff(then, now);
                    let (new, gone) = counts(&lines);
                    let state = match (view.files.contains_key(name), current.contains_key(name)) {
                        (true, false) => " (gone since)".to_string(),
                        (false, true) => " (new since)".to_string(),
                        _ if new + gone == 0 => " (unchanged)".to_string(),
                        _ => format!(" (+{new} −{gone} since)"),
                    };
                    ui.label(RichText::new(format!("{name}{state}")).strong());
                    if new + gone == 0 {
                        continue;
                    }
                    // One block per file, coloured line by line.
                    for l in &lines {
                        let (mark, s, color) = match l {
                            Line::Same(s) => ("  ", *s, t.muted),
                            Line::Gone(s) => ("- ", *s, RED),
                            Line::New(s) => ("+ ", *s, GREEN),
                        };
                        ui.add(
                            egui::Label::new(RichText::new(format!("{mark}{s}")).monospace().color(color))
                                .truncate(),
                        );
                    }
                    ui.add_space(6.0);
                }
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let tall = Vec2::new(0.0, TAP * 0.7);
            let b = egui::Button::new(RichText::new("Restore this revision").strong()).min_size(tall);
            let r = match restore {
                Ok(()) => ui
                    .add(b)
                    .on_hover_text("the project's files back to this revision, as a new revision"),
                Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
            };
            chrome.hit("revision restore", r.rect);
            if r.clicked() {
                out = Some(HistoryAct::Restore(view.row.n));
            }
            let c = ui.add(egui::Button::new("Close").min_size(tall));
            chrome.hit("revision close", c.rect);
            if c.clicked() {
                out = Some(HistoryAct::Close);
            }
        });
    });
    chrome.hit("revision form", m.response.rect);
    if out.is_none() && m.should_close() {
        out = Some(HistoryAct::Close);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diff_is_a_longest_common_subsequence_and_counts() {
        let d = diff("a\nb\nc\n", "a\nx\nc\nd\n");
        assert_eq!(
            d,
            [
                Line::Same("a"),
                Line::Gone("b"),
                Line::New("x"),
                Line::Same("c"),
                Line::New("d")
            ]
        );
        assert_eq!(counts(&d), (2, 1));
        assert_eq!(diff("", ""), []);
        assert_eq!(diff("a", ""), [Line::Gone("a")]);
        assert_eq!(diff("", "a"), [Line::New("a")]);
        assert_eq!(counts(&diff("same\nsame", "same\nsame")), (0, 0));
        // Past the cap: all gone, all new, no quadratic table.
        let big = "x\n".repeat(MAX_DIFF_LINES + 1);
        let d = diff(&big, "y");
        assert_eq!(counts(&d), (1, MAX_DIFF_LINES + 1));
    }

    #[test]
    fn when_reads_as_a_utc_date() {
        assert_eq!(when(0), "?");
        assert_eq!(when(1_000), "1970-01-01 00:00 UTC");
        assert_eq!(when(1_793_679_000_000), "2026-11-03 04:10 UTC");
        assert_eq!(when(951_782_400_000), "2000-02-29 00:00 UTC");
    }
}
