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
        // "Agent: " is how revisions before 2026-11-23 were marked.
        let text = text
            .strip_prefix("Described: ")
            .or_else(|| text.strip_prefix("Agent: "))
            .unwrap_or(text);
        let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut c = line.chars();
        c.next()
            .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
    }
}

/// Rows of a version's description in the History tab (two until
/// 2026-11-26: Nick, "way too short").
pub const DESCRIPTION_ROWS: usize = 3;

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
            .tip("Every save and every described change is kept here, newest at the top.");
        return None;
    }
    let newest: Vec<&RevisionRow> = rows.iter().rev().collect();
    // The row on show: the newest when none is picked. A version on show
    // that is not among `rows` (a searched list) marks none of them, and
    // the steps are off.
    let shown = at.map_or(Some(0), |n| newest.iter().position(|r| r.n == n));
    let here = shown.unwrap_or(usize::MAX);
    let mut pick = None;
    ui.horizontal(|ui| {
        let r = ui
            .add_enabled(
                shown.is_some_and(|k| k > 0),
                egui::Button::new(RichText::new("▲ Newer").size(18.0)),
            )
            .tip("Runs the next newer version (Up key).");
        chrome.hit("history newer", r.rect);
        if r.clicked() {
            pick = Some(Pick::Show(newest[here - 1].n));
        }
        let r = ui
            .add_enabled(
                shown.is_some_and(|k| k + 1 < newest.len()),
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
        if up && shown.is_some_and(|k| k > 0) {
            pick = Some(Pick::Show(newest[here - 1].n));
        } else if down && shown.is_some_and(|k| k + 1 < newest.len()) {
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
            if row.page { " · described" } else { "" }
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

/// A Describe revision's page, read back into its parts
/// ([`crate::instruct::record`] writes it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub instruction: String,
    pub file: String,
    pub why: String,
    pub result: String,
    /// "Earlier changes sent with it: N".
    pub earlier: Option<usize>,
    pub scenarios: Vec<String>,
    /// Each answer that did not pass: its problems.
    pub attempts: Vec<Vec<String>>,
    pub answer: String,
}

impl Page {
    /// The page's parts; `None` when it has neither an instruction nor a
    /// result (another kind of page: shown as it is, folded).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        #[derive(PartialEq)]
        enum In {
            Head,
            Scenarios,
            Attempts,
            Answer,
        }
        let mut p = Self::default();
        let mut at = In::Head;
        for line in text.lines() {
            let line = line.trim_end();
            if let Some(v) = line.strip_prefix("Instruction: ") {
                p.instruction = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("File: ") {
                p.file = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Why: ") {
                p.why = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Result: ") {
                p.result = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("Earlier changes sent with it: ") {
                p.earlier = v.trim().parse().ok();
            } else if line.starts_with("Scenarios:") {
                at = In::Scenarios;
            } else if line.starts_with("Answers that did not pass") {
                at = In::Attempts;
            } else if line.starts_with("The answer, without its code:") {
                at = In::Answer;
            } else if at == In::Answer {
                if !(line.is_empty() && p.answer.is_empty()) {
                    p.answer.push_str(line);
                    p.answer.push('\n');
                }
            } else if at == In::Attempts && line.starts_with("Attempt ") {
                p.attempts.push(Vec::new());
            } else if let Some(v) = line.strip_prefix("- ") {
                match (&at, p.attempts.last_mut()) {
                    (In::Scenarios, _) => p
                        .scenarios
                        .push(v.trim().trim_end_matches(": passed").to_string()),
                    (In::Attempts, Some(a)) => a.push(v.trim().to_string()),
                    _ => {}
                }
            }
        }
        p.answer = p.answer.trim().to_string();
        (!p.instruction.is_empty() || !p.result.is_empty()).then_some(p)
    }

    /// What came of it, in a line: whether every check passed (`None`
    /// when the page does not say).
    #[must_use]
    pub fn passed(&self) -> Option<bool> {
        if self.result.is_empty() {
            None
        } else if self.result.contains("did not pass") || self.result.starts_with("Not changed") {
            Some(false)
        } else if self.result.starts_with("Updated") || !self.scenarios.is_empty() {
            Some(true)
        } else {
            None
        }
    }
}

/// `at_ms` as "5 minutes ago" against `now_ms`; a date past a week, `?`
/// for 0.
#[must_use]
pub fn ago(at_ms: u64, now_ms: u64) -> String {
    if at_ms == 0 {
        return "?".into();
    }
    let s = now_ms.saturating_sub(at_ms) / 1000;
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match s {
        0..=59 => "just now".into(),
        60..=3599 => plural(s / 60, "minute"),
        3600..=86_399 => plural(s / 3600, "hour"),
        86_400..=604_799 => plural(s / 86_400, "day"),
        _ => when(at_ms).split(' ').next().unwrap_or("?").to_string(),
    }
}

/// Body text of the window: 18 pt (the legibility floor is 16).
const BODY: f32 = 18.0;
const META: f32 = 16.0;
/// A diff longer than this many changed lines starts folded.
const OPEN_DIFF_LINES: usize = 30;
/// Unchanged lines kept either side of a change.
const CONTEXT: usize = 2;

fn body(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(BODY)
}

/// A section that starts folded: `title` (and a muted `count`), its body.
fn fold(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    key: &str,
    n: u32,
    title: &str,
    tip: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    let r = egui::CollapsingHeader::new(RichText::new(title).size(BODY).strong())
        .id_salt(("history-fold", n, title))
        .default_open(false)
        .show(ui, add);
    chrome.hit(key, r.header_response.rect);
    r.header_response.tip(tip);
}

fn unchanged(n: usize) -> String {
    format!("⋯ {n} unchanged line{}", if n == 1 { "" } else { "s" })
}

/// One file's diff: the changed lines with a little context, the rest
/// folded into "N unchanged lines".
fn diff_rows(ui: &mut egui::Ui, d: &Diffed) {
    let t = theme(ui);
    let keep: Vec<bool> = (0..d.lines.len())
        .map(|i| {
            let lo = i.saturating_sub(CONTEXT);
            let hi = (i + CONTEXT + 1).min(d.lines.len());
            d.lines[lo..hi].iter().any(|(k, _)| *k != Kind::Same)
        })
        .collect();
    let mut skipped = 0;
    for (i, (kind, s)) in d.lines.iter().enumerate() {
        if keep[i] {
            if skipped > 0 {
                ui.label(RichText::new(unchanged(skipped)).size(META).color(t.muted));
                skipped = 0;
            }
            crate::diff_ui::row(ui, *kind, "", s);
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        ui.label(RichText::new(unchanged(skipped)).size(META).color(t.muted));
    }
}

/// "+3 −1" in green and red, after `name`, as one header text.
fn file_head(ui: &egui::Ui, name: &str, new: usize, gone: usize, note: &str) -> egui::text::LayoutJob {
    let t = theme(ui);
    let font = egui::FontId::proportional(BODY);
    let mut job = egui::text::LayoutJob::default();
    let mut add = |text: String, color: egui::Color32| {
        job.append(
            &text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    };
    add(name.to_string(), t.fg);
    if !note.is_empty() {
        add(format!("   {note}"), t.muted);
    }
    if new > 0 {
        add(format!("   +{new}"), crate::st_view::brand::GREEN);
    }
    if gone > 0 {
        add(format!("   −{gone}"), crate::st_view::brand::RED);
    }
    job
}

/// The revision in the middle of the screen, for a controls engineer: what
/// was asked and what came of it, who and when, then what changed (each
/// file's counts and diff against `current`, the project as it is now),
/// and the reasoning, the requirements and the failed attempts, each
/// folded until opened; Restore (when `restore` allows) and Close.
pub fn modal(
    ctx: &egui::Context,
    chrome: &mut Chrome,
    view: &RevisionView,
    current: &BTreeMap<String, String>,
    restore: &Result<(), String>,
) -> Option<HistoryAct> {
    modal_against(ctx, chrome, view, Against::Now(current), restore)
}

/// What a version's files are compared with.
#[derive(Clone, Copy)]
pub enum Against<'a> {
    /// The project as it is now: green is in it now, red was only in the
    /// version.
    Now(&'a BTreeMap<String, String>),
    /// The version before it: green was added by this version, red taken
    /// out (an empty map for the first version).
    Before(&'a BTreeMap<String, String>),
}

/// [`modal`] with the comparison given.
pub fn modal_against(
    ctx: &egui::Context,
    chrome: &mut Chrome,
    view: &RevisionView,
    against: Against<'_>,
    restore: &Result<(), String>,
) -> Option<HistoryAct> {
    let (current, before) = match against {
        Against::Now(c) => (c, false),
        Against::Before(c) => (c, true),
    };
    let mut out = None;
    let m = egui::Modal::new(egui::Id::unique("history-revision")).show(ctx, |ui| {
        let t = theme(ui);
        let screen = ctx.content_rect();
        ui.set_max_width((screen.width() - 80.0).clamp(280.0, 900.0));
        ui.set_max_height((screen.height() - 80.0).max(240.0));
        let n = view.row.n;
        let page = view.page.as_deref().map(|p| (Page::parse(p), p));
        let parsed = page.as_ref().and_then(|(p, _)| p.as_ref());
        // The title row: the version, small, then who and when.
        let now = crate::sim_projects::wall_ms();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(RichText::new(format!("Version {n}")).size(META).color(t.muted));
            ui.label(
                RichText::new(format!(
                    "· {} · {} · {}",
                    ago(view.row.at_ms, now),
                    when(view.row.at_ms),
                    view.row.by
                ))
                .size(META)
                .color(t.muted),
            )
            .tip("When this version was saved, and by whom.");
        });
        ui.add_space(4.0);
        // What was asked: the headline.
        let asked = parsed
            .map(|p| p.instruction.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(view.row.what.as_str());
        let r = ui.add(egui::Label::new(RichText::new(asked).size(22.0).strong().color(t.accent)));
        chrome.hit("revision asked", r.rect);
        // What came of it, in a line.
        match parsed {
            Some(p) if !p.result.is_empty() => {
                let (dot, color) = match p.passed() {
                    Some(true) => ("●", crate::st_view::brand::GREEN),
                    Some(false) => ("●", crate::st_view::brand::RED),
                    None => ("○", t.muted),
                };
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(dot).size(BODY).color(color));
                    ui.add(egui::Label::new(body(p.result.as_str())).wrap());
                });
            }
            None if view.page.is_none() => {
                ui.label(body("Saved without an instruction.").color(t.muted));
            }
            _ => {}
        }
        if let Some(k) = parsed.and_then(|p| p.earlier).filter(|k| *k > 0) {
            ui.label(
                RichText::new(format!("Sent along with {k} earlier change{}.", if k == 1 { "" } else { "s" }))
                    .size(META)
                    .color(t.muted),
            );
        }
        ui.add_space(6.0);
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("history-body")
            .max_height((screen.height() - 260.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                // What changed: the files, with their counts and diffs.
                ui.label(RichText::new("What changed").size(BODY + 2.0).strong().color(t.accent));
                if let Some(p) = parsed.filter(|p| !p.file.is_empty()) {
                    ui.label(body(format!("Written to {}.", p.file)));
                }
                ui.label(
                    RichText::new(if before {
                        "Against the version before it: green lines were added, red lines taken out."
                    } else {
                        "Against the project as it is now: green lines are in the project now, red lines were only in this version."
                    })
                        .size(META)
                        .color(t.muted),
                );
                let names: Vec<&String> = view.files.keys().chain(current.keys()).collect();
                let mut seen = Vec::new();
                for name in names {
                    if seen.contains(&name) {
                        continue;
                    }
                    seen.push(name);
                    let then = view.files.get(name).map_or("", String::as_str);
                    let now = current.get(name).map_or("", String::as_str);
                    let diffed = if before {
                        cached(ctx, n, name, now, then)
                    } else {
                        cached(ctx, n, name, then, now)
                    };
                    let (new, gone) = (diffed.new, diffed.gone);
                    let note = match (view.files.contains_key(name), current.contains_key(name), before) {
                        (true, false, false) => "no longer in the project",
                        (false, true, false) => "added since",
                        (true, false, true) => "new in this version",
                        (false, true, true) => "removed in this version",
                        _ if new + gone == 0 => "unchanged",
                        _ => "",
                    };
                    let head = file_head(ui, name, new, gone, note);
                    if new + gone == 0 {
                        ui.label(head);
                        continue;
                    }
                    let r = egui::CollapsingHeader::new(head)
                        .id_salt(("history-diff", n, name.as_str()))
                        .default_open(new + gone <= OPEN_DIFF_LINES)
                        .show(ui, |ui| diff_rows(ui, &diffed));
                    chrome.hit(format!("revision file {name}"), r.header_response.rect);
                    r.header_response.tip("The changed lines of this file. Click to fold or open.");
                    ui.add_space(2.0);
                }
                ui.add_space(8.0);
                // The reasoning, the requirements, the failed attempts.
                if let Some(p) = parsed {
                    if !p.why.is_empty() {
                        fold(ui, chrome, "revision why", n, "Why", "The reason given for this change.", |ui| {
                            ui.add(egui::Label::new(body(p.why.as_str())).wrap());
                        });
                    }
                    if !p.scenarios.is_empty() {
                        let title = format!("Requirements it passed ({})", p.scenarios.len());
                        fold(
                            ui,
                            chrome,
                            "revision scenarios",
                            n,
                            &title,
                            "The requirement scenarios this change was checked against.",
                            |ui| {
                                for s in &p.scenarios {
                                    ui.horizontal_top(|ui| {
                                        ui.label(RichText::new("✓").size(BODY).color(crate::st_view::brand::GREEN));
                                        ui.add(egui::Label::new(body(s.as_str())).wrap());
                                    });
                                }
                            },
                        );
                    }
                    if !p.attempts.is_empty() {
                        let title = format!("Attempts that did not pass ({})", p.attempts.len());
                        fold(
                            ui,
                            chrome,
                            "revision attempts",
                            n,
                            &title,
                            "Answers that failed a check and were sent back before this one.",
                            |ui| {
                                for (k, a) in p.attempts.iter().enumerate() {
                                    ui.label(body(format!("Attempt {}", k + 1)).strong());
                                    for problem in a {
                                        ui.horizontal_top(|ui| {
                                            ui.label(
                                                RichText::new("✗").size(BODY).color(crate::st_view::brand::RED),
                                            );
                                            ui.add(egui::Label::new(body(problem.as_str())).wrap());
                                        });
                                    }
                                }
                            },
                        );
                    }
                    if !p.answer.is_empty() {
                        fold(
                            ui,
                            chrome,
                            "revision answer",
                            n,
                            "The explanation it gave",
                            "What was said about the change, without the code.",
                            |ui| {
                                ui.add(egui::Label::new(body(p.answer.as_str())).wrap());
                            },
                        );
                    }
                } else if let Some((_, raw)) = &page {
                    // A page of another kind: folded, wrapped, cut.
                    fold(ui, chrome, "revision record", n, "Record", "The text kept with this version.", |ui| {
                        let shown: String = raw.chars().take(MAX_PAGE_SHOWN).collect();
                        ui.add(egui::Label::new(body(shown)).wrap());
                        if shown_cut(raw) {
                            ui.label(RichText::new("(cut)").size(META).color(t.muted));
                        }
                    });
                }
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let tall = Vec2::new(0.0, TAP * 0.7);
            let b = egui::Button::new(RichText::new("Restore this version").size(BODY).strong()).min_size(tall);
            let r = match restore {
                Ok(()) => ui
                    .add(b)
                    .tip("Puts the project's files back as they were here, saved as a new version."),
                Err(why) => ui.add_enabled(false, b).tip(why),
            };
            chrome.hit("revision restore", r.rect);
            if r.clicked() {
                out = Some(HistoryAct::Restore(n));
            }
            let c = ui
                .add(egui::Button::new(RichText::new("Close").size(BODY)).min_size(tall))
                .tip("Closes this window. Nothing changes (Escape).");
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

fn shown_cut(page: &str) -> bool {
    page.chars().count() > MAX_PAGE_SHOWN
}

/// The Code tab's mark while it shows an old version: "Version N, when",
/// read-only, with a way back to the live file. Returns true when the way
/// back is pressed.
pub fn stepped_mark(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    n: u32,
    row: Option<&RevisionRow>,
    live: &str,
) -> bool {
    let t = theme(ui);
    let mut back = false;
    let text = row.map_or_else(
        || format!("Version {n}"),
        |r| {
            format!(
                "Version {n}, {} · {}",
                ago(r.at_ms, crate::sim_projects::wall_ms()),
                r.by
            )
        },
    );
    let r = egui::Frame::new()
        .fill(t.accent.gamma_multiply(0.15))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(text).size(BODY).strong().color(t.accent));
                ui.label(RichText::new("read-only").size(META).color(t.muted));
                let c = ui
                    .add(egui::Button::new(
                        RichText::new(format!("Back to {live}")).size(META),
                    ))
                    .tip("Leaves this old version and shows the file as it is now.");
                chrome.hit("code live", c.rect);
                back = c.clicked();
            });
        });
    chrome.hit("code version mark", r.response.rect);
    back
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

    #[test]
    fn a_describe_page_reads_back_into_its_parts() {
        let text = "Instruction: Add a stop\nFile: main.st\nWhy: Needs one\nResult: Updated main.st: 2 scenarios passed, kept as tests.\n\nEarlier changes sent with it: 2\n\nScenarios:\n- stop works (a.st): passed\n\nAnswers that did not pass, sent back: 1\nAttempt 1:\n- line 3 is wrong\n\nThe answer, without its code:\nDone.\n";
        let p = Page::parse(text).unwrap();
        assert_eq!(p.instruction, "Add a stop");
        assert_eq!(p.file, "main.st");
        assert_eq!(p.earlier, Some(2));
        assert_eq!(p.scenarios, ["stop works (a.st)"]);
        assert_eq!(p.attempts, [vec!["line 3 is wrong".to_string()]]);
        assert_eq!(p.answer, "Done.");
        assert_eq!(p.passed(), Some(true));
        assert!(Page::parse("just some text").is_none());
        assert_eq!(
            Page::parse("Instruction: x\nResult: Updated a; 1 check did not pass.")
                .unwrap()
                .passed(),
            Some(false)
        );
    }

    #[test]
    fn ago_is_relative_then_a_date() {
        let now = 1_793_679_000_000;
        assert_eq!(ago(0, now), "?");
        assert_eq!(ago(now - 5_000, now), "just now");
        assert_eq!(ago(now - 60_000, now), "1 minute ago");
        assert_eq!(ago(now - 3 * 3_600_000, now), "3 hours ago");
        assert_eq!(ago(now - 2 * 86_400_000, now), "2 days ago");
        assert_eq!(ago(now - 30 * 86_400_000, now), "2026-10-04");
    }
}
