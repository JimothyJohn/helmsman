//! The Requirements tab (Nick, 2026-11-05): a list of requirements in plain
//! words, each with the test behind it. Typing one has the agent write its
//! test; the list shows each requirement green (its test passes), red
//! (it fails, and why) or grey (no test yet). A row opens to show its test,
//! with Edit, Run and Delete. Test files (`test_NAME.st`,
//! `helmsman_iec::spec`) live behind their rows, not in the Files list.
//!
//! In the simulation the page runs the tests (`spec::run_all`) whenever
//! the project's build changes and before Deploy and "Use this code", and
//! writes a new requirement's test through its own Agent route ([`Adding`]
//! with [`Backend::Page`]). On a box the results come from
//! `POST /api/projects/{p}/tests` and a new requirement goes to
//! `POST /api/projects/{p}/requirements`, where the box's agent writes and
//! checks the test ([`Backend::Box`]). A red test refuses Deploy and "Use
//! this code"; a grey one does not.

use std::fmt::Write as _;

use eframe::egui::{self, Color32, RichText, Sense};
use helmsman_iec::spec::Verdict;
use serde_json::{Value as Json, json};

use crate::claude::{Event, Sse};
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// Longest requirement, in characters (the box's limit).
pub const MAX_REQUIREMENT: usize = 500;

/// Where a test stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Passed,
    Failed,
    /// Its requirement is written, its test not yet.
    Untested,
}

impl State {
    fn parse(s: &str) -> Self {
        match s {
            "passed" => Self::Passed,
            "untested" => Self::Untested,
            _ => Self::Failed,
        }
    }
}

/// One test's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub file: String,
    pub requirement: String,
    pub state: State,
    /// Why it is red (empty otherwise).
    pub why: String,
    pub scans: u32,
    pub elapsed_ms: u64,
}

impl Row {
    fn from_verdict(v: &Verdict) -> Self {
        Self {
            file: v.file.clone(),
            requirement: v.requirement.clone(),
            state: State::parse(v.state()),
            why: if v.red() { v.why() } else { String::new() },
            scans: v.scans,
            elapsed_ms: u64::try_from(v.elapsed.as_millis()).unwrap_or(u64::MAX),
        }
    }

    fn from_json(j: &Json) -> Option<Self> {
        let state = match j["state"].as_str() {
            Some(s) => State::parse(s),
            // A box from before the untested state.
            None if j["passed"].as_bool() == Some(true) => State::Passed,
            None => State::Failed,
        };
        Some(Self {
            file: j["file"].as_str()?.to_string(),
            requirement: j["requirement"].as_str().unwrap_or("").to_string(),
            state,
            why: if state == State::Failed {
                j["why"].as_str().unwrap_or("").to_string()
            } else {
                String::new()
            },
            scans: u32::try_from(j["scans"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX),
            elapsed_ms: j["elapsed_ms"].as_u64().unwrap_or(0),
        })
    }

    /// `test_lamp` for `test_lamp.st`.
    #[must_use]
    pub fn name(&self) -> &str {
        self.file.trim_end_matches(".st")
    }

    #[must_use]
    pub fn passed(&self) -> bool {
        self.state == State::Passed
    }

    /// What the list shows: the requirement, or the file's name when its
    /// first comment says none.
    fn words(&self) -> &str {
        if self.requirement.trim().is_empty() {
            self.name()
        } else {
            &self.requirement
        }
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
            .filter(|r| r.state == State::Failed)
            .map(|r| format!("{}: {}", r.name(), r.why))
            .collect()
    }

    /// Why a deploy or a "Use this code" is refused, in one line; `None`
    /// when no test is red (grey ones never refuse).
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
        self.rows.iter().filter(|r| r.passed()).count()
    }
}

// ------------------------------------------------------- adding one

/// Who writes the new requirement's test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The box's agent, through `POST /api/projects/{p}/requirements`.
    Box,
    /// The page's own Agent route (the visitor's key or the local proxy):
    /// the answer's PROGRAM becomes the test, checked here.
    Page,
}

/// A requirement on its way to a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// The agent is writing (or the box is checking) the test.
    Writing,
    /// The test is written (and run): the file and how it went.
    Written {
        file: String,
        state: State,
        why: String,
    },
    /// Written as "no test yet": no agent, or it wrote nothing usable.
    Untested {
        file: String,
        why: String,
    },
    Failed(String),
}

/// The request one requirement sends; carried out by `web.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    pub seq: u64,
    pub url: String,
    pub body: String,
}

#[derive(Debug)]
pub struct Adding {
    pub requirement: String,
    /// The test's name (without `test_`).
    pub name: String,
    pub backend: Backend,
    pub phase: Phase,
    /// What the agent said while it worked (the box's `text` events).
    pub said: String,
    pub seq: u64,
    sse: Sse,
    /// The page backend's answer text.
    answer: crate::claude::Answer,
}

/// Why `text` cannot be a requirement, if it cannot.
#[must_use]
pub fn refusal(text: &str) -> Option<String> {
    let t = text.trim();
    let n = t.chars().count();
    if n == 0 {
        Some("Type a requirement first.".into())
    } else if n > MAX_REQUIREMENT {
        Some(format!(
            "A requirement is at most {MAX_REQUIREMENT} characters ({n} now)."
        ))
    } else if t.chars().any(char::is_control) {
        Some("A requirement is one line.".into())
    } else {
        None
    }
}

/// A free test name for `text`: its words as a slug, unique among `taken`
/// (file names).
#[must_use]
pub fn name_for(text: &str, taken: &[String]) -> String {
    let base = helmsman_iec::spec::slug(text, 45);
    let free = |n: &str| {
        !taken
            .iter()
            .any(|f| f.eq_ignore_ascii_case(&format!("test_{n}.st")))
    };
    if free(&base) {
        return base;
    }
    (2..100)
        .map(|k| format!("{base}_{k}"))
        .find(|n| free(n))
        .unwrap_or_else(|| format!("{base}_{}", taken.len() + 100))
}

impl Adding {
    /// Starts one on the box: the request to send.
    #[must_use]
    pub fn to_box(project: &str, requirement: &str, name: &str, seq: u64) -> (Self, AddRequest) {
        let a = Self::new(requirement, name, Backend::Box, seq);
        let req = AddRequest {
            seq,
            url: format!("/api/projects/{}/requirements", crate::remote::segment(project)),
            body: json!({ "text": requirement, "name": name }).to_string(),
        };
        (a, req)
    }

    /// Starts one in the page; the caller sends the Messages request.
    #[must_use]
    pub fn in_page(requirement: &str, name: &str, seq: u64) -> Self {
        Self::new(requirement, name, Backend::Page, seq)
    }

    fn new(requirement: &str, name: &str, backend: Backend, seq: u64) -> Self {
        Self {
            requirement: requirement.trim().to_string(),
            name: name.to_string(),
            backend,
            phase: Phase::Writing,
            said: String::new(),
            seq,
            sse: Sse::default(),
            answer: crate::claude::Answer::default(),
        }
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        self.phase == Phase::Writing
    }

    /// The box's answer: a status other than 200 and its body.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub fn refused(&mut self, status: u16, body: &str) {
        let said = serde_json::from_str::<Json>(body)
            .ok()
            .and_then(|j| j["error"].as_str().map(str::to_string));
        self.phase = Phase::Failed(match (status, said) {
            (401, _) => "Sign in (top right) to add a requirement.".into(),
            (_, Some(s)) => s,
            (s, None) => format!("the box answered {s}"),
        });
    }

    /// Part of the answer's stream, as text.
    pub fn chunk(&mut self, text: &str) {
        if !self.busy() {
            return;
        }
        let events = match self.sse.push(text) {
            Ok(e) => e,
            Err(why) => {
                self.phase = Phase::Failed(why);
                return;
            }
        };
        for e in events {
            match self.backend {
                Backend::Box => self.box_event(&e),
                Backend::Page => self.answer.apply(&e),
            }
        }
    }

    /// The stream ended, or broke (`why`). For the page backend the answer
    /// is then complete: [`Self::page_program`] reads it.
    pub fn end(&mut self, why: Option<&str>) {
        if let Ok(events) = self.sse.push("\n\n") {
            for e in events {
                match self.backend {
                    Backend::Box => self.box_event(&e),
                    Backend::Page => self.answer.apply(&e),
                }
            }
        }
        if self.backend == Backend::Box && self.busy() {
            self.phase = Phase::Failed(why.map_or_else(
                || "the box closed the stream before the test was written".into(),
                str::to_string,
            ));
        }
        if self.backend == Backend::Page
            && let Some(w) = why
        {
            self.phase = Phase::Failed(w.to_string());
        }
    }

    fn box_event(&mut self, e: &Event) {
        let Ok(d) = serde_json::from_str::<Json>(&e.data) else {
            return;
        };
        let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
        match e.name.as_str() {
            "text" if self.said.len() < 4096 => self.said.push_str(&s("delta")),
            "test" => {
                let file = s("file");
                let state = State::parse(&s("state"));
                self.phase = if state == State::Untested {
                    Phase::Untested {
                        file,
                        why: String::new(),
                    }
                } else {
                    Phase::Written {
                        file,
                        state,
                        why: s("why"),
                    }
                };
            }
            "error" if self.busy() => self.phase = Phase::Failed(s("message")),
            "done" if self.busy() => {
                self.phase = Phase::Failed(s("why").chars().take(300).collect());
            }
            _ => {}
        }
    }

    /// The page backend: the PROGRAM in the agent's answer, if it wrote
    /// one named `test_<name>`.
    #[must_use]
    pub fn page_program(&self) -> Option<String> {
        let p = crate::claude::extract_program(&self.answer.text)?;
        let want = format!("PROGRAM test_{}", self.name);
        p.lines()
            .find(|l| l.trim_start().to_ascii_uppercase().starts_with("PROGRAM"))
            .is_some_and(|l| l.trim().eq_ignore_ascii_case(&want))
            .then_some(p)
    }
}

/// The page backend's request: the requirement and the project's program
/// sources, as data, to the Messages API with the test writer's prompt.
#[must_use]
pub fn page_request(
    route: &crate::claude::Route,
    model: crate::claude::Model,
    requirement: &str,
    name: &str,
    programs: &[(String, String)],
) -> crate::claude::Request {
    let data = json!({
        "requirement": requirement,
        "name": name,
        "program": programs.iter().map(|(f, t)| json!({"file": f, "text": t})).collect::<Vec<_>>(),
    });
    let user = format!(
        "Write the requirement test for the requirement below, named test_{name}.\n<box-data>\n{}\n</box-data>",
        crate::claude::neutralize_data(&data.to_string())
    );
    crate::claude::one_shot(route, model, TEST_WRITER, &user)
}

/// The test writer's prompt in the page: the relay's `helmsman/tests-v1`
/// (agent-relay `agents/helmsman/tests-v1/system.md`), answered in one
/// fenced block instead of a tool call.
pub const TEST_WRITER: &str = include_str!("tests_writer.md");

// --------------------------------------------------------------- view

/// The tab's own state: what is typed, which row is open.
#[derive(Debug, Default)]
pub struct ReqUi {
    pub draft: String,
    /// The open row's file.
    pub open: Option<String>,
    /// The Edit question: the file, the wording typed, whether the agent
    /// rewrites the test for it.
    pub editing: Option<(String, String, bool)>,
    /// The Delete question: the file.
    pub deleting: Option<String>,
}

/// What the tab asks its owner to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Add this requirement (trimmed, checked by [`refusal`]).
    Add(String),
    Run,
    /// Open this test file in the Code tab.
    Edit(String),
    Delete(String),
    /// New wording for a requirement, its test kept: the file and the
    /// whole new text of it.
    Reword {
        file: String,
        text: String,
    },
    /// New wording, and a new test from the agent: delete the file, then
    /// add the requirement as [`Act::Add`] does.
    Rewrite {
        file: String,
        requirement: String,
    },
}

/// `test` (a requirement test's text) stating `requirement` instead: its
/// first comment replaced, or the requirement put first when it has none.
/// The rest of the file is kept byte for byte.
#[must_use]
pub fn reword(test: &str, requirement: &str) -> String {
    let header = helmsman_iec::spec::header(requirement);
    let found = test
        .find("(*")
        .and_then(|s| test[s + 2..].find("*)").map(|e| (s, s + 2 + e + 2)));
    match found {
        Some((s, e)) => format!("{}{}{}", &test[..s], header.trim_end(), &test[e..]),
        None => format!("{header}{test}"),
    }
}

/// What the tab draws from.
pub struct View<'a> {
    pub results: &'a Results,
    pub adding: Option<&'a Adding>,
    /// Tests are running.
    pub busy: bool,
    /// Why a requirement cannot be added here (no project open, not signed
    /// in as an engineer, a built-in example open).
    pub cannot_add: Option<String>,
    /// The test's text, when the page holds it.
    pub source: &'a dyn Fn(&str) -> Option<String>,
    /// Where the agent runs: shown in the Add button's tooltip.
    pub agent: &'a str,
}

/// The Requirements tab.
pub fn ui(ui: &mut egui::Ui, chrome: &mut Chrome, v: &View<'_>, rui: &mut ReqUi) -> Option<Act> {
    let t = theme(ui);
    let mut act = None;
    // Room for the scroll bar: a full-width row widens a resizable panel.
    ui.set_max_width(ui.available_width() - 12.0);

    // Add a requirement.
    let add_why = v
        .cannot_add
        .clone()
        .or_else(|| {
            v.adding
                .filter(|a| a.busy())
                .map(|_| "The agent is still writing the last one.".into())
        })
        .or_else(|| refusal(&rui.draft));
    let field = ui.add(
        egui::TextEdit::singleline(&mut rui.draft)
            .id_salt("req-draft")
            .hint_text("e.g. The lamp is on within 2 s of start")
            .desired_width(ui.available_width()),
    );
    let field = field.tip(
        "A requirement in plain words: what the program must do, and when. \
         Press Enter or Add and the agent writes the test that checks it.",
    );
    chrome.hit("req draft", field.rect);
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.horizontal_wrapped(|ui| {
        let add =
            egui::Button::new(RichText::new("Add").strong()).min_size(egui::vec2(TAP * 1.5, TAP * 0.75));
        let r = ui
            .add_enabled(add_why.is_none(), add)
            .tip(add_why.clone().unwrap_or_else(|| {
                format!(
                    "Adds the requirement to the list; {} writes its test and runs it. \
                 With no agent it is added as \"no test yet\".",
                    v.agent
                )
            }));
        chrome.hit("req add", r.rect);
        if (r.clicked() || enter) && add_why.is_none() {
            act = Some(Act::Add(std::mem::take(&mut rui.draft).trim().to_string()));
        }
        let run = ui
            .add_enabled(
                !v.busy,
                egui::Button::new(if v.busy { "Running…" } else { "Run all" }),
            )
            .tip("Runs every requirement's test against the program in the simulation.");
        chrome.hit("tests run", run.rect);
        if run.clicked() {
            act = Some(Act::Run);
        }
    });

    // The one being added.
    if let Some(a) = v.adding {
        let (line, color) = match &a.phase {
            Phase::Writing => (format!("Writing the test for: {}", a.requirement), t.muted),
            Phase::Written {
                state: State::Passed, ..
            } => (format!("Added and passing: {}", a.requirement), t.fg),
            Phase::Written { why, .. } => (format!("Added, and it fails: {}. {why}", a.requirement), RED),
            Phase::Untested { why, .. } => (
                if why.is_empty() {
                    format!("Added with no test yet: {}", a.requirement)
                } else {
                    format!("Added with no test yet: {}. {why}", a.requirement)
                },
                t.muted,
            ),
            Phase::Failed(why) => (format!("Not added: {why}"), RED),
        };
        let r = ui.add(egui::Label::new(RichText::new(line).color(color)).wrap());
        let r = if a.said.trim().is_empty() {
            r.tip("The newest requirement and what became of it.")
        } else {
            r.tip(format!("The agent said: {}", a.said.trim()))
        };
        chrome.hit("req adding", r.rect);
    }
    ui.separator();

    // The list.
    let n = v.results.rows.len();
    let grey = v
        .results
        .rows
        .iter()
        .filter(|r| r.state == State::Untested)
        .count();
    let red = v.results.rows.iter().filter(|r| r.state == State::Failed).count();
    let summary = if !v.results.ran {
        "Not run yet".to_string()
    } else if n == 0 {
        "No requirements yet".to_string()
    } else {
        let mut s = format!("{} of {n} passing", v.results.green());
        if red > 0 {
            let _ = write!(s, ", {red} failing");
        }
        if grey > 0 {
            let _ = write!(s, ", {grey} with no test yet");
        }
        s
    };
    let r = ui
        .label(
            RichText::new(summary)
                .strong()
                .color(if red > 0 { RED } else { t.fg }),
        )
        .tip("A failing requirement stops Deploy and \"Use this code\"; one with no test yet does not.");
    chrome.hit("req summary", r.rect);
    for row in &v.results.rows {
        let (mark, color, state_words) = match row.state {
            State::Passed => ("✓", t.accent, "passing"),
            State::Failed => ("✗", RED, "failing"),
            State::Untested => ("○", t.muted, "no test yet"),
        };
        let open = rui.open.as_deref() == Some(row.file.as_str());
        let mut asked = (false, false);
        let r = ui
            .horizontal_wrapped(|ui| {
                ui.label(RichText::new(mark).strong().color(color));
                let l = ui.add(
                    egui::Label::new(RichText::new(row.words()).color(t.fg))
                        .wrap()
                        .sense(Sense::click()),
                );
                let why = v.cannot_add.clone();
                let e = ui
                    .add_enabled(why.is_none(), egui::Button::new("Edit").small())
                    .tip(why.clone().unwrap_or_else(|| {
                        "Changes the requirement's wording; the agent can rewrite its test to match.".into()
                    }));
                chrome.hit(format!("req edit {}", row.name()), e.rect);
                let d = ui
                    .add_enabled(why.is_none(), egui::Button::new("Delete").small())
                    .tip(why.unwrap_or_else(|| "Removes this requirement and its test. Asks first.".into()));
                chrome.hit(format!("req delete {}", row.name()), d.rect);
                asked = (e.clicked(), d.clicked());
                l
            })
            .inner;
        if asked.0 {
            rui.editing = Some((row.file.clone(), row.requirement.clone(), true));
        }
        if asked.1 {
            rui.deleting = Some(row.file.clone());
        }
        let tip = match row.state {
            State::Passed => format!(
                "{state_words}: its test passed after {} scans ({:.2} s). Click to see the test.",
                row.scans,
                row.elapsed_ms as f64 / 1000.0
            ),
            State::Failed => format!("{state_words}: {}. Click to see the test.", row.why),
            State::Untested => format!("{state_words}. Click to write or edit its test."),
        };
        let r = r.tip(tip);
        chrome.hit(format!("test {}", row.name()), r.rect);
        if r.clicked() {
            rui.open = if open { None } else { Some(row.file.clone()) };
        }
        if row.state == State::Failed && !row.why.is_empty() {
            ui.label(RichText::new(&row.why).color(RED));
        }
        if open {
            if let Some(src) = (v.source)(&row.file) {
                egui::Frame::new().fill(t.panel).inner_margin(6.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(src.trim_end()).monospace()).wrap())
                        .tip(format!("The test behind this requirement, {}.", row.file));
                });
            }
            ui.horizontal_wrapped(|ui| {
                let e = ui
                    .button("Edit test by hand")
                    .tip("Opens this test in the Code tab to change it by hand.");
                chrome.hit(format!("test edit {}", row.name()), e.rect);
                if e.clicked() {
                    act = Some(Act::Edit(row.file.clone()));
                }
                let d = ui
                    .add_enabled(v.cannot_add.is_none(), egui::Button::new("Delete"))
                    .tip(
                        v.cannot_add
                            .clone()
                            .unwrap_or_else(|| "Removes this requirement and its test.".into()),
                    );
                chrome.hit(format!("test delete {}", row.name()), d.rect);
                if d.clicked() {
                    rui.deleting = Some(row.file.clone());
                }
            });
        }
        ui.add_space(4.0);
    }
    if let Some(a) = edit_ui(ui.ctx(), chrome, v, rui) {
        act = Some(a);
    }
    if let Some(a) = delete_ui(ui.ctx(), chrome, v, rui) {
        act = Some(a);
    }
    act
}

/// The Edit question: the wording, whether the agent rewrites the test.
fn edit_ui(ctx: &egui::Context, chrome: &mut Chrome, v: &View<'_>, rui: &mut ReqUi) -> Option<Act> {
    let (file, mut text, mut rewrite) = rui.editing.clone()?;
    let old = v
        .results
        .rows
        .iter()
        .find(|r| r.file == file)
        .map(|r| r.requirement.clone())
        .unwrap_or_default();
    let mut out = None;
    let mut done = false;
    let m = egui::Modal::new(egui::Id::unique("req-edit")).show(ctx, |ui| {
        ui.set_min_width(480.0);
        ui.set_max_width(480.0);
        let t = theme(ui);
        ui.label(RichText::new("Edit the requirement").strong().size(20.0));
        ui.add_space(8.0);
        let tip = "The requirement in plain words: what the program must do, and when.";
        let f = ui
            .add(
                egui::TextEdit::multiline(&mut text)
                    .id_salt("req-edit-text")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            )
            .tip(tip);
        chrome.hit("req edit text", f.rect);
        let c = ui
            .checkbox(&mut rewrite, format!("Have {} rewrite its test for the new words", v.agent))
            .tip("On: the old test goes and the agent writes one that checks the new wording. Off: only the words change; the test stays as it is.");
        chrome.hit("req edit rewrite", c.rect);
        let why = refusal(&text).or_else(|| v.cannot_add.clone());
        let same = text.trim() == old.trim() && !rewrite;
        if let Some(w) = &why {
            ui.add(egui::Label::new(RichText::new(w).color(RED)).wrap());
        } else if same {
            ui.label(RichText::new("Nothing to change yet.").color(t.muted));
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let tall = egui::vec2(0.0, TAP * 0.7);
            let ok = ui
                .add_enabled(why.is_none() && !same, egui::Button::new(RichText::new("Save").strong()).min_size(tall))
                .tip(if rewrite {
                    "Replaces the requirement and asks the agent for its new test."
                } else {
                    "Changes the requirement's words in its test file."
                });
            chrome.hit("req edit ok", ok.rect);
            let hand = ui
                .add(egui::Button::new("Edit test by hand").min_size(tall))
                .tip("Opens the test in the Code tab instead.");
            chrome.hit("req edit hand", hand.rect);
            let no = ui
                .add(egui::Button::new("Cancel").min_size(tall))
                .tip("Leaves the requirement as it is.");
            chrome.hit("req edit cancel", no.rect);
            if ok.clicked() {
                let requirement = text.trim().to_string();
                out = Some(if rewrite {
                    Act::Rewrite {
                        file: file.clone(),
                        requirement,
                    }
                } else {
                    let src = (v.source)(&file).unwrap_or_default();
                    Act::Reword {
                        file: file.clone(),
                        text: reword(&src, &requirement),
                    }
                });
                done = true;
            } else if hand.clicked() {
                out = Some(Act::Edit(file.clone()));
                done = true;
            } else if no.clicked() {
                done = true;
            }
        });
    });
    chrome.hit("req edit form", m.response.rect);
    if done || m.should_close() {
        rui.editing = None;
    } else {
        rui.editing = Some((file, text, rewrite));
    }
    out
}

/// The Delete question.
fn delete_ui(ctx: &egui::Context, chrome: &mut Chrome, v: &View<'_>, rui: &mut ReqUi) -> Option<Act> {
    let file = rui.deleting.clone()?;
    let words = v
        .results
        .rows
        .iter()
        .find(|r| r.file == file)
        .map_or_else(|| file.clone(), |r| r.words().to_string());
    let mut out = None;
    let mut done = false;
    let m = egui::Modal::new(egui::Id::unique("req-delete")).show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.label(RichText::new("Delete this requirement?").strong().size(20.0));
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(format!(
                "\u{201c}{words}\u{201d} and its test ({file}) are removed."
            ))
            .wrap(),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let tall = egui::vec2(0.0, TAP * 0.7);
            let ok = ui
                .add(egui::Button::new(RichText::new("Delete").strong().color(RED)).min_size(tall))
                .tip("Removes the requirement and its test.");
            chrome.hit("req delete ok", ok.rect);
            let no = ui
                .add(egui::Button::new("Cancel").min_size(tall))
                .tip("Keeps it.");
            chrome.hit("req delete cancel", no.rect);
            if ok.clicked() {
                out = Some(Act::Delete(file.clone()));
                done = true;
            } else if no.clicked() {
                done = true;
            }
        });
    });
    chrome.hit("req delete form", m.response.rect);
    if done || m.should_close() {
        rui.deleting = None;
    }
    out
}

#[cfg(test)]
mod reword_tests {
    use super::reword;

    #[test]
    fn reword_replaces_the_first_comment_only() {
        let t = "(* The lamp is on within 2 s of start *)\nPROGRAM test_lamp\n(* step 1 *)\nEND_PROGRAM\n";
        let out = reword(t, "The lamp is on within 1 s of start");
        assert_eq!(
            out,
            "(* The lamp is on within 1 s of start *)\nPROGRAM test_lamp\n(* step 1 *)\nEND_PROGRAM\n"
        );
        assert_eq!(
            helmsman_iec::spec::stated(&out).as_deref(),
            Some("The lamp is on within 1 s of start")
        );
        // A comment closer in the words cannot end the comment early.
        let out = reword(t, "a *) b");
        assert_eq!(helmsman_iec::spec::stated(&out).as_deref(), Some("a * ) b"));
        assert!(out.ends_with("PROGRAM test_lamp\n(* step 1 *)\nEND_PROGRAM\n"));
        // None yet: put first.
        assert_eq!(
            reword("PROGRAM p\nEND_PROGRAM\n", "x"),
            "(* x *)\nPROGRAM p\nEND_PROGRAM\n"
        );
    }
}
