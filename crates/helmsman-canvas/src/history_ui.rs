//! A project's history (Nick 2026-11-01, `project-history`; interview
//! 2026-11-02: revisions on the box, not git), in its own tab of the left
//! pane since 2026-11-13 ([`tab_ui`]): the revisions newest first, each
//! one line saying what changed, stepped through with Newer / Older; in
//! the simulation a pick runs that version. A double click opens the
//! revision in the middle of the screen with its Agent page and a line diff of each file against
//! the project as it is, and Restore for an engineer. The same rows and
//! modal serve the box (`crate::box_projects`, from `/api/projects/{p}/
//! history`) and the simulation (`crate::sim_projects`, revisions in the
//! browser's storage).

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use eframe::egui::{self, RichText, Vec2};

use crate::diff_ui::Kind;
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

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
    /// The change in a few words, when there is more than `what` to go
    /// on (the Agent's reason).
    pub about: Option<String>,
}

impl RevisionRow {
    /// The row's one line in the History tab: the change in a few words.
    #[must_use]
    pub fn short(&self) -> String {
        let text = self.about.as_deref().unwrap_or(&self.what);
        let text = text.strip_prefix("Agent: ").unwrap_or(text);
        let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut c = line.chars();
        c.next()
            .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
    }
}

/// Rows of a version's description in the History tab.
pub const DESCRIPTION_ROWS: usize = 2;

/// What the History tab asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Run this revision (a click, or a step up or down).
    Show(u32),
    /// Open this revision's details: its Agent page and diffs.
    Details(u32),
}

/// The History tab (Nick, 2026-11-13): the revisions newest first, each
/// one line saying what changed, under Newer / Older that step through
/// them (the Up and Down keys too). `at`: the revision shown, `None` for
/// the newest. Returns what was asked.
pub fn tab_ui(ui: &mut egui::Ui, chrome: &mut Chrome, rows: &[RevisionRow], at: Option<u32>) -> Option<Pick> {
    let t = theme(ui);
    if rows.is_empty() {
        ui.label(RichText::new("No versions yet.").color(t.muted))
            .tip("Every save and every change the Agent makes is kept here, newest at the top.");
        return None;
    }
    let newest: Vec<&RevisionRow> = rows.iter().rev().collect();
    let here = at.and_then(|n| newest.iter().position(|r| r.n == n)).unwrap_or(0);
    let mut pick = None;
    ui.horizontal(|ui| {
        let r = ui
            .add_enabled(here > 0, egui::Button::new(RichText::new("▲ Newer").size(18.0)))
            .tip("Runs the next newer version (Up key).");
        chrome.hit("history newer", r.rect);
        if r.clicked() {
            pick = Some(Pick::Show(newest[here - 1].n));
        }
        let r = ui
            .add_enabled(
                here + 1 < newest.len(),
                egui::Button::new(RichText::new("▼ Older").size(18.0)),
            )
            .tip("Runs the next older version (Down key).");
        chrome.hit("history older", r.rect);
        if r.clicked() {
            pick = Some(Pick::Show(newest[here + 1].n));
        }
    });
    // Up and Down step too, while no text field has the keyboard.
    if pick.is_none() && ui.memory(|m| m.focused().is_none()) {
        let (up, down) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if up && here > 0 {
            pick = Some(Pick::Show(newest[here - 1].n));
        } else if down && here + 1 < newest.len() {
            pick = Some(Pick::Show(newest[here + 1].n));
        }
    }
    ui.add_space(4.0);
    for (k, row) in newest.iter().enumerate() {
        let on = k == here;
        // Two lines of the description (Nick, 2026-11-13), the second
        // ending in "…" when there is more: wrapped at the pane's width,
        // so a long one never widens it.
        let font = egui::TextStyle::Button.resolve(ui.style());
        let width = ui.available_width() - 2.0 * ui.spacing().button_padding.x - 12.0;
        let mut job = egui::text::LayoutJob::simple(
            row.short(),
            font,
            if on { t.accent } else { t.fg },
            width.max(40.0),
        );
        job.wrap.max_rows = DESCRIPTION_ROWS;
        job.wrap.overflow_character = Some('…');
        // Laid out here: a button given the job would wrap it again.
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let r = ui.selectable_label(on, galley); // tip: exempt, added below
        chrome.hit(format!("revision {}", row.n), r.rect);
        let r = r.tip(format!(
            "{}\n{} · {} · version {}{}. Click to run it; double-click for its details.",
            row.what,
            row.by,
            when(row.at_ms),
            row.n,
            if row.page { " · from the Agent" } else { "" }
        ));
        if r.double_clicked() {
            pick = Some(Pick::Details(row.n));
        } else if r.clicked() && !on {
            pick = Some(Pick::Show(row.n));
        }
        ui.label(
            RichText::new(format!("{} · {}", when(row.at_ms), row.by))
                .size(16.0)
                .color(t.muted),
        )
        .tip("When it was saved, and by whom.");
    }
    pick
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

/// A file's diff kept between frames: the LCS table is up to
/// [`MAX_DIFF_LINES`] squared, so the modal does not redo it every frame.
#[derive(Debug, Clone)]
struct Diffed {
    lines: Vec<(Kind, String)>,
    new: usize,
    gone: usize,
}

/// The diffs of the revision on show, by file and a hash of the two texts
/// (the project can change under an open modal), in the context's memory.
#[derive(Debug, Clone, Default)]
struct DiffCache {
    revision: u32,
    files: HashMap<(String, u64), Arc<Diffed>>,
}

#[cfg(test)]
thread_local! {
    /// How many diffs [`cached`] computed on this thread (a test reads it).
    static COMPUTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The diff of `then` (revision `revision`'s `name`) against `now`,
/// computed once for as long as neither text changes.
fn cached(ctx: &egui::Context, revision: u32, name: &str, then: &str, now: &str) -> Arc<Diffed> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (then, now).hash(&mut h);
    let key = (name.to_string(), h.finish());
    let id = egui::Id::NULL.with("history-diffs");
    let hit = ctx.data_mut(|d| {
        let c = d.get_temp_mut_or_default::<DiffCache>(id);
        if c.revision != revision {
            *c = DiffCache {
                revision,
                files: HashMap::new(),
            };
        }
        c.files.get(&key).cloned()
    });
    if let Some(hit) = hit {
        return hit;
    }
    #[cfg(test)]
    COMPUTED.with(|c| c.set(c.get() + 1));
    let lines = diff(then, now);
    let (new, gone) = counts(&lines);
    let made = Arc::new(Diffed {
        lines: lines
            .iter()
            .map(|l| match l {
                Line::Same(s) => (Kind::Same, (*s).to_string()),
                Line::Gone(s) => (Kind::Removed, (*s).to_string()),
                Line::New(s) => (Kind::Added, (*s).to_string()),
            })
            .collect(),
        new,
        gone,
    });
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<DiffCache>(id)
            .files
            .insert(key, made.clone());
    });
    made
}

/// How many lines a diff adds and removes.
#[must_use]
pub fn counts(lines: &[Line<'_>]) -> (usize, usize) {
    let new = lines.iter().filter(|l| matches!(l, Line::New(_))).count();
    let gone = lines.iter().filter(|l| matches!(l, Line::Gone(_))).count();
    (new, gone)
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
                    let diffed = cached(ctx, view.row.n, name, then, now);
                    let (new, gone) = (diffed.new, diffed.gone);
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
                    // One block per file, line by line as GitHub draws
                    // a diff (Nick, 2026-11-08).
                    for (kind, s) in &diffed.lines {
                        crate::diff_ui::row(ui, *kind, "", s);
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
                    .tip("Puts the project's files back as they were here, saved as a new revision."),
                Err(why) => ui.add_enabled(false, b).tip(why),
            };
            chrome.hit("revision restore", r.rect);
            if r.clicked() {
                out = Some(HistoryAct::Restore(view.row.n));
            }
            let c = ui
                .add(egui::Button::new("Close").min_size(tall))
                .tip("Closes the revision. Nothing changes (Escape).");
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

    /// The revision modal draws every frame; a file's diff is worked out
    /// once for as long as neither text changes, and again when one does.
    #[test]
    fn a_files_diff_is_computed_once_not_every_frame() {
        let ctx = egui::Context::default();
        let before = COMPUTED.with(std::cell::Cell::get);
        let computed = || COMPUTED.with(std::cell::Cell::get) - before;
        let first = cached(&ctx, 3, "main.st", "a\nb\n", "a\nx\n");
        assert_eq!((first.new, first.gone), (1, 1));
        for _ in 0..100 {
            let again = cached(&ctx, 3, "main.st", "a\nb\n", "a\nx\n");
            assert!(Arc::ptr_eq(&first, &again));
        }
        assert_eq!(computed(), 1);
        cached(&ctx, 3, "other.st", "a\nb\n", "a\nx\n");
        assert_eq!(computed(), 2, "each file has its own");
        let changed = cached(&ctx, 3, "main.st", "a\nb\n", "a\nx\ny\n");
        assert_eq!((changed.new, changed.gone), (2, 1));
        assert_eq!(computed(), 3, "the project changed under the modal");
        cached(&ctx, 4, "main.st", "a\nb\n", "a\nx\n");
        assert_eq!(computed(), 4, "another revision");
    }

    #[test]
    fn when_reads_as_a_utc_date() {
        assert_eq!(when(0), "?");
        assert_eq!(when(1_000), "1970-01-01 00:00 UTC");
        assert_eq!(when(1_793_679_000_000), "2026-11-03 04:10 UTC");
        assert_eq!(when(951_782_400_000), "2000-02-29 00:00 UTC");
    }
}
