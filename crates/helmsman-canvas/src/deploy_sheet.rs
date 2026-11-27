//! "What changes on the machine" (Nick, 2026-11-27, Show Changes): Deploy
//! on a box opens this sheet and never deploys straight away. It compares
//! the project with the program the box runs now, the way the box will
//! see it: each file's diff, the diagram diff with its counts, every
//! output whose logic changes, the checks, the replay where the policy
//! wants one (run before the sheet shows), and the tier with who may
//! apply it. Deploy on the sheet runs the usual steps; Cancel sends
//! nothing to the machine.
//!
//! Free of I/O like [`crate::box_projects`]: what to ask the box for is
//! queued there; the answers come in through [`Sheet`]'s `parse` and
//! `read` methods, and [`ui`] draws it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use eframe::egui::{self, RichText};
use helmsman_diagram::diff::{self, Line};
use helmsman_iec::ImageSizes;
use serde_json::Value as Json;

use crate::change;
use crate::diff_ui::Kind;
use crate::editor::Chrome;
use crate::tips::Tip;

/// What the box runs now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Running {
    /// Being read.
    #[default]
    Reading,
    /// The box runs no change set yet.
    NothingRuns,
    /// The change set's id and each module's source.
    Is {
        id: String,
        modules: BTreeMap<String, String>,
    },
    /// It could not be read, and why.
    Unread(String),
}

/// What `POST .../check` said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    /// The id the project would run as.
    pub snapshot: String,
    /// The tier against what runs, with why, when the box has one.
    pub tier: Option<String>,
    pub reasons: Vec<String>,
    /// The role the policy asks of the one who applies it.
    pub role: Option<String>,
    pub needs_replay: bool,
    /// (module, file) the project builds.
    pub modules: Vec<(String, String)>,
    /// What never stops a deploy, one line each.
    pub warnings: Vec<String>,
}

impl Check {
    /// From the check's JSON.
    #[must_use]
    pub fn parse(j: &Json) -> Self {
        let text = |v: &Json| v.as_str().map(str::to_string);
        Self {
            snapshot: text(&j["snapshot"]).unwrap_or_default(),
            tier: text(&j["approval"]["tier"]),
            reasons: j["approval"]["reasons"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| text(&r["why"]))
                .collect(),
            role: text(&j["approval"]["needs"]["role"]),
            needs_replay: j["approval"]["needs"]["replay"].as_bool() == Some(true),
            modules: j["modules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| Some((text(&m["name"])?, text(&m["file"])?)))
                .collect(),
            warnings: j["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|w| {
                    let msg = text(&w["message"])?;
                    Some(match w["line"].as_u64() {
                        Some(l) => format!("{}:{l}: {msg}", w["file"].as_str().unwrap_or("?")),
                        None => msg,
                    })
                })
                .collect(),
        }
    }
}

/// How the replay stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Replay {
    /// The policy asks for none (or the check is not in yet).
    #[default]
    NotNeeded,
    /// Run before the sheet is complete.
    Running,
    /// Its result, in a sentence, and whether any signal differs.
    Done { sentence: String, differs: bool },
}

impl Replay {
    /// A replay's JSON as a sentence.
    #[must_use]
    pub fn parse(j: &Json) -> Self {
        let differing: Vec<&str> = j["signals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["identical"] == Json::Bool(false))
            .filter_map(|s| s["signal"].as_str())
            .collect();
        let faults = j["faults"].as_array().map_or(0, Vec::len);
        let secs = j["window_ms"].as_u64().unwrap_or(0) / 1000;
        let mut sentence = format!("Replayed {secs} s of recorded values: ");
        if differing.is_empty() {
            sentence.push_str("every output ran the same");
        } else {
            let n = differing.len();
            let _ = write!(
                sentence,
                "{n} output{} differ{}: {}",
                if n == 1 { "" } else { "s" },
                if n == 1 { "s" } else { "" },
                differing.join(", ")
            );
        }
        if faults > 0 {
            let _ = write!(
                sentence,
                "; {faults} fault{} in the new program",
                if faults == 1 { "" } else { "s" }
            );
        }
        sentence.push('.');
        Self::Done {
            sentence,
            differs: !differing.is_empty() || faults > 0,
        }
    }
}

/// One file of the project against the module the box runs from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    pub file: String,
    pub module: String,
    pub lines: Vec<Line>,
}

impl FileDiff {
    /// Lines added and removed.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let n = |f: fn(&Line) -> bool| self.lines.iter().filter(|l| f(l)).count();
        (
            n(|l| matches!(l, Line::Added { .. })),
            n(|l| matches!(l, Line::Removed { .. })),
        )
    }
}

/// What the project would change, worked out once the box has said what
/// runs and what the project builds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub files: Vec<FileDiff>,
    /// Blocks added, changed and removed.
    pub blocks: (usize, usize, usize),
    pub outputs: Vec<String>,
    /// Modules the box runs that the project no longer builds.
    pub removed: Vec<String>,
    /// Why the blocks and the outputs could not be compared, when they
    /// could not: the sheet says so and never reports "no output changes".
    pub unknown: Option<String>,
    /// The whole program the box runs and the project builds, when they
    /// could be compared: what the sheet's diagram draws.
    pub old_src: String,
    pub new_src: String,
}

impl Report {
    /// Whether any file differs.
    #[must_use]
    pub fn differs(&self) -> bool {
        !self.removed.is_empty()
            || self.files.iter().any(|f| f.counts() != (0, 0))
            || self.blocks != (0, 0, 0)
    }

    /// Compares the project with what the box runs. The line diffs are per
    /// file (the running module's source carries every file under a
    /// `(* file NAME *)` line); the blocks and the outputs are worked out on
    /// the WHOLE program, every file lowered together, on both sides, so a
    /// change made through a block in another file is seen. `files` is the
    /// project's text by file.
    #[must_use]
    pub fn compare(
        running: &BTreeMap<String, String>,
        check: &Check,
        files: &BTreeMap<String, String>,
        sizes: ImageSizes,
    ) -> Self {
        let mut r = Self::default();
        let built: Vec<(&String, &String)> = files
            .iter()
            .filter(|(n, _)| crate::sim_projects::is_st(n) && !helmsman_iec::spec::is_test(n))
            .collect();
        let module_of = |file: &str| {
            check
                .modules
                .iter()
                .find(|(_, f)| f == file)
                .map_or_else(String::new, |(m, _)| m.clone())
        };
        // What the running modules hold of each file.
        let mut olds: BTreeMap<String, String> = BTreeMap::new();
        for src in running.values() {
            for (file, body) in split_files(src) {
                olds.entry(file).or_insert(body);
            }
        }
        // A source with no file lines is the one file's own.
        if olds.is_empty()
            && let ([(file, _)], [src]) = (built.as_slice(), running.values().collect::<Vec<_>>().as_slice())
        {
            olds.insert((*file).clone(), (*src).clone());
        }
        for (file, new) in &built {
            let old = olds.get(*file).cloned().unwrap_or_default();
            r.files.push(FileDiff {
                file: (*file).clone(),
                module: module_of(file),
                lines: diff::lines(&old, new),
            });
        }
        for (file, old) in &olds {
            if !built.iter().any(|(f, _)| *f == file) {
                r.files.push(FileDiff {
                    file: format!("{file} (removed)"),
                    module: String::new(),
                    lines: diff::lines(old, ""),
                });
            }
        }
        for module in running.keys() {
            if !check.modules.iter().any(|(m, _)| m == module) {
                r.removed.push(module.clone());
            }
        }
        // The whole program, both sides.
        let mut names: Vec<&String> = check.modules.iter().map(|(m, _)| m).collect();
        names.dedup();
        if names.len() > 1 || running.len() > 1 {
            r.unknown = Some("the project or the box runs more than one program, which are compared one at a time by the box".into());
            return r;
        }
        let old_src = running.values().next().cloned().unwrap_or_default();
        let mut new_src = String::new();
        for (file, text) in &built {
            let _ = writeln!(new_src, "(* file {file} *)");
            new_src.push_str(text);
            if !text.ends_with('\n') {
                new_src.push('\n');
            }
        }
        let one = |t: &str| -> crate::scope::Files {
            std::iter::once(("program.st".to_string(), t.to_string())).collect()
        };
        let (s, _) = change::summarize(&one(&old_src), &one(&new_src), "program.st", sizes, &[]);
        if let Some(why) = s.unknown {
            r.unknown = Some(why);
        } else if s.old_broken {
            r.unknown = Some("the program the box runs does not compile here".into());
        } else {
            r.blocks = (s.blocks_added, s.blocks_changed, s.blocks_removed);
            r.outputs = s.outputs;
            r.old_src = old_src;
            r.new_src = new_src;
        }
        r
    }
}

/// A module's source split by its `(* file NAME *)` lines into each file's
/// text.
fn split_files(src: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in src.lines() {
        if let Some(name) = line.strip_prefix("(* file ").and_then(|r| r.strip_suffix(" *)")) {
            out.push((name.to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// The sheet's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sheet {
    pub project: String,
    pub running: Running,
    pub check: Option<Check>,
    pub replay: Replay,
    /// What the project changes, once everything it needs is in.
    pub report: Option<Report>,
}

impl Sheet {
    #[must_use]
    pub fn new(project: &str) -> Self {
        Self {
            project: project.to_string(),
            ..Self::default()
        }
    }

    /// `GET /api/running`: the change set the box runs.
    pub fn read_running(&mut self, status: u16, j: &Json) {
        self.running = match (status, j["running"].as_str()) {
            (200..=299, Some(id)) => Running::Is {
                id: id.to_string(),
                modules: BTreeMap::new(),
            },
            (200..=299, None) => Running::NothingRuns,
            _ => Running::Unread(format!("HTTP {status}")),
        };
    }

    /// `GET /api/snapshots/{id}`: the modules of what runs.
    pub fn read_snapshot(&mut self, status: u16, j: &Json) {
        let Running::Is { id, .. } = &self.running else {
            return;
        };
        let id = id.clone();
        self.running = if (200..=299).contains(&status) {
            Running::Is {
                id,
                modules: j["modules"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(n, m)| Some((n.clone(), m["source"].as_str()?.to_string())))
                    .collect(),
            }
        } else {
            Running::Unread(format!("HTTP {status}"))
        };
    }

    /// Whether the box runs exactly what the project would deploy.
    #[must_use]
    pub fn same(&self) -> bool {
        match (&self.running, &self.check) {
            (Running::Is { id, .. }, Some(c)) => !c.snapshot.is_empty() && *id == c.snapshot,
            _ => false,
        }
    }

    /// Works out the report when everything it needs is in.
    pub fn compute(&mut self, files: &BTreeMap<String, String>, sizes: ImageSizes) {
        if self.report.is_some() {
            return;
        }
        let Some(check) = &self.check else { return };
        let running = match &self.running {
            Running::Is { modules, .. } => modules.clone(),
            Running::NothingRuns => BTreeMap::new(),
            Running::Reading | Running::Unread(_) => return,
        };
        self.report = Some(Report::compare(&running, check, files, sizes));
    }

    /// Why Deploy is off on the sheet, if it is.
    #[must_use]
    pub fn blocked(&self, busy: bool, failed: Option<&str>) -> Option<String> {
        if let Some(why) = failed {
            return Some(why.to_string());
        }
        if busy || self.check.is_none() || self.running == Running::Reading || self.report.is_none() {
            return Some("Still checking.".into());
        }
        if let Running::Unread(why) = &self.running {
            return Some(format!(
                "The program the box runs could not be read ({why}), so what changes is unknown."
            ));
        }
        if self.replay == Replay::Running {
            return Some("The replay is still running.".into());
        }
        if self.same() {
            return Some("The box already runs this program: nothing to deploy.".into());
        }
        None
    }
}

/// The height of the diagram on the sheet, points.
const DIAGRAM_H: f32 = 260.0;

/// What the sheet asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Deploy,
    Cancel,
}

/// What the page tells the sheet each frame.
pub struct Context<'a> {
    /// A check, replay or save is still under way.
    pub busy: bool,
    /// What went wrong while the sheet was building, if anything.
    pub failed: Option<&'a str>,
    /// Why this page may not deploy, if it may not (signed out, a viewer).
    pub forbidden: Option<String>,
    /// The build's problems, when it did not build.
    pub problems: &'a [crate::projects::Diag],
    /// The diagram of the first changed module, marked against what runs
    /// (the Show Changes viewer): read-only, with no scan on it.
    pub diagram: Option<&'a mut crate::st_view::Viewer>,
}

/// The sheet, over the page.
pub fn ui(ctx: &egui::Context, chrome: &mut Chrome, sheet: &Sheet, c: &mut Context<'_>) -> Option<Act> {
    let mut act = None;
    let m = egui::Modal::new(egui::Id::unique("deploy-sheet")).show(ctx, |ui| {
        let t = crate::theme::theme(ui);
        let screen = ctx.content_rect();
        ui.set_max_width((screen.width() - 80.0).clamp(280.0, 820.0));
        ui.set_max_height((screen.height() - 80.0).max(240.0));
        let r = ui.add(egui::Label::new(
            RichText::new("What changes on the machine")
                .size(22.0)
                .strong()
                .color(t.accent),
        ));
        chrome.hit("deploy sheet", r.rect);
        let r = ui.add(
            egui::Label::new(
                RichText::new("Your edits were saved; nothing has been sent to the machine.").color(t.fg),
            )
            .wrap(),
        );
        chrome.hit("deploy sheet saved", r.rect);
        ui.label(
            RichText::new(format!(
                "Deploying {} to the machine, against the program it runs now.",
                sheet.project
            ))
            .color(t.muted),
        );
        ui.add_space(4.0);
        let blocked = c.forbidden.clone().or_else(|| sheet.blocked(c.busy, c.failed));
        egui::ScrollArea::vertical()
            .id_salt("deploy-sheet-body")
            .max_height((screen.height() - 260.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| body(ui, chrome, sheet, &mut *c, blocked.as_deref()));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let can = blocked.is_none();
            let r = ui
                .add_enabled(
                    can,
                    egui::Button::new(RichText::new("Deploy").size(18.0).strong()),
                )
                .tip(
                    blocked
                        .clone()
                        .unwrap_or_else(|| "Deploy: sends the project, as listed above.".into()),
                );
            chrome.hit("deploy sheet deploy", r.rect);
            if r.clicked() && can {
                act = Some(Act::Deploy);
            }
            let r = ui
                .button(RichText::new("Cancel").size(18.0))
                .tip("Cancel: sends nothing to the machine. Esc does the same.");
            chrome.hit("deploy sheet cancel", r.rect);
            if r.clicked() {
                act = Some(Act::Cancel);
            }
        });
    });
    if crate::theme::escaped(&m) {
        act = Some(Act::Cancel);
    }
    act
}

fn body(ui: &mut egui::Ui, chrome: &mut Chrome, sheet: &Sheet, c: &mut Context<'_>, blocked: Option<&str>) {
    use crate::st_view::brand::{GREEN, RED};
    let t = crate::theme::theme(ui);
    // A build that failed says why and stops here.
    if let Some(why) = c.failed {
        let r = ui.add(egui::Label::new(RichText::new(why).strong().color(t.ink(RED))).wrap());
        chrome.hit("deploy sheet failed", r.rect);
        for d in c.problems.iter().take(8) {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("{}:{}: {}", d.file, d.line, d.message))
                        .monospace()
                        .color(t.fg),
                )
                .wrap(),
            );
        }
        return;
    }
    if sheet.check.is_none() || sheet.running == Running::Reading || c.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Checking what changes…").color(t.muted));
        });
    }
    if sheet.same() {
        let r = ui.add(
            egui::Label::new(
                RichText::new("The box already runs this program.")
                    .strong()
                    .color(t.ink(GREEN)),
            )
            .wrap(),
        );
        chrome.hit("deploy sheet same", r.rect);
    }
    let Some(report) = &sheet.report else { return };
    // What changes, in numbers and names.
    let (a, ch, rm) = report.blocks;
    let lines: (usize, usize) = report
        .files
        .iter()
        .map(FileDiff::counts)
        .fold((0, 0), |s, x| (s.0 + x.0, s.1 + x.1));
    if let Some(why) = &report.unknown {
        // Never "no output changes" for what could not be compared.
        let r = ui
            .add(
                egui::Label::new(
                    RichText::new(format!(
                        "Could not compare the outputs: {why}. Read the files below."
                    ))
                    .strong()
                    .color(t.ink(crate::st_view::brand::WARN)),
                )
                .wrap(),
            )
            .tip("The blocks and the outputs that change could not be worked out.");
        chrome.hit("deploy sheet outputs", r.rect);
        ui.label(
            RichText::new(format!("Files: {} line(s) added, {} removed.", lines.0, lines.1)).color(t.fg),
        );
    } else {
        let r = ui
            .add(
                egui::Label::new(
                    RichText::new(format!(
                        "Diagram: {a} block(s) added, {ch} changed, {rm} removed. Files: {} line(s) added, {} removed.",
                        lines.0, lines.1
                    ))
                    .color(t.fg),
                )
                .wrap(),
            )
            .tip("How much differs from the program the box runs now.");
        chrome.hit("deploy sheet counts", r.rect);
        let policy_says = sheet
            .check
            .as_ref()
            .and_then(|c| c.tier.as_deref())
            .is_some_and(|t| t == "output-affecting");
        let outputs = if !report.outputs.is_empty() {
            format!("Switches {}.", report.outputs.join(", "))
        } else if policy_says {
            "No output's logic changes. The box's policy still treats a new module as output-affecting."
                .to_string()
        } else {
            "No output changes.".to_string()
        };
        let r = ui
            .add(egui::Label::new(RichText::new(outputs).strong().color(t.fg)).wrap())
            .tip("The outputs whose logic changes.");
        chrome.hit("deploy sheet outputs", r.rect);
    }
    if !sheet.same() && !report.differs() {
        ui.add(
            egui::Label::new(
                RichText::new("The program is the same; the I/O map, calendar or drivers differ.")
                    .color(t.fg),
            )
            .wrap(),
        );
    }
    if let Some(v) = c.diagram.as_deref_mut() {
        ui.add_space(4.0);
        let size = egui::vec2(ui.available_width(), DIAGRAM_H);
        let r = ui
            .allocate_ui(size, |ui| {
                ui.set_min_size(size);
                v.canvas(ui, None);
            })
            .response;
        chrome.hit("deploy sheet diagram", r.rect);
        ui.label(
            RichText::new("The diagram, marked against the program the box runs: green added, red removed.")
                .color(t.muted),
        );
    }
    for m in &report.removed {
        ui.add(egui::Label::new(RichText::new(format!("{m} is no longer deployed.")).color(t.fg)).wrap());
    }
    // The checks, the tier and who may apply it.
    if let Some(check) = &sheet.check {
        let r = ui
            .add(
                egui::Label::new(
                    RichText::new(if check.warnings.is_empty() {
                        "Checks: it builds.".to_string()
                    } else {
                        format!(
                            "Checks: it builds; {} warning(s) never stop a deploy.",
                            check.warnings.len()
                        )
                    })
                    .color(t.fg),
                )
                .wrap(),
            )
            .tip("The box built the project: this is what it would run.");
        chrome.hit("deploy sheet check", r.rect);
        for w in check.warnings.iter().take(5) {
            ui.add(egui::Label::new(RichText::new(w).monospace().color(t.muted)).wrap());
        }
        if let Some(tier) = &check.tier {
            let who = check
                .role
                .as_deref()
                .map_or_else(String::new, |r| format!(" Who may apply it: {r} or above."));
            let r = ui
                .add(egui::Label::new(RichText::new(format!("Tier: {tier}.{who}")).color(t.fg)).wrap())
                .tip("The box's approval policy decides the tier from what changes.");
            chrome.hit("deploy sheet tier", r.rect);
            for why in check.reasons.iter().take(4) {
                ui.add(egui::Label::new(RichText::new(why).color(t.muted)).wrap());
            }
        }
        match &sheet.replay {
            Replay::NotNeeded if check.needs_replay => {}
            Replay::NotNeeded => {}
            Replay::Running => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Replaying in the simulation…").color(t.muted));
                });
            }
            Replay::Done { sentence, differs } => {
                let color = if *differs { t.fg } else { t.ink(GREEN) };
                let r = ui.add(egui::Label::new(RichText::new(sentence).color(color)).wrap());
                chrome.hit("deploy sheet replay", r.rect);
            }
        }
    }
    if let Some(why) = blocked.filter(|_| !sheet.same()) {
        ui.add(egui::Label::new(RichText::new(why).strong().color(t.ink(RED))).wrap());
    }
    ui.add_space(6.0);
    // Each file's diff, changed lines with a little context.
    for f in &report.files {
        let (n, g) = f.counts();
        let head = format!("{} ({}): +{n} −{g}", f.file, f.module);
        if n + g == 0 {
            ui.label(RichText::new(format!("{} ({}): unchanged", f.file, f.module)).color(t.muted));
            continue;
        }
        let r = egui::CollapsingHeader::new(RichText::new(head).strong().color(t.fg))
            .icon(crate::theme::fold_icon)
            .id_salt(("deploy-sheet-file", f.file.as_str()))
            .default_open(n + g <= 60)
            .show(ui, |ui| rows(ui, &f.lines));
        chrome.hit(format!("deploy sheet diff {}", f.file), r.header_response.rect);
    }
}

/// The changed lines with two lines of context, a rule where lines are left out.
fn rows(ui: &mut egui::Ui, lines: &[Line]) {
    const CONTEXT: usize = 2;
    let t = crate::theme::theme(ui);
    let near: Vec<bool> = (0..lines.len())
        .map(|k| {
            let (lo, hi) = (k.saturating_sub(CONTEXT), (k + CONTEXT + 1).min(lines.len()));
            lines[lo..hi].iter().any(Line::is_change)
        })
        .collect();
    let mut skipped = false;
    for (k, l) in lines.iter().enumerate() {
        if !near[k] {
            if !skipped {
                ui.label(RichText::new("   ⋯").monospace().color(t.muted));
            }
            skipped = true;
            continue;
        }
        skipped = false;
        let (kind, n) = match l {
            Line::Same { new, .. } => (Kind::Same, *new),
            Line::Added { new, .. } => (Kind::Added, *new),
            Line::Removed { old, .. } => (Kind::Removed, *old),
        };
        crate::diff_ui::row(ui, kind, &format!("{n:>4}"), l.text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 64 };
    const OLD: &str = "PROGRAM p\nVAR_INPUT\n  a : BOOL;\nEND_VAR\nVAR_OUTPUT\n  x : BOOL;\n  y : BOOL;\nEND_VAR\nx := a;\ny := NOT a;\nEND_PROGRAM\n";

    fn check(snapshot: &str) -> Check {
        Check::parse(&json!({
            "snapshot": snapshot,
            "modules": [{ "name": "p", "file": "main.st" }],
            "approval": { "tier": "output-affecting", "reasons": [{ "tier": "output-affecting", "why": "x changes" }], "needs": { "role": "engineer", "replay": true } },
            "warnings": [{ "file": "main.st", "line": 3, "message": "unused" }],
        }))
    }

    #[test]
    fn a_check_gives_the_tier_who_may_apply_and_the_modules() {
        let c = check("abc");
        assert_eq!(c.tier.as_deref(), Some("output-affecting"));
        assert_eq!(c.role.as_deref(), Some("engineer"));
        assert!(c.needs_replay);
        assert_eq!(c.modules, vec![("p".to_string(), "main.st".to_string())]);
        assert_eq!(c.warnings, vec!["main.st:3: unused".to_string()]);
    }

    #[test]
    fn the_report_names_the_outputs_and_lines_that_change() {
        let new = OLD.replace("x := a;", "x := a OR NOT a;");
        let running: BTreeMap<String, String> = [("p".to_string(), OLD.to_string())].into();
        let files: BTreeMap<String, String> = [("main.st".to_string(), new)].into();
        let r = Report::compare(&running, &check("abc"), &files, SIZES);
        assert_eq!(r.outputs, vec!["X".to_string()], "{r:?}");
        assert!(r.differs());
        assert_eq!(r.files[0].counts(), (1, 1));
    }

    #[test]
    fn a_program_that_is_the_one_running_differs_in_nothing() {
        let running: BTreeMap<String, String> = [("p".to_string(), OLD.to_string())].into();
        let files: BTreeMap<String, String> = [("main.st".to_string(), OLD.to_string())].into();
        let r = Report::compare(&running, &check("abc"), &files, SIZES);
        assert!(!r.differs(), "{r:?}");
        assert_eq!(r.outputs.len(), 0);
    }

    #[test]
    fn a_module_the_box_runs_and_the_project_lacks_is_removed() {
        let running: BTreeMap<String, String> = [
            ("p".to_string(), OLD.to_string()),
            ("q".to_string(), OLD.to_string()),
        ]
        .into();
        let files: BTreeMap<String, String> = [("main.st".to_string(), OLD.to_string())].into();
        let r = Report::compare(&running, &check("abc"), &files, SIZES);
        assert_eq!(r.removed, vec!["q".to_string()]);
        assert!(r.differs());
    }

    #[test]
    fn same_ids_mean_the_box_already_runs_it_and_deploy_is_off() {
        let mut s = Sheet::new("proj");
        s.read_running(200, &json!({ "running": "abc" }));
        s.read_snapshot(200, &json!({ "modules": { "p": { "source": OLD } } }));
        s.check = Some(check("abc"));
        let files: BTreeMap<String, String> = [("main.st".to_string(), OLD.to_string())].into();
        s.compute(&files, SIZES);
        assert!(s.same());
        assert!(s.blocked(false, None).is_some_and(|w| w.contains("already runs")));
    }

    #[test]
    fn a_different_id_with_everything_in_lets_deploy_go() {
        let mut s = Sheet::new("proj");
        s.read_running(200, &json!({ "running": "old" }));
        s.read_snapshot(200, &json!({ "modules": { "p": { "source": OLD } } }));
        s.check = Some(check("new"));
        let files: BTreeMap<String, String> =
            [("main.st".to_string(), OLD.replace("y := NOT a;", "y := a;"))].into();
        s.compute(&files, SIZES);
        assert_eq!(s.blocked(false, None), None);
        assert!(s.blocked(true, None).is_some(), "busy holds it");
        assert!(s.blocked(false, Some("does not build")).is_some());
    }

    #[test]
    fn what_runs_that_cannot_be_read_holds_deploy() {
        let mut s = Sheet::new("proj");
        s.read_running(200, &json!({ "running": "old" }));
        s.read_snapshot(404, &json!({}));
        s.check = Some(check("new"));
        s.compute(&BTreeMap::new(), SIZES);
        assert!(s.blocked(false, None).is_some());
    }

    #[test]
    fn a_block_in_another_file_is_seen_on_the_whole_program() {
        let marked = |f: &str, t: &str| format!("(* file {f} *)\n{t}");
        let old = "PROGRAM p\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := start;\nEND_PROGRAM\n";
        let main = "PROGRAM p\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nVAR l : Latch; END_VAR\nl(set := start, reset := FALSE);\nlamp := l.q;\nEND_PROGRAM\n";
        let blocks = "FUNCTION_BLOCK Latch\nVAR_INPUT set : BOOL; reset : BOOL; END_VAR\nVAR_OUTPUT q : BOOL; END_VAR\nIF set THEN q := TRUE; END_IF;\nEND_FUNCTION_BLOCK\n";
        let running: BTreeMap<String, String> = [("p".to_string(), marked("main.st", old))].into();
        let files: BTreeMap<String, String> = [
            ("main.st".to_string(), main.to_string()),
            ("blocks.st".to_string(), blocks.to_string()),
        ]
        .into();
        let r = Report::compare(&running, &check("abc"), &files, SIZES);
        assert_eq!(r.outputs, vec!["LAMP".to_string()], "{r:?}");
        assert!(r.unknown.is_none());
        assert!(r.blocks.0 > 0);
        assert!(r.files.iter().any(|f| f.file == "blocks.st" && f.counts().0 > 0));
    }

    #[test]
    fn what_cannot_be_compared_is_unknown_never_no_change() {
        let running: BTreeMap<String, String> = [("p".to_string(), OLD.to_string())].into();
        let files: BTreeMap<String, String> =
            [("main.st".to_string(), "PROGRAM p\nnot st\n".to_string())].into();
        let r = Report::compare(&running, &check("abc"), &files, SIZES);
        assert!(r.unknown.is_some(), "{r:?}");
    }

    #[test]
    fn a_replay_is_said_in_a_sentence() {
        let r = Replay::parse(&json!({
            "window_ms": 30000,
            "signals": [{ "signal": "x", "identical": false }, { "signal": "y", "identical": true }],
            "faults": [],
        }));
        assert_eq!(
            r,
            Replay::Done {
                sentence: "Replayed 30 s of recorded values: 1 output differs: x.".into(),
                differs: true
            }
        );
    }
}
