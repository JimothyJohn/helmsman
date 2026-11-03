//! The Requirements tab (TODO.md `spec-tests`, interview 2026-11-02): the
//! project's requirement tests (`test_NAME.st`, `helmsman_iec::spec`), one
//! row each, green or red, the requirement in the engineer's words and,
//! when red, why. In the simulation the page runs them (`spec::run_all`)
//! whenever the project's build changes and before Deploy and "Use this
//! code"; on a box they come from `POST /api/projects/{p}/tests`, and the
//! box runs them again before a deploy. A red test refuses both, naming
//! the requirement.

use eframe::egui::{self, Color32, RichText, Sense};
use helmsman_iec::spec::Verdict;
use serde_json::Value as Json;

use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// One test's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub file: String,
    pub requirement: String,
    pub passed: bool,
    /// Why it is red (empty when green).
    pub why: String,
    pub scans: u32,
    pub elapsed_ms: u64,
}

impl Row {
    fn from_verdict(v: &Verdict) -> Self {
        Self {
            file: v.file.clone(),
            requirement: v.requirement.clone(),
            passed: v.passed(),
            why: v.why(),
            scans: v.scans,
            elapsed_ms: u64::try_from(v.elapsed.as_millis()).unwrap_or(u64::MAX),
        }
    }

    fn from_json(j: &Json) -> Option<Self> {
        Some(Self {
            file: j["file"].as_str()?.to_string(),
            requirement: j["requirement"].as_str().unwrap_or("").to_string(),
            passed: j["passed"].as_bool().unwrap_or(false),
            why: j["why"].as_str().unwrap_or("").to_string(),
            scans: u32::try_from(j["scans"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX),
            elapsed_ms: j["elapsed_ms"].as_u64().unwrap_or(0),
        })
    }

    /// `test_lamp` for `test_lamp.st`.
    #[must_use]
    pub fn name(&self) -> &str {
        self.file.trim_end_matches(".st")
    }
}

/// The tests' last results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    pub rows: Vec<Row>,
    /// Whether they have run at all (a project with no tests has run,
    /// with no rows).
    pub ran: bool,
}

impl Results {
    #[must_use]
    pub fn from_verdicts(v: &[Verdict]) -> Self {
        Self {
            rows: v.iter().map(Row::from_verdict).collect(),
            ran: true,
        }
    }

    /// `POST /api/projects/{p}/tests`'s body.
    #[must_use]
    pub fn from_json(j: &Json) -> Self {
        Self {
            rows: j["tests"]
                .as_array()
                .map(|a| a.iter().filter_map(Row::from_json).collect())
                .unwrap_or_default(),
            ran: true,
        }
    }

    /// The red rows, one line each: `test_lamp: lamp on in time failed at 2.00 s`.
    #[must_use]
    pub fn red(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| !r.passed)
            .map(|r| format!("{}: {}", r.name(), r.why))
            .collect()
    }

    /// Why a deploy or a "Use this code" is refused, in one line; `None`
    /// when every test is green (or there are none).
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        let red = self.red();
        match red.len() {
            0 => None,
            1 => Some(format!("Refused: requirement test {} is red.", red[0])),
            n => Some(format!(
                "Refused: {n} requirement tests are red: {}.",
                red.join("; ")
            )),
        }
    }

    #[must_use]
    pub fn green(&self) -> usize {
        self.rows.iter().filter(|r| r.passed).count()
    }
}

/// The tab: a line with the count and a Run button, then a row per test.
/// Returns whether Run was pressed. `source` says where the results came
/// from (the page, or the box) and `busy` that a run is under way.
pub fn ui(ui: &mut egui::Ui, chrome: &mut Chrome, results: &Results, source: &str, busy: bool) -> bool {
    let t = theme(ui);
    let mut run = false;
    ui.set_max_width(ui.available_width());
    ui.horizontal_wrapped(|ui| {
        let n = results.rows.len();
        let summary = if !results.ran {
            "Not run yet.".to_string()
        } else if n == 0 {
            "No requirement tests.".to_string()
        } else {
            let green = results.green();
            if green == n {
                format!("{n} of {n} green")
            } else {
                format!("{green} of {n} green")
            }
        };
        let color = if results.ran && results.green() < results.rows.len() {
            RED
        } else {
            t.fg
        };
        ui.label(RichText::new(summary).strong().color(color));
        let b = ui.add_enabled(!busy, egui::Button::new(if busy { "Running…" } else { "Run" }));
        chrome.hit("tests run", b.rect);
        if b.clicked() {
            run = true;
        }
    });
    if !source.is_empty() {
        ui.add(egui::Label::new(RichText::new(source).small().color(t.muted)).truncate());
    }
    if results.ran && results.rows.is_empty() {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "A requirement test is a test_NAME.st PROGRAM: its first comment says the \
                     requirement, it drives the program's inputs, steps the clock and checks the \
                     outputs with ASSERT, and sets done. A red one stops Deploy and Use this code.",
                )
                .small()
                .color(t.muted),
            )
            .wrap(),
        );
    }
    for r in &results.rows {
        let dot = if r.passed {
            RichText::new("●").color(t.accent)
        } else {
            RichText::new("●").color(RED)
        };
        let row = ui.horizontal(|ui| {
            ui.label(dot);
            ui.add(
                egui::Label::new(RichText::new(&r.requirement).strong())
                    .truncate()
                    .sense(Sense::click()),
            )
        });
        let row = row.inner;
        chrome.hit(format!("test {}", r.name()), row.rect);
        let detail = if r.passed {
            format!(
                "{}: green after {} scans ({:.2} s)",
                r.name(),
                r.scans,
                r.elapsed_ms as f64 / 1000.0
            )
        } else {
            format!("{}: {}", r.name(), r.why)
        };
        row.on_hover_text(&detail);
        if !r.passed {
            ui.horizontal(|ui| {
                ui.add_space(TAP * 0.4);
                ui.add(egui::Label::new(RichText::new(&r.why).small().color(RED)).wrap());
            });
        }
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use helmsman_iec::{ImageSizes, spec};

    #[test]
    fn results_from_verdicts_and_json_say_why_and_refuse() {
        let plan = spec::plan([
            (
                "main.st",
                "PROGRAM p VAR_INPUT a : BOOL; END_VAR VAR_OUTPUT b : BOOL; END_VAR b := FALSE; END_PROGRAM",
            ),
            (
                "test_b.st",
                "(* b follows a *)\nPROGRAM test_b VAR_INPUT b : BOOL; END_VAR VAR_OUTPUT a : BOOL; done : BOOL; END_VAR VAR n : INT; follows : ASSERT; END_VAR a := TRUE; n := n + 1; IF n > 2 THEN follows(COND := b); done := TRUE; END_IF; END_PROGRAM",
            ),
        ]);
        let v = spec::run_all(&plan, spec::Settings::new(ImageSizes { i: 8, q: 8, m: 64 }));
        let r = Results::from_verdicts(&v);
        assert!(r.ran && r.rows.len() == 1 && !r.rows[0].passed);
        assert_eq!(r.rows[0].requirement, "b follows a");
        assert_eq!(r.red(), vec!["test_b: follows failed at 0.02 s"]);
        assert_eq!(
            r.refusal().as_deref(),
            Some("Refused: requirement test test_b: follows failed at 0.02 s is red.")
        );
        let j = serde_json::json!({ "ok": true, "tests": [
            { "file": "test_x.st", "requirement": "x", "passed": true, "why": "", "scans": 3, "elapsed_ms": 20 },
            { "file": 7 },
        ]});
        let r = Results::from_json(&j);
        assert_eq!(r.rows.len(), 1, "a malformed row is skipped");
        assert!(r.refusal().is_none());
        assert!(Results::default().refusal().is_none());
        assert_eq!(Results::from_json(&serde_json::json!("garbage")).rows, vec![]);
    }
}
