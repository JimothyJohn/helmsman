//! The Editor on the simulation in the page (docs/playground/, or any page
//! no box serves): an ST editor that checks as you type (errors marked
//! after a pause; what runs is unchanged until Deploy, as on a box), the
//! program running in the page with Run / Stop / Step, a virtual I/O
//! panel built from the program's ports, a trend, the live diagram, and
//! Claude with the visitor's own key. No box and no server: the state is
//! [`crate::playground::Session`] and [`crate::claude::Chat`].

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use helmsman_diagram::Scan;
use helmsman_iec::{Ty, Value};

use crate::arrange::Layers;
use crate::claude::{Chat, Model, Phase, Proxy, Request};
use crate::editor::{self, Body, Chrome, Tab};
use crate::playground::{MAX_SOURCE, Point, Session, TREND_SIGNALS, Widget, nice_range, tick_label};
use crate::projects::{self, Act, Diag, FilesUi, Listing, Workspace};
use crate::sim_projects::{SimProjects, SimRevision, Storage};
use crate::st_view::brand::{BLACK, GREEN, OFF_RED, WARN, WHITE, YELLOW};
use crate::st_view::{Pick, TAP, Viewer};
use crate::theme::{Theme, theme};
use crate::tips::Tip;

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// Quiet time after the last keystroke before the editor checks, s.
const DEBOUNCE: f64 = 0.35;
/// Trend line colours, in signal order (the last, white, is the page's ink
/// in light mode).
const TREND: [Color32; TREND_SIGNALS] = [
    YELLOW,
    Color32::from_rgb(0x6E, 0xC6, 0xFF),
    Color32::from_rgb(0x9B, 0xE5, 0x8C),
    Color32::from_rgb(0xFF, 0x8A, 0x65),
    Color32::from_rgb(0xD5, 0x9C, 0xFF),
    WHITE,
];

/// Where Claude requests go: the browser, or (natively) a list tests read.
#[derive(Default)]
pub struct Link {
    #[cfg(target_arch = "wasm32")]
    web: crate::web::ClaudeLink,
    /// Requests "sent" (native builds have no network here).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<(u64, Request)>,
    #[cfg(not(target_arch = "wasm32"))]
    pub inbox: Vec<crate::claude::Incoming>,
    /// Answers stopped.
    #[cfg(not(target_arch = "wasm32"))]
    pub aborted: usize,
}

impl Link {
    fn send(&mut self, seq: u64, req: Request, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.send(seq, req, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.sent.push((seq, req));
        }
    }

    fn abort(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.web.abort();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.aborted += 1;
        }
    }

    fn check_proxy(&mut self, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.check_proxy(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.inbox.push(crate::claude::Incoming::Proxy(false));
        }
    }

    fn drain(&mut self) -> Vec<crate::claude::Incoming> {
        #[cfg(target_arch = "wasm32")]
        return self.web.drain();
        #[cfg(not(target_arch = "wasm32"))]
        std::mem::take(&mut self.inbox)
    }
}

pub struct PlaygroundView {
    pub session: Session,
    /// The Messages tab's log.
    pub log: crate::messages::Log,
    /// What the log last saw, to report changes once.
    seen: Seen,
    /// The diagram of the program running, rebuilt when it changes.
    pub viewer: Option<Viewer>,
    /// The Agent's last change, marked on the diagram and in the code
    /// until the file is edited ([`crate::agent_marks`]).
    pub agent_change: Option<crate::agent_marks::AgentChange>,
    /// Its shared default layout (the project's) and this browser's
    /// tweaks (`arrange.rs`).
    pub layers: Layers,
    /// The viewer was just built: lay it out from the layers.
    fresh: bool,
    viewer_of: String,
    /// When the editor last changed and is not checked yet.
    pending: Option<f64>,
    /// The diagram's pick last frame (a new one sends the ST there).
    last_pick: Option<Pick>,
    pub chat: Chat,
    pub link: Link,
    proxy_asked: bool,
    /// The device library's index, fetched when the Devices tab scans.
    #[cfg(target_arch = "wasm32")]
    library: crate::web::TextFetch,
    /// A one-line note under the top bar (loaded an example, …).
    pub notice: Option<String>,
    /// The text of `notice` while it says the program is running in this
    /// page: it goes when the program stops, faults or restarts.
    running_notice: Option<String>,
    /// The projects, kept in the browser's storage.
    pub proj: SimProjects,
    pub files_ui: FilesUi,
    pub devices_ui: crate::devices::DevicesUi,
    /// What `session.sensors` was bound from: the program running and the
    /// project's files, so a frame rebinds only after a change.
    sensors_from: Option<(String, Vec<(String, String)>)>,
    /// The question "what is this program called?", asked before the
    /// first program the Agent writes from a built-in example lands.
    pub program_name: Option<ProgramName>,
    /// The project the Agent's new programs land in, once named.
    pub new_program: Option<String>,
    /// The Agent tab (`crate::instruct`).
    pub instruct: crate::instruct::Instruct,
    /// The Agent's answer [`Self::settle`] last took.
    answered_seq: u64,
    /// A checked new program waiting for its project's name.
    to_name: Option<crate::instruct::Apply>,
    /// The revision on view (the History modal).
    pub revision: Option<crate::history_ui::RevisionView>,
    /// The project's requirement tests, as last run in this page
    /// (`crate::requirements`): after every build change, before Deploy
    /// and "Use this code".
    pub tests: crate::requirements::Results,
    /// The lines selected in the Code tab (file, first, last), as the
    /// editor last drew them: the Agent's scope when there are some.
    pub selection: Option<(String, u32, u32)>,
    /// Downloads made (natively nothing saves them: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub downloads: Vec<(String, Vec<u8>)>,
}

impl PlaygroundView {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let projects: Box<dyn Storage> = Box::new(crate::web::LocalProjects);
        #[cfg(not(target_arch = "wasm32"))]
        let projects: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_storage(projects)
    }

    /// The simulation with its projects kept in `projects`; it opens the
    /// project open last (else the first example), running.
    #[must_use]
    pub fn with_storage(projects: Box<dyn Storage>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let tweaks: Box<dyn Storage> = Box::new(crate::web::LocalArrangements);
        #[cfg(not(target_arch = "wasm32"))]
        let tweaks: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_stores(projects, tweaks)
    }

    /// [`Self::with_storage`], with the diagram's personal layouts kept in
    /// `tweaks`.
    #[must_use]
    pub fn with_stores(projects: Box<dyn Storage>, tweaks: Box<dyn Storage>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::web::LocalKeyStore);
        #[cfg(not(target_arch = "wasm32"))]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::claude::NoStore);
        let proj = SimProjects::load(projects);
        let mut v = Self {
            session: Session::new(&proj.build_src()),
            viewer: None,
            agent_change: None,
            layers: Layers::load(tweaks),
            fresh: false,
            viewer_of: String::new(),
            pending: None,
            last_pick: None,
            chat: Chat::new(store),
            link: Link::default(),
            proxy_asked: false,
            #[cfg(target_arch = "wasm32")]
            library: crate::web::TextFetch::default(),
            log: crate::messages::Log::default(),
            seen: Seen::default(),
            notice: None,
            running_notice: None,
            proj,
            files_ui: FilesUi::default(),
            devices_ui: crate::devices::DevicesUi::default(),
            sensors_from: None,
            program_name: None,
            new_program: None,
            instruct: crate::instruct::Instruct::default(),
            answered_seq: 0,
            to_name: None,
            revision: None,
            tests: crate::requirements::Results::default(),
            selection: None,
            #[cfg(not(target_arch = "wasm32"))]
            downloads: Vec::new(),
        };
        v.session.set_running(true);
        v.proj.running = Some(v.proj.open.clone());
        v.sync_viewer();
        v.run_tests();
        v
    }

    /// How this page runs the project's requirement tests: the session's
    /// scan period, the simulation's image sizes.
    fn test_settings(&self) -> helmsman_iec::spec::Settings {
        helmsman_iec::spec::Settings::new(crate::playground::SIZES)
            .period(std::time::Duration::from_millis(self.session.period_ms.max(1)))
    }

    /// Runs the open project's requirement tests against its build and
    /// keeps the results (the deploy gate reads them).
    pub fn run_tests(&mut self) {
        let plan = self.proj.test_plan();
        let v = helmsman_iec::spec::run_all(&plan, self.test_settings());
        self.tests = crate::requirements::Results::from_verdicts(&v);
    }

    /// The gate "Use this code" and Deploy share: why the project (with
    /// `extra` added or replacing a file, for a candidate) must not land,
    /// if a requirement test is red. Runs the tests on the candidate and,
    /// when it is the project as it stands, keeps the results.
    fn tests_refuse(&mut self, extra: Option<(&str, &str)>) -> Option<String> {
        let settings = self.test_settings();
        let results = match extra {
            None => {
                self.run_tests();
                self.tests.clone()
            }
            Some((file, text)) => {
                let mut files: std::collections::BTreeMap<&str, &str> = self
                    .proj
                    .files
                    .iter()
                    .map(|(n, t)| (n.as_str(), t.as_str()))
                    .collect();
                files.insert(file, text);
                let plan = helmsman_iec::spec::plan(
                    files
                        .iter()
                        .filter(|(n, _)| {
                            helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St)
                        })
                        .map(|(n, t)| (*n, *t)),
                );
                crate::requirements::Results::from_verdicts(&helmsman_iec::spec::run_all(&plan, settings))
            }
        };
        results.refusal()
    }

    /// Checks a PROGRAM answer (`text`) landing as `main.st` of the saved
    /// project `name`, with the answer's `scenarios` added: the project's
    /// own files with them, compiled and its requirement tests run, as the
    /// project will stand. The scenarios come back named free among the
    /// project's files (they were named free among the files of the open
    /// example the answer was checked against). Why not, if it must not land.
    fn program_into(
        &self,
        name: &str,
        text: &str,
        scenarios: &[(String, String)],
    ) -> Result<Vec<(String, String)>, String> {
        let Some(target) = self.proj.saved.get(name) else {
            return Ok(scenarios.to_vec());
        };
        let mut files = target.0.clone();
        files.insert(helmsman_project::MAIN.to_string(), text.to_string());
        let mut taken: Vec<String> = files.keys().cloned().collect();
        let mut named = Vec::new();
        for (file, t) in scenarios {
            let want = file
                .trim_end_matches(".st")
                .trim_start_matches(helmsman_iec::spec::TEST_PREFIX);
            let free = format!(
                "{}{}.st",
                helmsman_iec::spec::TEST_PREFIX,
                crate::requirements::name_for(want, &taken)
            );
            taken.push(free.clone());
            files.insert(free.clone(), t.clone());
            named.push((free, t.clone()));
        }
        let plan = helmsman_iec::spec::plan(
            files
                .iter()
                .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
                .map(|(n, t)| (n.as_str(), t.as_str())),
        );
        if let Err(d) = helmsman_iec::compile(&plan.main, crate::playground::SIZES) {
            let first = d.first().map_or_else(String::new, ToString::to_string);
            return Err(format!(
                "Refused: with {name}'s other files the program does not compile: {first}"
            ));
        }
        let verdicts = helmsman_iec::spec::run_all(&plan, self.test_settings());
        match crate::requirements::Results::from_verdicts(&verdicts).refusal() {
            Some(why) => Err(format!("{why} (the program would replace main.st of {name})")),
            None => Ok(named),
        }
    }

    /// Loads the open project's build into the session, compiled and
    /// stopped.
    fn reload(&mut self) {
        let src = self.proj.build_src();
        self.session.load(&src);
        self.pending = None;
        self.sync_viewer();
        self.run_tests();
    }

    /// Opens project `name` and runs it if it compiles (Nick, 2026-11-01:
    /// an example that opens stopped looks dead); an edit still waits for
    /// Deploy.
    pub fn open_project(&mut self, name: &str) {
        self.proj.open_project(name);
        self.reload();
        self.notice = None;
        if self.session.build.as_ref().is_some_and(|b| b.module.is_some()) {
            self.session.set_running(true);
            self.proj.running = Some(self.proj.open.clone());
        }
    }

    /// The editor's text changed: the session's source follows at once
    /// and is checked after the pause. What runs waits for Deploy.
    fn edited(&mut self, t: f64) {
        self.session.src = self.proj.build_src();
        self.pending = Some(t);
        self.notice = None;
    }

    /// The compile errors, then the checker's warnings, each with its
    /// file and line.
    #[must_use]
    pub fn diags(&self) -> Vec<Diag> {
        let at = |d: &helmsman_iec::Diagnostic| {
            let (file, line) = self
                .proj
                .locate(d.pos.line)
                .unwrap_or_else(|| (self.proj.current.clone(), d.pos.line));
            (file, line, d.pos.col, d.msg.clone())
        };
        let errors = self.session.diags.iter().map(|d| {
            let (f, l, c, m) = at(d);
            Diag::error(f, l, c, m)
        });
        let warnings = self.session.warnings.iter().map(|d| {
            let (f, l, c, m) = at(d);
            Diag::warning(f, l, c, m)
        });
        errors.chain(warnings).collect()
    }

    /// Opens `file` at `line` in the ST editor.
    fn goto(&mut self, file: &str, line: u32, chrome: &mut Chrome) {
        self.proj.open_file(file);
        chrome.goto(line);
    }

    /// Carries out what the Files sidebar asked.
    fn act(&mut self, a: Act) {
        match a {
            Act::UseAnswer { name, text, save } => {
                // A block answer lands in the open project: the project's
                // requirement tests must stay green with it in. (A PROGRAM
                // answer opens a new project, which has no tests yet.)
                if !text.to_ascii_uppercase().contains("PROGRAM ") {
                    let file = self.proj.free_file(&format!("{name}.st"));
                    if let Some(why) = self.tests_refuse(Some((&file, &text))) {
                        self.notice = Some(why.clone());
                        self.log.error(
                            self.session.now_ms as f64 / 1000.0,
                            format!("Use this code: {why}"),
                        );
                        return;
                    }
                }
                // A PROGRAM answer into a project that exists replaces its
                // main.st: that project's own tests must hold with it, and
                // the scenarios take names free there.
                let mut scenarios = self.to_name.as_ref().map(|a| a.tests.clone()).unwrap_or_default();
                if text.to_ascii_uppercase().contains("PROGRAM ") && self.proj.saved.contains_key(&name) {
                    match self.program_into(&name, &text, &scenarios) {
                        Ok(named) => scenarios = named,
                        Err(why) => {
                            self.notice = Some(why.clone());
                            self.log.error(
                                self.session.now_ms as f64 / 1000.0,
                                format!("Use this code: {why}"),
                            );
                            return;
                        }
                    }
                }
                // Unsaved edits first (a save is a revision: none when
                // there is nothing to save).
                if save && self.proj.dirty() {
                    self.proj.save();
                } else if !save {
                    let open = self.proj.open.clone();
                    self.proj.open_project(&open);
                }
                let mut dropped = 0;
                if text.to_ascii_uppercase().contains("PROGRAM ") {
                    dropped = self.land_program(&name, text);
                    // The scenarios its answer passed, kept as its tests.
                    for (f, t) in scenarios {
                        self.proj.files.insert(f, t);
                    }
                } else {
                    let file = self.proj.free_file(&format!("{name}.st"));
                    self.proj.new_file(&file);
                    if let Some(t) = self.proj.current_text_mut() {
                        *t = text;
                    }
                }
                // Recorded with the instruction that produced it.
                let apply = self.to_name.take();
                let what = apply.as_ref().map_or_else(
                    || format!("took the Agent's answer into {}", self.proj.current),
                    |a| format!("Agent: {}", a.instruction).chars().take(120).collect(),
                );
                self.proj
                    .save_as(&what, apply.as_ref().map(|a| a.record.as_str()));
                self.reload();
                if let Some(a) = &apply {
                    self.instruct.applied(a, None);
                    self.log_outcome();
                }
                let size = if dropped > 0 {
                    format!(" {dropped} oldest iteration(s) dropped: the project was past its size.")
                } else {
                    String::new()
                };
                self.notice = Some(format!(
                    "{} opened from the Agent's answer; Deploy runs it.{size}",
                    self.proj.current
                ));
            }
            Act::OpenProject(p) => self.open_project(&p),
            Act::OpenFile(f) => self.proj.open_file(&f),
            Act::ShowRevision(n) => {
                self.revision = self
                    .proj
                    .revisions()
                    .iter()
                    .find(|r| r.n == n)
                    .map(SimRevision::view);
            }
            Act::NewProject(p) => {
                self.proj.create(&p, None);
                self.reload();
            }
            Act::CopyProject { from, to } | Act::FromStarter { from, to } => {
                self.proj.create(&to, Some(&from));
                self.reload();
            }
            Act::RenameProject { from, to } => self.proj.rename(&from, &to),
            Act::DeleteProject(p) => {
                let open = self.proj.open == p;
                self.proj.delete(&p);
                if open {
                    self.reload();
                }
            }
            Act::NewFile(f) => self.proj.new_file(&f),
            Act::RenameFile { from, to } => {
                self.proj.rename_file(&from, &to);
                self.edited(f64::NEG_INFINITY);
            }
            Act::DeleteFile(f) => {
                self.proj.delete_file(&f);
                self.edited(f64::NEG_INFINITY);
            }
            Act::CopyFile(f) => {
                if let Some(text) = self.proj.files.get(&f) {
                    self.files_ui.clipboard = Some((f, text.clone()));
                }
            }
            Act::DuplicateFile(f) => {
                if let Some(text) = self.proj.files.get(&f).cloned() {
                    let name = self.proj.free_file(&f);
                    self.proj.add_file(&name, text);
                    self.edited(f64::NEG_INFINITY);
                }
            }
            Act::PasteFile { name, text } => {
                let name = self.proj.free_file(&name);
                self.proj.add_file(&name, text);
                self.edited(f64::NEG_INFINITY);
            }
            Act::Deploy => self.deploy(),
        }
    }

    /// Rebuilds the diagram when the program running changed.
    fn sync_viewer(&mut self) {
        let Some(b) = &self.session.build else {
            self.viewer = None;
            return;
        };
        if b.src == self.viewer_of && self.viewer.is_some() {
            return;
        }
        self.viewer_of.clone_from(&b.src);
        let old = self.viewer.as_ref().and_then(|v| v.because.clone());
        // Blocks the user moved stay moved across edits: kept as tweaks,
        // and the new diagram is laid out from the layers.
        if let Some(v) = &self.viewer {
            self.layers.track(v);
        }
        self.fresh = true;
        self.viewer = b.module.clone().map(|m| {
            let mut v = Viewer::new(&b.src, m);
            // Opens with no path lit; the user's pick survives an edit
            // while its variable does.
            v.because = old.filter(|o| v.knows(o));
            v
        });
    }

    /// Checks the editor if it has been quiet long enough (or `now`):
    /// errors are marked, and the program running (and its diagram) stay
    /// as deployed.
    fn check_due(&mut self, t: f64, now: bool) {
        if let Some(at) = self.pending
            && (now || t - at >= DEBOUNCE)
        {
            self.pending = None;
            let src = self.session.src.clone();
            self.session.check(&src);
            self.run_tests();
        }
    }

    /// Picks output `name`, as a tap on it does.
    pub fn pick(&mut self, name: &str) {
        if let Some(v) = &mut self.viewer {
            v.because = Some(name.to_string());
        }
    }

    /// What the header names: the open project.
    #[must_use]
    pub fn title(&self) -> String {
        self.proj.open.clone()
    }

    /// The PROGRAM running, in lower case (the Monitor's name for it);
    /// the project's name while nothing compiled.
    #[must_use]
    pub fn program_name(&self) -> String {
        self.viewer
            .as_ref()
            .and_then(|v| v.module.program())
            .map_or_else(|| self.title(), |p| p.name.to_ascii_lowercase())
    }

    /// The project's devices and points as text, the agent's context
    /// (what the Devices tab shows, values included).
    #[must_use]
    pub fn devices_text(&self) -> String {
        let text = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        let map = helmsman_project::iomap::skim(&text);
        let points: Vec<(String, Value)> = self
            .session
            .inputs()
            .chain(self.session.outputs())
            .map(|p| (p.name.clone(), p.value))
            .collect();
        let live = |name: &str| {
            points
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| crate::devices::Live {
                    value: Some(*v),
                    quality: None,
                    forced: false,
                })
        };
        let mut text = crate::devices::describe(&crate::devices::tree(&map, &live, &|_| {
            crate::devices::LinkState::Simulated
        }));
        text.push_str(&crate::devices::describe_found(&self.devices_ui.found));
        text.push_str(&crate::plant::describe(&self.plant_states(&map)));
        text
    }

    /// The project's plant-model states (`{state}` declarations), each
    /// with the simulation's value and the map signal that measures it.
    #[must_use]
    pub fn plant_states(&self, map: &helmsman_project::iomap::MapText) -> Vec<crate::plant::PlantState> {
        let files: Vec<(&str, &str)> = self
            .proj
            .files
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        let unit = std::cell::OnceCell::new();
        let value = |pou: &str, name: &str| {
            let trace = self.session.trace.as_ref()?;
            let src = self.session.build.as_ref().map_or("", |b| b.src.as_str());
            let unit = unit.get_or_init(|| helmsman_iec::ast::parse(src).ok()).as_ref()?;
            crate::plant::value_in_trace(trace, unit, pou, name)
        };
        let live = |name: &str| self.session.value(name);
        let mut states = crate::plant::states(&files, map, &value, &live);
        for s in &mut states {
            s.from_sensor = s.measured_by.as_ref().is_some_and(|m| {
                self.session
                    .sensors
                    .iter()
                    .any(|(_, signal)| signal.eq_ignore_ascii_case(&m.signal))
            });
        }
        states
    }

    /// Points the simulation's measured plant states at their sensors
    /// (`Session::sensors`): a state whose map `in` signal is one of the
    /// program's inputs runs from that input instead of the model.
    pub fn bind_sensors(&mut self) {
        let src = self.session.build.as_ref().map_or("", |b| b.src.as_str());
        if self.sensors_from.as_ref().is_some_and(|(s, f)| {
            s == src
                && f.len() == self.proj.files.len()
                && f.iter()
                    .zip(&self.proj.files)
                    .all(|((a, x), (b, y))| a == b && x == y)
        }) {
            return;
        }
        let src = src.to_string();
        let files: Vec<(String, String)> = self
            .proj
            .files
            .iter()
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        let map = helmsman_project::iomap::skim(
            self.proj
                .files
                .get(helmsman_project::IO_MAP)
                .map_or("", String::as_str),
        );
        let states = self.plant_states(&map);
        let mut sensors = crate::plant::substitutions(&states, &src);
        sensors.retain(|(_, signal)| self.session.inputs().any(|p| p.name.eq_ignore_ascii_case(signal)));
        self.session.sensors = sensors;
        self.sensors_from = Some((src, files));
    }

    /// The project as the Agent sees it, in `scope`: what
    /// [`crate::instruct`] checks an answer against.
    fn agent_env(&self) -> (Vec<String>, String, helmsman_iec::spec::Settings) {
        (
            self.session.diag_lines(),
            self.devices_text(),
            self.test_settings(),
        )
    }

    /// Sends the typed instruction about the file in scope, with the last
    /// instructions carried out on that file (read from History).
    pub fn send_instruction(&mut self, ctx: &egui::Context) {
        if let Err(why) = self.chat.route() {
            self.instruct.failed(why);
            return;
        }
        let scope = self.scope();
        let file = crate::instruct::target(&scope);
        let earlier = crate::instruct::recent(
            self.proj.revisions().iter().filter_map(|r| r.page.as_deref()),
            &file,
        );
        let (diags, devices, settings) = self.agent_env();
        let env = crate::instruct::Env {
            project: &self.proj.open,
            scope,
            files: &self.proj.files,
            source: &self.session.src,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        match self.instruct.start(&env, &earlier) {
            Ok(ask) => self.send_ask(&ask, ctx),
            Err(why) => self.instruct.failed(&why),
        }
    }

    /// One request of the instruction under way, to the Agent.
    fn send_ask(&mut self, ask: &crate::instruct::Ask, ctx: &egui::Context) {
        match self.chat.send(&ask.message, &ask.context) {
            Ok((seq, req)) => self.link.send(seq, req, ctx),
            Err(why) => self.instruct.failed(&why),
        }
    }

    /// Stop: the answer under way is dropped and nothing is written.
    pub fn stop_answer(&mut self) {
        self.stop_answer_with("stopped");
    }

    /// [`Self::stop_answer`], saying `why` in the instruction's outcome.
    fn stop_answer_with(&mut self, why: &str) {
        self.chat.stop();
        self.link.abort();
        if self.instruct.busy() {
            self.instruct.failed(why);
        }
    }

    /// Loads the program in Claude's answer (never runs it).
    /// Loads `src` into the editor (the open file, or the project's first
    /// program file when a map or settings file is open), compiled and
    /// stopped.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn load(&mut self, src: &str, what: &str) {
        if crate::sim_projects::is_st(&self.proj.current) {
            if let Some(t) = self.proj.current_text_mut() {
                src.clone_into(t);
            }
        } else if let Some(f) = self
            .proj
            .files
            .keys()
            .find(|f| crate::sim_projects::is_st(f))
            .cloned()
        {
            self.proj.files.insert(f.clone(), src.to_string());
            self.proj.current = f;
        } else {
            self.proj.files.insert("main.st".into(), src.to_string());
            self.proj.current = "main.st".into();
        }
        let ok = self.session.load(&self.proj.build_src());
        self.pending = None;
        self.sync_viewer();
        self.notice = Some(if ok {
            format!("Loaded {what}. Press Run to start it.")
        } else {
            format!("Loaded {what}; it does not compile yet (see the errors under the editor).")
        });
    }

    /// The code into a new project (a PROGRAM, from a built-in example).
    /// A PROGRAM goes to the project named for the Agent's programs
    /// (Nick, 2026-11-01: named once, asked here the first time, see
    /// [`ProgramName`]); a block goes into the open project as a file.
    pub fn take_code(&mut self, code: String) {
        let is_program = code.to_ascii_uppercase().contains("PROGRAM ");
        let name = match (&self.new_program, is_program) {
            (Some(p), true) => p.clone(),
            (None, true) => {
                // The PROGRAM's name to start from; past a built-in example's
                // name (read-only), the next free one.
                let want = answer_name(&code);
                let input = if self.proj.examples.contains_key(&want) {
                    self.proj.free(&want)
                } else {
                    want
                };
                self.program_name = Some(ProgramName {
                    text: code.clone(),
                    input,
                    error: None,
                });
                return;
            }
            (_, false) => answer_name(&code),
        };
        self.use_named(name, code);
    }

    /// The code into project (or file) `name`, after the unsaved-edits
    /// question when there is one.
    fn use_named(&mut self, name: String, code: String) {
        let act = Act::UseAnswer {
            name,
            text: code,
            save: true,
        };
        if self.proj.dirty() {
            self.files_ui.confirm = Some(act);
        } else {
            self.act(act);
        }
    }

    /// The name typed for the conversation's program is taken: the
    /// pending code lands there (the modal's OK; tests call it).
    pub fn confirm_program_name(&mut self) {
        let Some(mut p) = self.program_name.take() else {
            return;
        };
        let name = p.input.trim().to_ascii_lowercase();
        if !helmsman_project::project_name(&name) {
            p.error =
                Some("A program's name is 1 to 48 of a-z 0-9 _ -, starting with a letter or digit.".into());
            self.program_name = Some(p);
            return;
        }
        if self.proj.examples.contains_key(&name) {
            p.error = Some(format!(
                "{name} is a built-in example and read-only: pick another name."
            ));
            self.program_name = Some(p);
            return;
        }
        self.new_program = Some(name.clone());
        self.use_named(name, p.text);
    }

    /// A PROGRAM answer into project `project` (made if there is none):
    /// the newest is always `main.st`; the one it replaced becomes the
    /// next `vN.st`, kept under "earlier" and never built. When the
    /// project would outgrow its size, the oldest iterations go first;
    /// returns how many went.
    fn land_program(&mut self, project: &str, text: String) -> usize {
        let fresh = !self.proj.exists(project);
        if fresh {
            self.proj.create(project, None);
            // `create` opens it with the starter program; the answer takes its place.
            self.proj.files.clear();
        } else if self.proj.open != project {
            self.proj.open_project(project);
        }
        let main = helmsman_project::MAIN;
        if let Some(old) = self.proj.files.remove(main) {
            let n = helmsman_project::Files(self.proj.files.clone()).next_iteration();
            self.proj.files.insert(helmsman_project::iteration_name(n), old);
        }
        self.proj.files.insert(main.to_string(), text);
        self.proj.current = main.to_string();
        let mut dropped = 0;
        while helmsman_project::Files(self.proj.files.clone()).total() > helmsman_project::MAX_TOTAL {
            let oldest = helmsman_project::Files(self.proj.files.clone())
                .iterations()
                .next()
                .map(|(_, f)| f.to_string());
            let Some(oldest) = oldest else { break };
            self.proj.files.remove(&oldest);
            dropped += 1;
        }
        dropped
    }

    /// The ST file the Agent's scope starts from: the open one, or the
    /// project's first program file when a map or settings file is open.
    fn scope_file(&self) -> Option<String> {
        if crate::sim_projects::is_st(&self.proj.current) && self.proj.files.contains_key(&self.proj.current)
        {
            return Some(self.proj.current.clone());
        }
        self.proj
            .files
            .keys()
            .find(|f| crate::sim_projects::is_st(f))
            .cloned()
    }

    /// Where the paused scan stands, as the files show it: `12` or
    /// `12 of motor.st`.
    fn paused_line(&self) -> Option<String> {
        let id = self.session.paused_at()?;
        let line = self.session.trace.as_ref()?.map().stmt(id)?.line;
        Some(match self.proj.locate(line) {
            Some((file, l)) => format!("{l} of {file}"),
            None => line.to_string(),
        })
    }

    /// The block picked in the diagram as a scope: its file, its name and
    /// the lines of the statement it was lowered from. Only while the
    /// diagram is of the text in the editor (an edit not yet deployed
    /// moves the lines under it).
    fn picked_block(&self) -> Option<(String, String, u32, u32)> {
        let v = self.viewer.as_ref()?;
        if v.src != self.proj.build_src() {
            return None;
        }
        let d = v.diagram();
        let (stmt, name) = match v.pick? {
            Pick::Node(n) => {
                let n = d.nodes.get(n.0 as usize)?;
                let name = match &n.kind {
                    helmsman_diagram::NodeKind::Fb { instance, .. } => instance.clone(),
                    helmsman_diagram::NodeKind::Write { path }
                    | helmsman_diagram::NodeKind::Read { path } => path.clone(),
                    _ if !n.label.trim().is_empty() => n.label.clone(),
                    _ => n.text.clone(),
                };
                (n.stmt, name)
            }
            Pick::Region(r) => {
                let r = d.regions.get(r.0 as usize)?;
                (r.stmt, r.label.clone())
            }
        };
        let s = d.stmts.iter().find(|s| s.id == stmt)?;
        let (file, line) = self.proj.locate(s.pos.line)?;
        let text = self.proj.files.get(&file)?;
        let end = crate::scope::statement_end(text, line, s.pos.col);
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let name = if name.chars().count() > 32 {
            format!("{}…", name.chars().take(31).collect::<String>())
        } else {
            name
        };
        Some((file, name, line, end))
    }

    /// What the Agent may change now (`crate::scope`): the lines selected
    /// in the Code tab, else the block picked in the diagram, else the
    /// open file; a new project while a built-in example is open.
    #[must_use]
    pub fn scope(&self) -> crate::scope::Scope {
        crate::scope::detect(&crate::scope::Focus {
            writable: !self.proj.is_example(),
            open: self.scope_file(),
            selection: self.selection.clone(),
            block: self.picked_block(),
        })
    }

    /// The ST editor's widget id for the open file (its selection is the
    /// Agent's scope).
    pub fn source_id(&self) -> egui::Id {
        editor::source_id("pg-source", &self.proj.open, &self.proj.current)
    }

    /// Back to the whole file: the selection and the pick go.
    pub fn whole_file(&mut self, ctx: &egui::Context) {
        self.selection = None;
        editor::deselect(ctx, self.source_id());
        if let Some(v) = &mut self.viewer {
            v.pick = None;
        }
    }

    /// An instruction belongs to the project it was typed in: when another
    /// is open before the answer is in, the run ends with nothing written,
    /// so no retry is built from the other project's source and tests.
    fn drop_run_of_another_project(&mut self) {
        let moved = self
            .instruct
            .run
            .as_ref()
            .is_some_and(|r| r.project != self.proj.open);
        if moved {
            self.stop_answer_with("the open project changed while the Agent was answering");
            self.log_outcome();
        }
    }

    /// A finished answer to the instruction under way goes through
    /// [`crate::instruct::Instruct::answered`]: back to the Agent while it
    /// fails its checks, written when it passes, refused when it still
    /// fails. A broken or stopped answer ends the instruction.
    fn settle(&mut self, ctx: &egui::Context) {
        use crate::instruct::Step;
        if !self.instruct.busy() || self.to_name.is_some() {
            return;
        }
        let Some(a) = &self.chat.answer else { return };
        if a.running() || self.chat.seq() == self.answered_seq {
            return;
        }
        self.answered_seq = self.chat.seq();
        let text = match &a.phase {
            Phase::Done => a.text.clone(),
            Phase::Failed(why) => {
                let why = why.clone();
                self.instruct.failed(&why);
                return self.log_outcome();
            }
            Phase::Waiting | Phase::Streaming => return,
        };
        let Some(scope) = self.instruct.run.as_ref().map(|r| r.scope.clone()) else {
            return;
        };
        let (diags, devices, settings) = self.agent_env();
        let env = crate::instruct::Env {
            project: &self.proj.open,
            scope,
            files: &self.proj.files,
            source: &self.session.src,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        match self.instruct.answered(&text, &env) {
            Step::Ask(ask) => self.send_ask(&ask, ctx),
            Step::Apply(apply) => self.write(apply),
            Step::Done => self.log_outcome(),
        }
    }

    /// The outcome of the last instruction, in Messages.
    fn log_outcome(&mut self) {
        use crate::instruct::Kind;
        let Some(o) = &self.instruct.outcome else { return };
        let at = self.session.now_ms as f64 / 1000.0;
        let mut line = o.status.clone();
        for p in &o.problems {
            line.push(' ');
            line.push_str(p);
        }
        match o.kind {
            Kind::Applied | Kind::Undone | Kind::Said => self.log.info(at, line),
            Kind::Refused | Kind::Failed => self.log.warning(at, line),
        }
    }

    /// The Agent's last change, marked while its file is what the Agent
    /// wrote: the editor's line marks for the file open, the diagram's
    /// baseline. The first edit (any other text) clears them.
    pub(crate) fn agent_marks(&mut self, chrome: &mut Chrome) {
        let text = self
            .agent_change
            .as_ref()
            .and_then(|c| self.proj.files.get(&c.file))
            .map(String::as_str);
        if self
            .agent_change
            .as_ref()
            .is_some_and(|c| !c.holds(&self.proj.open, text))
        {
            self.agent_change = None;
        }
        let c = self.agent_change.as_ref();
        chrome.st_marks = c
            .filter(|c| c.file == self.proj.current)
            .map(|c| c.lines().clone());
        if let Some(v) = &mut self.viewer {
            v.baseline = c.and_then(|c| c.baseline.clone());
        }
    }

    /// A checked answer into the open project: unsaved edits are saved
    /// first as their own revision, so the version before the change is
    /// one; then the file (and the scenarios it passed, as requirement
    /// tests) are saved as one revision whose History entry is the
    /// instruction's record. A new program from a built-in example goes
    /// the way it always has, into a project named once.
    fn write(&mut self, apply: crate::instruct::Apply) {
        if apply.scope == crate::scope::Scope::NewProject {
            let code = apply.text.clone();
            self.to_name = Some(apply);
            self.take_code(code);
            return;
        }
        if self.proj.open != apply.project {
            let why = format!("{} is no longer open.", apply.project);
            self.instruct.not_written(&apply.file, &why);
            return self.log_outcome();
        }
        if self.proj.dirty() {
            self.proj.save();
        }
        let saved: std::collections::BTreeMap<String, String> = self
            .proj
            .files
            .iter()
            .filter(|(n, _)| !helmsman_project::agent::is_agent_file(n))
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        if self.proj.revisions().last().is_none_or(|r| r.files != saved) {
            self.proj.save_as("saved before the Agent's change", None);
        }
        let before = self.proj.revisions().last().map(|r| r.n);
        let was = self.proj.files.get(&apply.file).cloned().unwrap_or_default();
        let baseline = self.viewer.as_ref().map(|v| std::rc::Rc::new(v.module.clone()));
        self.agent_change = Some(crate::agent_marks::AgentChange::new(
            &apply.project,
            &apply.file,
            &was,
            apply.text.clone(),
            baseline,
        ));
        self.proj.files.insert(apply.file.clone(), apply.text.clone());
        for (f, t) in &apply.tests {
            self.proj.files.insert(f.clone(), t.clone());
        }
        self.proj.current.clone_from(&apply.file);
        let what: String = format!("Agent: {}", apply.instruction)
            .chars()
            .take(120)
            .collect();
        self.proj.save_as(&what, Some(&apply.record));
        self.reload();
        let undo = before.map(|revision| crate::instruct::Undo {
            project: apply.project.clone(),
            file: apply.file.clone(),
            revision,
            text: apply.text.clone(),
        });
        self.instruct.applied(&apply, undo);
        self.log_outcome();
    }

    /// Why Undo is off now, if it is.
    fn undo_blocked(&self) -> Option<String> {
        let u = self.instruct.undo()?;
        if self.proj.open != u.project {
            return Some(format!("Open {} first.", u.project));
        }
        self.instruct.undo_refusal(
            self.proj.files.get(&u.file).map(String::as_str),
            self.proj.dirty(),
        )
    }

    /// Undo: the project back to the revision before the Agent's change,
    /// saved as a new revision (the change stays in History).
    pub fn undo_change(&mut self) {
        let Some(u) = self.instruct.undo().cloned() else {
            return;
        };
        if self.undo_blocked().is_some() {
            return;
        }
        if self.proj.restore(u.revision) {
            if self.proj.files.contains_key(&u.file) {
                self.proj.current.clone_from(&u.file);
            }
            self.reload();
            self.instruct.undone(&u);
            self.log_outcome();
        }
    }

    /// The revision on view, in the middle of the screen, until Close or
    /// Restore (a built-in example is read-only).
    fn revision_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(view) = self.revision.clone() else {
            return;
        };
        let current: std::collections::BTreeMap<String, String> = self
            .proj
            .files
            .iter()
            .filter(|(n, _)| !helmsman_project::agent::is_agent_file(n))
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        let write = if self.proj.is_example() {
            Err("A built-in example is read-only: copy the project first.".into())
        } else {
            Ok(())
        };
        match crate::history_ui::modal(ctx, chrome, &view, &current, &write) {
            Some(crate::history_ui::HistoryAct::Close) => self.revision = None,
            Some(crate::history_ui::HistoryAct::Restore(n)) => {
                self.revision = None;
                if self.proj.restore(n) {
                    self.reload();
                    self.notice = Some(format!("Revision {n} restored."));
                }
            }
            None => {}
        }
    }

    /// The "Name the program" question as a box in the middle of the
    /// screen (the view draws it each frame after the Editor).
    fn program_name_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(mut p) = self.program_name.clone() else {
            return;
        };
        let mut done = false;
        let mut ok = false;
        let m = crate::theme::near_pointer(ctx, "pg-program-name").show(ctx, |ui| {
            ui.set_max_width(420.0);
            let why = "The Agent's answers in this conversation land in a project of this name: \
                       the newest as main.st, the earlier ones kept under it.";
            ui.label(RichText::new("Name this program").strong().size(16.0))
                .tip(why);
            ui.add_space(8.0);
            let field = ui
                .add(
                    egui::TextEdit::singleline(&mut p.input)
                        .hint_text("a-z 0-9 _ -")
                        .desired_width(f32::INFINITY),
                )
                .tip(format!("{why} Enter confirms."));
            chrome.hit("program name", field.rect);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(e) = &p.error {
                ui.colored_label(crate::theme::theme(ui).ink(RED), e);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let yes = ui
                    .add(egui::Button::new(RichText::new("Use this code").strong()).min_size(tall))
                    .tip("Makes the project under this name and takes the code into it (Enter).");
                chrome.hit("program ok", yes.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Leaves the code in the answer. Nothing is made.");
                chrome.hit("program cancel", no.rect);
                if yes.clicked() || enter {
                    ok = true;
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("program form", m.response.rect);
        // OK first: the click that pressed it also reads as a click on
        // the modal's backdrop, which would otherwise shut the question.
        if ok {
            self.program_name = Some(p);
            self.confirm_program_name();
        } else if done || m.should_close() {
            self.program_name = None;
            if let Some(a) = self.to_name.take() {
                self.instruct
                    .not_written(&a.file, "No name was given, so no project was made.");
            }
        } else {
            self.program_name = Some(p);
        }
    }

    /// One frame of the Editor.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.tick(&ui.ctx().clone());
        self.agent_marks(chrome);
        editor::show(ui, chrome, self);
        if self.devices_ui.take_import() {
            #[cfg(target_arch = "wasm32")]
            crate::web::pick_file(crate::app::DEVICE_ACCEPT, &ui.ctx().clone());
        }
        if let Some(crate::devices::DeviceAct::Add(w)) = self.devices_ui.form_ui(&ui.ctx().clone(), chrome) {
            self.apply_wiring(&w);
        }
        if let Some(line) = self.devices_ui.plant.form_ui(&ui.ctx().clone(), chrome) {
            self.add_device_line(&line);
        }
        if self.devices_ui.renaming.is_some() {
            let files: Vec<(String, String)> = self
                .proj
                .files
                .iter()
                .map(|(n, t)| (n.clone(), t.clone()))
                .collect();
            self.devices_ui.plan_rename(&files);
        }
        if let Some(crate::devices::DeviceAct::Delete(name)) =
            self.devices_ui.delete_ui(&ui.ctx().clone(), chrome)
            && let Some(t) = self.proj.files.get_mut(helmsman_project::IO_MAP)
        {
            *t = crate::device_form::remove_device(t, &name);
            self.proj.note(format!("{name} deleted from io.map."), false);
            self.edited(f64::NEG_INFINITY);
        }
        if let Some(crate::devices::DeviceAct::Rename(plan)) =
            self.devices_ui.rename_ui(&ui.ctx().clone(), chrome)
        {
            for (f, text, _) in plan.files {
                if let Some(t) = self.proj.files.get_mut(&f) {
                    *t = text;
                }
            }
            self.proj
                .note(format!("Renamed {} to {}.", plan.old, plan.new), false);
            self.edited(f64::NEG_INFINITY);
        }
        if let Some(a) = self.files_ui.naming_ui(&ui.ctx().clone(), chrome) {
            self.act(a);
        }
        if let Some(a) = self.files_ui.confirm_ui(&ui.ctx().clone(), chrome) {
            self.act(a);
        }
        // Drawn after the unsaved-edits question: a modal that appears in
        // the frame a click is released in reads that release as a click
        // on its backdrop and shuts itself.
        self.program_name_ui(&ui.ctx().clone(), chrome);
        self.revision_ui(&ui.ctx().clone(), chrome);
    }

    /// Runs the program (and Claude's answer) on with nothing drawn: the
    /// Monitor watches the same session.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if !self.proxy_asked {
            self.proxy_asked = true;
            self.link.check_proxy(ctx);
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(r) = self.library.take() {
            match r.and_then(|t| helmsman_project::devlib::Index::parse(&t)) {
                Ok(index) => self.devices_ui.attach_library(&index),
                Err(why) => {
                    self.devices_ui.library_note =
                        Some(format!("the device library could not be read: {why}"));
                }
            }
        }
        for m in self.link.drain() {
            self.chat.receive(m);
        }
        self.drop_run_of_another_project();
        self.settle(ctx);
        let t = ctx.input(|i| i.time);
        self.check_due(t, false);
        self.arrange();
        self.bind_sensors();
        self.session.advance(t);
        self.watch(t);
        if self.session.running || self.pending.is_some() || self.chat.busy() || self.instruct.busy() {
            ctx.request_repaint();
        }
    }
}

impl PlaygroundView {
    /// What the guided tour reads of this view (`crate::tour`), with the
    /// Editor on screen and the trend panel as `chrome` has it.
    #[must_use]
    pub fn tour_seen(&self, chrome: &Chrome) -> crate::tour::Seen {
        use std::hash::{Hash, Hasher};
        let hash = |f: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            f(&mut h);
            h.finish()
        };
        let blink = crate::sim_projects::example_project(crate::playground::EXAMPLES[0].src).0;
        let trend = &self.session.trend;
        crate::tour::Seen {
            editor: true,
            naming_blink: self.files_ui.naming == Some(projects::Naming::FromStarter(blink)),
            open: self.proj.open.clone(),
            example: self.proj.is_example(),
            source: hash(&|h| self.proj.files.hash(h)),
            compiles: self.pending.is_none() && self.session.diags.is_empty(),
            live: self.session.running
                && !self.session.stale()
                && self.proj.running.as_deref() == Some(self.proj.open.as_str()),
            picked: self
                .viewer
                .as_ref()
                .filter(|v| v.pick.is_some() || v.because.is_some())
                .map(|v| format!("{:?} {:?}", v.pick, v.because)),
            trend: hash(&|h| {
                trend.window_ms.hash(h);
                trend.signals.hash(h);
            }),
            asks: self.chat.seq(),
            keyed: self.chat.route().is_ok(),
            trend_open: chrome.trend && chrome.bottom == editor::Bottom::Trend,
        }
    }
}

/// What [`PlaygroundView::watch`] compares against.
#[derive(Default)]
struct Seen {
    fault: Option<String>,
    diags: Vec<String>,
    warned: Option<String>,
    notice: Option<String>,
    storage: Option<String>,
    running: Option<bool>,
    scans: u64,
}

impl PlaygroundView {
    /// Reports to the log what changed since the last frame: compile
    /// errors (as they appear), a fault, a notice, a storage problem,
    /// the program starting or stopping.
    fn watch(&mut self, t: f64) {
        let fault = self.session.fault.as_ref().map(ToString::to_string);
        let ended = (self.seen.running == Some(true) && !self.session.running)
            || (fault.is_some() && fault != self.seen.fault)
            || self.session.scans < self.seen.scans;
        self.seen.scans = self.session.scans;
        if ended
            && self
                .running_notice
                .take()
                .is_some_and(|n| self.notice.as_ref() == Some(&n))
        {
            self.notice = None;
        }
        if fault != self.seen.fault {
            if let Some(f) = &fault {
                self.log
                    .error(t, format!("Fault: {f}. Run or Step restarts the program."));
            }
            self.seen.fault = fault;
        }
        let warned = projects::warning_summary(&self.diags());
        if warned != self.seen.warned {
            if let Some(w) = &warned {
                self.log.warning(t, w.clone());
            }
            self.seen.warned = warned;
        }
        let diags = self.session.diag_lines();
        if diags != self.seen.diags {
            if !diags.is_empty() {
                let n = diags.len();
                self.log
                    .error(t, format!("{n} problem(s): {}", diags.join(" · ")));
            } else if !self.seen.diags.is_empty() {
                self.log.info(t, "Compiles.");
            }
            self.seen.diags = diags;
        }
        let notice = self.proj.notice.as_ref().map(|(n, _)| n.clone());
        if notice != self.seen.notice {
            if let Some((n, error)) = &self.proj.notice {
                if *error {
                    self.log.error(t, n.clone());
                } else {
                    self.log.info(t, n.clone());
                }
            }
            self.seen.notice = notice;
        }
        if self.proj.storage_problem != self.seen.storage {
            if let Some(w) = &self.proj.storage_problem {
                self.log.warning(t, w.clone());
            }
            self.seen.storage.clone_from(&self.proj.storage_problem);
        }
        let running = Some(self.session.running);
        if running != self.seen.running {
            if self.seen.running.is_some() {
                self.log.info(
                    t,
                    if self.session.running {
                        "Running."
                    } else {
                        "Stopped."
                    },
                );
            }
            self.seen.running = running;
        }
    }

    /// Lays the diagram out from its layers, or keeps a finished move.
    pub fn arrange(&mut self) {
        let fresh = std::mem::take(&mut self.fresh);
        if let Some(v) = &mut self.viewer {
            let key = format!("sim:{}", self.proj.open);
            self.layers.show(v, &key, self.proj.layout(), fresh);
        }
    }

    /// Saves the diagram as it stands as the open project's default.
    pub fn save_layout(&mut self) {
        let Some(v) = &self.viewer else { return };
        let text = v.arrangement().to_text();
        if self.proj.save_layout(&text) {
            self.layers.adopt(v, &text);
        }
    }
}

impl Body for PlaygroundView {
    fn toolbar(&mut self, ui: &mut egui::Ui, _chrome: &mut Chrome) {
        // Run, step and restart are glyphs over the diagram (app::glyphs);
        // here only what state the program is in.
        let s = &self.session;
        let state = if s.fault.is_some() {
            "faulted"
        } else if s.paused.is_some() {
            "paused"
        } else if s.running {
            "running"
        } else {
            "stopped"
        };
        #[allow(clippy::cast_precision_loss)]
        let secs = s.now_ms as f64 / 1000.0;
        ui.label(
            RichText::new(format!("{state} · {secs:.1}s"))
                .monospace()
                .strong()
                .color(if s.running && s.fault.is_none() {
                    crate::theme::theme(ui).ink(GREEN)
                } else {
                    crate::theme::theme(ui).ink(RED)
                }),
        )
        .tip(format!(
            "The simulation's state and clock, after {} scans.",
            s.scans
        ));
    }

    fn has_notices(&self) -> bool {
        self.session.fault.is_some()
            || self.session.paused.is_some()
            || self.notice.is_some()
            || self.proj.notice.is_some()
            || self.proj.storage_problem.is_some()
    }

    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if let Some(f) = &self.session.fault {
            let text = format!("Fault: {f}. Run or Step restarts the program.");
            crate::messages::copyable(ui, chrome, "fault", &text, crate::theme::theme(ui).ink(RED));
        }
        if self.session.paused.is_some() {
            let at = self
                .paused_line()
                .map_or_else(String::new, |l| format!(" before line {l}"));
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    crate::theme::theme(ui).ink(WARN),
                    format!("Paused at a breakpoint{at}."),
                )
                .tip("The diagram and the values show the scan as it stood just before that statement.");
                ui.label(
                    RichText::new("F8 runs the rest of the scan. Ctrl Enter goes on to the next stop.")
                        .color(t.muted),
                );
            });
        }
        if let Some(n) = &self.notice {
            ui.label(RichText::new(n).color(t.muted));
        }
        if let Some((n, error)) = self.proj.notice.clone() {
            if error {
                crate::messages::copyable(ui, chrome, "notice", &n, crate::theme::theme(ui).ink(RED));
            } else {
                let r = ui.label(RichText::new(n).color(t.muted));
                chrome.hit("notice", r.rect);
            }
            if error && (self.proj.pending.is_some() || self.proj.pending_file.is_some()) {
                let r = ui
                    .button("Replace")
                    .tip("Saves your version over the one kept in this browser.");
                chrome.hit("replace", r.rect);
                if r.clicked() {
                    self.proj.replace();
                    self.reload();
                }
            }
        }
        if let Some(why) = &self.proj.storage_problem {
            let r = ui.label(RichText::new(format!("⚠ {why}")).color(t.muted));
            chrome.hit("storage", r.rect);
        }
    }

    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let now = ui.ctx().input(|i| i.time);
        let stale = self.session.stale();
        let errors = self.session.diags.len();
        // An edit is only checked: what runs is the last deploy, as on a box.
        let undeployed = stale && self.pending.is_none() && self.session.build.is_some();
        let mut go = false;
        // The header is the open file's name (Nick, 2026-11-08), then word
        // wrap, then the → that deploys this version.
        let name = self.proj.current.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            editor::file_header(
                ui,
                chrome,
                &name,
                "The open file. It is checked as you type; Deploy runs it.",
            );
            editor::wrap_toggle(ui, chrome);
            go = crate::projects::deploy_arrow(
                ui,
                chrome,
                "code deploy",
                &Ok(()),
                "Deploys this version: compiles the open project and runs it in the page (Ctrl Shift Enter, File › Deploy).",
            );
            if self.pending.is_some() {
                ui.add(crate::theme::whole(RichText::new("…").color(t.muted)))
                    .tip("Checking the edit.");
            } else if errors > 0 {
                ui.add(crate::theme::whole(
                    RichText::new(format!("✖ {errors} error(s)")).color(crate::theme::theme(ui).ink(RED)),
                ))
                .tip("The list under the code has each one. Click an error to go to its line.");
            } else {
                ui.add(crate::theme::whole(RichText::new("✔ compiles").color(t.muted)))
                    .tip("The program has no errors.");
            }
            if undeployed {
                let note = if errors > 0 {
                    "The program running is unchanged. Fix the errors, then Deploy."
                } else {
                    "The program running is unchanged. Deploy runs this version."
                };
                ui.add(crate::theme::whole(RichText::new("· not deployed").color(t.muted)))
                    .tip(note);
            }
        });
        if go {
            self.deploy();
        }
        let all = self.diags();
        let current = self.proj.current.clone();
        let diags = projects::marks_in(&all, &current, false);
        let warns = projects::marks_in(&all, &current, true);
        let avail = ui.available_height();
        let list_h = if all.is_empty() { 0.0 } else { 110.0 };
        let size = Vec2::new(ui.available_width(), (avail - list_h).max(120.0));
        let id = self.source_id();
        let Some(text) = self.proj.current_text_mut() else {
            ui.label(RichText::new("No file open.").color(t.muted))
                .tip("Open or make one in Files.");
            return;
        };
        let r = ui
            .allocate_ui(size, |ui| {
                editor::st_editor(ui, id, text, &diags, &warns, MAX_SOURCE, chrome)
            })
            .inner;
        chrome.hit("st editor", r.rect);
        if r.changed() {
            self.edited(now);
        }
        // The selected lines, the Agent's scope while there are some. Read
        // while the editor has focus: an editor without it drops its
        // selection, and moving to the Agent tab takes the focus.
        if ui.ctx().memory(|m| m.has_focus(id)) {
            self.selection = editor::selection(ui.ctx(), id).and_then(|(a, b)| {
                let text = self.proj.files.get(&current)?;
                crate::scope::selected_lines(text, a, b).map(|(from, to)| (current.clone(), from, to))
            });
        } else if self.selection.as_ref().is_some_and(|(f, ..)| *f != current) {
            self.selection = None;
        }
        if !all.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("pg-diags")
                .max_height(list_h)
                .show(ui, |ui| {
                    if let Some(d) = projects::diags_ui(ui, &all, chrome) {
                        self.goto(&d.file, d.line, chrome);
                    }
                });
        }
    }

    /// The projects (the visitor's, then the examples), the open one's
    /// files, and under them the program's outline: every block instance
    /// it calls, nested; a click shows that one's diagram.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let rows = self.proj.rows();
        let files: Vec<(String, bool)> = self
            .proj
            .files
            .keys()
            // The Agent's pages are the Agent tab's, not a file to open.
            .filter(|f| !helmsman_project::agent::is_agent_file(f))
            .map(|f| (f.clone(), self.proj.file_dirty(f)))
            .collect();
        let open = self.proj.open.clone();
        let current = self.proj.current.clone();
        let history: Vec<crate::history_ui::RevisionRow> =
            self.proj.revisions().iter().map(SimRevision::row).collect();
        let listing = Listing {
            projects: &rows,
            open: Some(&open),
            files,
            current: Some(&current),
            write: Ok(()),
            dirty: self.proj.dirty(),
            starters: false,
            running: self.proj.running.as_deref() == Some(open.as_str()),
            history: &history,
            // The page deploys at once: never busy.
            deploy: Ok(()),
        };
        if let Some(a) = self.files_ui.show(ui, &listing, chrome) {
            self.act(a);
        }
        ui.separator();
        ui.label(RichText::new("Program").strong().color(t.muted))
            .tip("The compiled program: each POU and the instances it calls. Click one to open its diagram.");
        match &mut self.viewer {
            Some(v) => {
                ui.indent("pg-tree", |ui| v.tree_ui(ui));
            }
            None => {
                ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            }
        }
        // The blank space under it all: a right-click offers a new project
        // (and a paste into the open one).
        if let Some(a) = self.files_ui.blank_menu(ui, chrome, &Ok(()), true) {
            self.act(a);
        }
    }

    fn devices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::devices::{LinkState, Live, tree};
        let text = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        let map = helmsman_project::iomap::skim(&text);
        let points: Vec<(String, Value)> = self
            .session
            .inputs()
            .chain(self.session.outputs())
            .map(|p| (p.name.clone(), p.value))
            .collect();
        let live = |name: &str| {
            points
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| Live {
                    value: Some(*v),
                    quality: None,
                    forced: false,
                })
        };
        let nodes = tree(&map, &live, &|_| LinkState::Simulated);
        let source = if text.is_empty() {
            String::new()
        } else {
            format!("io.map of {}", self.proj.open)
        };
        let write = if self.proj.is_example() {
            Err("The examples are read-only: copy the project first (File › Copy project).".into())
        } else {
            Ok(())
        };
        self.devices_ui.plant.states = self.plant_states(&map);
        self.devices_ui.plant.devices = crate::plant::device_choices(&map);
        self.devices_ui.plant.simulated = true;
        self.devices_ui.copy_first = self.proj.is_example();
        self.devices_ui.program = crate::device_form::ports_of(&self.proj.build_src());
        self.devices_ui.map = map.clone();
        self.devices_ui.eds_files = self
            .proj
            .files
            .keys()
            .filter(|f| crate::devices::is_eds(f))
            .cloned()
            .collect();
        match self.devices_ui.show(ui, &nodes, &source, chrome, &write) {
            Some(crate::devices::DeviceAct::Scan) => {
                self.devices_ui.simulated_scan(&map);
                #[cfg(target_arch = "wasm32")]
                self.library
                    .fetch(helmsman_project::devlib::DEFAULT_URL, &ui.ctx().clone());
            }
            Some(crate::devices::DeviceAct::Use(d)) => self.use_device(d),
            Some(crate::devices::DeviceAct::OpenMap) => {
                self.proj.open_file(helmsman_project::IO_MAP);
                chrome.show_code();
            }
            Some(
                crate::devices::DeviceAct::Add(_)
                | crate::devices::DeviceAct::Rename(_)
                | crate::devices::DeviceAct::Delete(_),
            )
            | None => {}
        }
    }

    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.trend_tab(ui, chrome);
    }

    fn messages(&mut self) -> &mut crate::messages::Log {
        &mut self.log
    }

    fn tabs(&self) -> &'static [Tab] {
        &[Tab::Io, Tab::Claude]
    }

    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab, chrome: &mut Chrome) {
        match tab {
            Tab::Io => self.io(ui, chrome),
            Tab::Claude => self.claude(ui, chrome),
        }
    }

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        if let Some(v) = &mut self.viewer {
            v.icons
                .update(self.proj.files.get(crate::icons::FILE).map(String::as_str));
            v.crumbs_ui(ui, &[]);
            let area = ui.available_rect_before_wrap();
            v.breaks.clone_from(&self.session.breaks);
            v.paused = self.session.paused_at();
            let now = ui.input(|i| i.time);
            for t in self.session.earlier() {
                v.saw(t, now);
            }
            v.canvas(ui, scan);
            let mut jump = None;
            if v.pick != self.last_pick {
                self.last_pick = v.pick;
                if let Some((line, _)) = v.picked() {
                    jump = Some(line);
                    // The later act wins: a pick is the scope now.
                    self.selection = None;
                }
            }
            let reads = v.reads_on_screen();
            self.map_inputs(ui, area, &reads);
            if let Some(line) = jump {
                match self.proj.locate(line) {
                    Some((file, l)) => self.goto(&file, l, chrome),
                    None => chrome.goto(line),
                }
            }
            return;
        }
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg);
        let why = self
            .session
            .build
            .as_ref()
            .and_then(|b| b.module_error.clone())
            .unwrap_or_else(|| "Write a PROGRAM that compiles to see its diagram.".into());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            why,
            FontId::proportional(crate::theme::BODY),
            t.muted,
        );
    }
}

impl PlaygroundView {
    fn io(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            return;
        }
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        let outputs: Vec<Point> = self.session.outputs().cloned().collect();
        let on_map = self.on_map();
        let off_map: Vec<&Point> = inputs
            .iter()
            .filter(|p| !on_map.iter().any(|r| r.eq_ignore_ascii_case(&p.name)))
            .collect();
        egui::ScrollArea::vertical().id_salt("pg-io").show(ui, |ui| {
            let inputs_tip = if inputs.is_empty() {
                "No VAR_INPUT or %IX bit: add one to get a control."
            } else if off_map.len() < inputs.len() {
                "Switch, press or drag the inputs here, or on their blocks in the diagram."
            } else {
                "Switch, press or drag the inputs here."
            };
            ui.label(RichText::new("Inputs").strong()).tip(inputs_tip);
            if inputs.is_empty() {
                ui.label(RichText::new("none").color(t.muted)).tip(inputs_tip);
            }
            // Two columns when the panel is wide enough for them (Nick,
            // 2026-11-01: denser); each cell capped at its column's width,
            // or the row overflows and the resizable panel grows to fit.
            let (cols, col) = columns(ui);
            let mut set = Vec::new();
            egui::Grid::new("pg-io-in").num_columns(cols).show(ui, |ui| {
                for pair in off_map.chunks(cols) {
                    for p in pair {
                        ui.scope(|ui| {
                            ui.set_max_width(col);
                            if let Some(v) = input_ui(ui, p) {
                                set.push((p.name.clone(), v));
                            }
                        });
                    }
                    ui.end_row();
                }
            });
            for (name, v) in set {
                if let Err(why) = self.session.try_input(&name, v) {
                    self.notice = Some(why);
                }
            }
            ui.add_space(8.0);
            let outputs_tip = if outputs.is_empty() {
                "No VAR_OUTPUT or %QX bit: add one to get a lamp."
            } else {
                "Click an output to pick its block in the diagram."
            };
            ui.label(RichText::new("Outputs").strong()).tip(outputs_tip);
            if outputs.is_empty() {
                ui.label(RichText::new("none").color(t.muted)).tip(outputs_tip);
            }
            let mut picked = None;
            let (cols, col) = columns(ui);
            let grid = egui::Grid::new("pg-io-out").num_columns(cols).show(ui, |ui| {
                for pair in outputs.chunks(cols) {
                    for p in pair {
                        ui.scope(|ui| {
                            ui.set_max_width(col);
                            if output_ui(ui, p) {
                                picked = Some(p.name.clone());
                            }
                        });
                    }
                    ui.end_row();
                }
            });
            if !outputs.is_empty() {
                chrome.hit("io outputs", grid.response.rect);
            }
            if let Some(name) = picked {
                self.pick(&name);
            }
        });
    }

    /// The signals to trend (outputs, then inputs) and the chart.
    fn trend_tab(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            return;
        }
        let points: Vec<String> = self
            .session
            .outputs()
            .chain(self.session.inputs())
            .map(|p| p.name.clone())
            .collect();
        // The heading shares the signals' row: on a line of its own it
        // and the slider left the plot the smallest part of the panel.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Signals").strong().color(t.muted))
                .tip(format!(
                    "Click a signal to pin it to the trend, up to {TREND_SIGNALS}."
                ));
            for name in &points {
                let on = self.session.trend.signals.contains(name);
                let tip = if on {
                    "On the trend. Click to take it off."
                } else {
                    "Click to pin it to the trend."
                };
                if crate::theme::tab(ui, on, &name.to_ascii_lowercase(), egui::Vec2::ZERO)
                    .tip(tip)
                    .clicked()
                {
                    self.session.trend.toggle(name);
                }
            }
        });
        window_slider(ui, chrome, &mut self.session.trend.window_ms);
        ui.add_space(6.0);
        let plot = self.trend_plot(ui);
        if let Some((from, to)) = lane_handles(ui, chrome, plot, &self.session.trend.signals) {
            self.session.trend.move_signal(from, to);
        }
    }

    fn claude(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().id_salt("pg-claude").show(ui, |ui| {
            // Room for the scroll bar: a full-width field plus the bar is
            // wider than the panel, and a resizable panel grows to fit.
            ui.set_max_width(ui.available_width() - 12.0);
            match self.chat.proxy {
                Proxy::Yes => {
                    ui.label(RichText::new("Using the local proxy").color(t.muted))
                        .tip("The API key stays on this machine.");
                }
                Proxy::Checking => {
                    ui.label(RichText::new("Checking for a local proxy…").color(t.muted))
                        .tip("With a local proxy the API key stays on this machine.");
                }
                Proxy::No => self.key_ui(ui, chrome),
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Model").color(t.muted))
                    .tip("The model that answers. A larger one is slower and costs more.");
                // No wider than the panel: a row that overflows a resizable
                // panel widens it every frame. `width` is only a floor, so
                // the selected text is truncated to the room (2026-11-08:
                // with two tabs the panel is narrower than this row was).
                egui::ComboBox::from_id_salt("pg-model")
                    .width((ui.available_width() - 40.0).max(60.0))
                    .wrap_mode(egui::TextWrapMode::Truncate)
                    // The name alone: the note in brackets is in the list.
                    .selected_text(self.chat.model.label().split(" (").next().unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for m in Model::ALL {
                            ui.selectable_value(&mut self.chat.model, m, m.label())
                                .tip(format!("Answers with {}.", m.label()));
                        }
                    })
                    .response
                    .tip("The model that answers. A larger one is slower and costs more.");
            });
            ui.add_space(4.0);
            self.scope_ui(ui, chrome);
            let view = crate::instruct::View {
                target: self.scope().label().replacen("Editing: ", "", 1),
                blocked: self.chat.route().err().map(str::to_string),
                undo_blocked: self.undo_blocked(),
                decided: std::collections::BTreeMap::new(),
            };
            match crate::instruct::ui(ui, chrome, &mut self.instruct, &view) {
                Some(crate::instruct::Act::Send) => self.send_instruction(&ctx),
                Some(crate::instruct::Act::Stop) => self.stop_answer(),
                Some(crate::instruct::Act::Undo) => self.undo_change(),
                Some(crate::instruct::Act::Review(_)) | None => {}
            }
        });
    }

    /// The Agent tab's scope line (card agent-scoped-edits), only while a
    /// selection or a pick narrows it: what the next instruction may
    /// change and which rule gave it, with Whole file.
    fn scope_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let scope = self.scope();
        if scope.lines().is_none() {
            return;
        }
        let mut whole = false;
        // Wrapped: a long name in a plain row would widen the panel.
        ui.horizontal_wrapped(|ui| {
            let r = ui.label(RichText::new(scope.label()).strong()).tip(scope.rule());
            chrome.hit("agent scope", r.rect);
            if scope.lines().is_some() {
                let b = ui
                    .small_button("Whole file")
                    .tip("Drops the selection and the pick: the Agent edits the open file.");
                chrome.hit("scope whole file", b.rect);
                whole = b.clicked();
            }
        });
        if whole {
            self.whole_file(&ui.ctx().clone());
        }
        ui.add_space(4.0);
    }

    fn key_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let held = "Held in this page only; sent only to api.anthropic.com, billed to your account.";
        let key_tip = if self.chat.from_env {
            format!("{held} Preloaded from ANTHROPIC_API_KEY in this machine's environment.")
        } else {
            held.to_string()
        };
        ui.label(RichText::new("Anthropic API key").strong())
            .tip(&key_tip);
        let r = ui
            .add(
                egui::TextEdit::singleline(self.chat.key_mut())
                    .id_salt("pg-key")
                    .password(true)
                    .hint_text("sk-ant-…")
                    .desired_width(f32::INFINITY),
            )
            .tip(&key_tip);
        chrome.hit("agent key", r.rect);
        if r.changed() {
            self.chat.key_changed();
        }
        if self.chat.has_key() && !self.chat.key_looks_right() {
            ui.label(
                RichText::new("That does not look like an Anthropic key (sk-ant-…).")
                    .small()
                    .color(crate::theme::theme(ui).ink(RED)),
            );
        }
        let mut remember = self.chat.remember;
        if ui
            .checkbox(&mut remember, "Remember on this device")
            .tip("Stores the key in this browser's localStorage.")
            .changed()
        {
            self.chat.set_remember(remember);
        }
        if remember {
            ui.label(
                RichText::new(
                    "Stored in this browser's localStorage, readable by any script on this site. \
                     Untick, or Forget, on a shared computer.",
                )
                .small()
                .color(crate::theme::theme(ui).ink(RED)),
            );
        }
        // Wrapped: in a plain row a label extends, and a row wider than a
        // resizable panel widens it every frame.
        ui.horizontal_wrapped(|ui| {
            if self.chat.has_key()
                && ui
                    .button("Forget key")
                    .tip("Clears the key from this page and this browser.")
                    .clicked()
            {
                self.chat.forget_key();
            }
        });
        ui.add_space(6.0);
    }

    /// Draws the trend; returns the rect it filled (the lanes' handles go
    /// over it).
    fn trend_plot(&self, ui: &mut egui::Ui) -> Rect {
        let t = theme(ui);
        let trend = &self.session.trend;
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, t.chart);
        if trend.signals.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Pick signals to trend above",
                FontId::proportional(crate::theme::MIN_TEXT),
                t.muted,
            );
            return rect;
        }
        let n = trend.signals.len();
        #[allow(clippy::cast_precision_loss)]
        let lane = rect.height() / n as f32;
        let end = trend.samples.last_t().unwrap_or(0);
        let window = trend.window_ms.max(1);
        let start = end.saturating_sub(window);
        // Samples older than the window are kept for a wider one: not drawn.
        #[allow(clippy::cast_precision_loss)]
        let x_of = |t: u64| {
            rect.left() + 90.0 + (rect.width() - 90.0 - SCALE_W) * ((t - start) as f32 / window as f32)
        };
        for (k, name) in trend.signals.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let top = rect.top() + lane * k as f32;
            let color = match k % TREND.len() {
                5 if !t.dark => t.fg,
                i => t.ink(TREND[i]),
            };
            let (lo, hi) = trend
                .samples
                .lane(k, start)
                .map(|(_, v)| v)
                .filter(|v| v.is_finite())
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let last = trend.samples.last_value(k);
            painter.text(
                Pos2::new(rect.left() + 6.0, top + lane / 2.0),
                egui::Align2::LEFT_CENTER,
                format!(
                    "{}\n{}",
                    name.to_ascii_lowercase(),
                    last.filter(|v| v.is_finite())
                        .map_or("–".into(), |v| Value::Real(v).to_string())
                ),
                FontId::monospace(crate::theme::MIN_TEXT),
                color,
            );
            painter.line_segment(
                [Pos2::new(rect.left(), top), Pos2::new(rect.right(), top)],
                Stroke::new(1.0, t.rule),
            );
            let (lo, hi, step) = nice_range(lo, hi);
            scale_labels(&painter, &t, rect, top, lane, (lo, hi, step));
            let y_of = |v: f64| {
                #[allow(clippy::cast_possible_truncation)]
                let f = ((v - lo) / (hi - lo)) as f32;
                top + lane - 4.0 - (lane - 8.0) * f.clamp(0.0, 1.0)
            };
            let pts = lane_points(
                trend
                    .samples
                    .lane(k, start)
                    .filter(|(_, v)| v.is_finite())
                    .map(|(t, v)| (x_of(t), y_of(v))),
            );
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
            }
        }
        rect
    }
}

/// One pixel column of a lane's samples: its first, lowest, highest and
/// last, each with its place in the column's order.
struct Column {
    x: f32,
    n: usize,
    first: (usize, f32, f32),
    lo: (usize, f32, f32),
    hi: (usize, f32, f32),
    last: (usize, f32, f32),
}

impl Column {
    fn new(c: f32, x: f32, y: f32) -> Self {
        let e = (0, x, y);
        Self {
            x: c,
            n: 1,
            first: e,
            lo: e,
            hi: e,
            last: e,
        }
    }

    fn add(&mut self, x: f32, y: f32) {
        let e = (self.n, x, y);
        self.n += 1;
        if y < self.lo.2 {
            self.lo = e;
        }
        if y > self.hi.2 {
            self.hi = e;
        }
        self.last = e;
    }

    /// Its kept samples onto `pts`, in time order, each as a step.
    fn flush(&self, pts: &mut Vec<Pos2>) {
        let mut kept = [self.first, self.lo, self.hi, self.last];
        kept.sort_by_key(|e| e.0);
        let mut prev = None;
        for (k, x, y) in kept {
            if prev == Some(k) {
                continue;
            }
            prev = Some(k);
            // Steps, not slopes: a PLC value holds until the next scan.
            if let Some(before) = pts.last().copied() {
                pts.push(Pos2::new(x, before.y));
            }
            pts.push(Pos2::new(x, y));
        }
    }
}

/// The polyline of a lane drawn from its samples (x, y in points, x
/// rising): a step per sample, but each pixel column of them reduced to
/// its first, lowest, highest and last, in time order. A 10 minute
/// window of 10 ms scans is 60 000 samples and draws a few thousand
/// points, the same picture.
pub fn lane_points(samples: impl Iterator<Item = (f32, f32)>) -> Vec<Pos2> {
    let mut pts = Vec::new();
    let mut column: Option<Column> = None;
    for (x, y) in samples {
        match &mut column {
            Some(c) if c.x.to_bits() == x.floor().to_bits() => c.add(x, y),
            _ => {
                if let Some(c) = column.take() {
                    c.flush(&mut pts);
                }
                column = Some(Column::new(x.floor(), x, y));
            }
        }
    }
    if let Some(c) = column {
        c.flush(&mut pts);
    }
    pts
}

/// The narrowest trend window, ms (Nick, 2026-11-02: down to 100 ms).
pub const WINDOW_MIN_MS: u64 = 100;
/// The widest, ms: the box's whole history (`box_trend::WINDOW_MS`).
pub const WINDOW_MAX_MS: u64 = 600_000;

/// A window's width as the slider reads it: `100 ms`, `2.5 s`, `100 s`,
/// whole minutes as `1 min`, `10 min`.
#[must_use]
pub fn window_label(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let secs = ms / 1000;
    if ms < 10_000 && !ms.is_multiple_of(1000) {
        #[allow(clippy::cast_precision_loss)]
        return format!("{:.1} s", ms as f64 / 1000.0);
    }
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// Width of a lane's label column, which is also its drag handle.
const LANE_HANDLE_W: f32 = 88.0;

/// The lane labels of a trend drawn in `rect`, one lane per name, as
/// drag handles (both trends; Nick, 2026-11-07): drag a label up or down
/// and its signal moves to the lane it is dropped on, a bar marking
/// where it will land. The hits are `trend lane <name>`. Returns
/// `(from, to)` when a drag ends on another lane.
pub fn lane_handles(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    rect: Rect,
    names: &[String],
) -> Option<(usize, usize)> {
    let n = names.len();
    if n == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let lane = rect.height() / n as f32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lane_at = |y: f32| (((y - rect.top()) / lane).floor().max(0.0) as usize).min(n - 1);
    let accent = theme(ui).accent;
    let mut moved = None;
    for (k, name) in names.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let top = rect.top() + lane * k as f32;
        let handle = Rect::from_min_size(Pos2::new(rect.left(), top), Vec2::new(LANE_HANDLE_W, lane));
        let r = ui
            .interact(handle, ui.scope_id().with(("trend lane", k)), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab)
            .tip("Drag up or down to move this signal to another lane.");
        chrome.hit(format!("trend lane {}", name.to_ascii_lowercase()), handle);
        let at = ui.ctx().input(|i| i.pointer.latest_pos());
        if r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = at {
                let to = lane_at(p.y);
                // The bar sits on the edge the signal will take: above
                // the target lane moving up, below it moving down.
                #[allow(clippy::cast_precision_loss)]
                let y = rect.top() + lane * (if to > k { to + 1 } else { to }) as f32;
                ui.painter_at(rect).line_segment(
                    [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                    Stroke::new(3.0, accent),
                );
            }
        }
        if r.drag_stopped()
            && let Some(p) = at
        {
            let to = lane_at(p.y);
            if to != k {
                moved = Some((k, to));
            }
        }
    }
    moved
}

/// The window's width as one slider, logarithmic from [`WINDOW_MIN_MS`]
/// to [`WINDOW_MAX_MS`], its value beside it (both trends; Nick,
/// 2026-11-02: a slider instead of buttons, exponential, changing the
/// plot as it moves). The hit is `trend window`. Returns whether it moved.
pub fn window_slider(ui: &mut egui::Ui, chrome: &mut Chrome, window_ms: &mut u64) -> bool {
    let t = theme(ui);
    let before = *window_ms;
    #[allow(clippy::cast_precision_loss)]
    let mut v = (*window_ms).clamp(WINDOW_MIN_MS, WINDOW_MAX_MS) as f64;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Window").color(t.muted))
            .tip("How much time the trend shows.");
        // The value label at the right needs ~52 pt; the slider takes the
        // rest, so the row never widens the panel.
        let label_w = 56.0;
        let w = (ui.available_width() - label_w - 12.0).max(40.0);
        ui.spacing_mut().slider_width = w;
        #[allow(clippy::cast_precision_loss)]
        let r = ui
            .add(
                egui::Slider::new(&mut v, WINDOW_MIN_MS as f64..=WINDOW_MAX_MS as f64)
                    .logarithmic(true)
                    .show_value(false),
            )
            .tip("Drag to show more or less time on the trend.");
        chrome.hit("trend window", r.rect);
        ui.add(
            egui::Label::new(
                RichText::new(window_label(round_window(v)))
                    .monospace()
                    .color(t.muted),
            )
            .truncate(),
        );
    });
    *window_ms = round_window(v);
    *window_ms != before
}

/// A slider position as a window: two significant digits, so the label
/// reads `2.5 s`, `25 s`, `250 s` rather than `2.473 s`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn round_window(v: f64) -> u64 {
    let v = v.clamp(WINDOW_MIN_MS as f64, WINDOW_MAX_MS as f64);
    let mag = 10f64.powf(v.log10().floor() - 1.0);
    ((v / mag).round() * mag)
        .round()
        .clamp(WINDOW_MIN_MS as f64, WINDOW_MAX_MS as f64) as u64
}

/// The room kept at a trend's right edge for [`scale_labels`]: a trace
/// that ran to the edge was drawn under them once the window filled.
pub const SCALE_W: f32 = 64.0;

/// The lane's scale: its top and bottom values at the right edge, so a
/// line can be read (both trends draw them).
pub fn scale_labels(
    painter: &egui::Painter,
    t: &Theme,
    rect: Rect,
    top: f32,
    lane: f32,
    (lo, hi, step): (f64, f64, f64),
) {
    let x = rect.right() - 4.0;
    for (v, y, align) in [
        (hi, top + 3.0, egui::Align2::RIGHT_TOP),
        (lo, top + lane - 3.0, egui::Align2::RIGHT_BOTTOM),
    ] {
        painter.text(
            Pos2::new(x, y),
            align,
            tick_label(v, step),
            FontId::monospace(crate::theme::MIN_TEXT),
            t.muted,
        );
    }
}

impl PlaygroundView {
    /// Each input's control, drawn on its blocks in the diagram: a click
    /// flips a switch, a push button is on while held, a number drags
    /// sideways (or takes a typed value on a click).
    fn map_inputs(&mut self, ui: &mut egui::Ui, area: egui::Rect, reads: &[(String, egui::Rect)]) {
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        ui.set_clip_rect(area);
        // A push button read in two places is held if either is.
        let mut held: Vec<(String, bool)> = Vec::new();
        let mut set = Vec::new();
        for (k, (path, rect)) in reads.iter().enumerate() {
            let Some(p) = inputs.iter().find(|p| p.name.eq_ignore_ascii_case(path)) else {
                continue;
            };
            if !area.intersects(*rect) {
                continue;
            }
            match map_input_ui(ui, *rect, p, k) {
                MapInput::Held(on) => match held.iter_mut().find(|(n, _)| *n == p.name) {
                    Some((_, h)) => *h |= on,
                    None => held.push((p.name.clone(), on)),
                },
                MapInput::Set(v) => set.push((p.name.clone(), v)),
                MapInput::Nothing => {}
            }
        }
        for (name, on) in held {
            self.session.set_input(&name, Value::Bool(on));
        }
        for (name, v) in set {
            self.session.set_input(&name, v);
        }
    }

    /// Import device…: a vendor's EDS joins the project as `NAME.eds`
    /// (replacing one of that name) and the New device form opens filled
    /// in from it; the address is what is left to type.
    fn import_eds(&mut self, file: &str, bytes: &[u8]) {
        if self.proj.is_example() {
            self.proj.note(
                "The examples are read-only: copy the project first (File › Copy project).",
                true,
            );
            return;
        }
        let text = match helmsman_project::check_text(bytes) {
            Ok(t) => t.to_string(),
            Err(e) => {
                self.proj.note(format!("{file} was refused: {e}"), true);
                return;
            }
        };
        let Some(target) = helmsman_project::eds_file_name(file) else {
            self.proj.note(
                format!("{file}: nothing of the name survives as a file name (a-z 0-9 _ -)"),
                true,
            );
            return;
        };
        match self.proj.files.get_mut(&target) {
            Some(old) => old.clone_from(&text),
            None => self.proj.add_file(&target, text.clone()),
        }
        self.proj
            .note(format!("{file} is in {} as {target}.", self.proj.open), false);
        self.devices_ui.adding = Some(crate::devices::NewDevice::from_eds(&target, &text));
    }

    /// The Devices tab's Add: one `device` line onto the project's
    /// `io.map`, made if it has none.
    fn add_device_line(&mut self, line: &str) {
        match self.proj.files.get_mut(helmsman_project::IO_MAP) {
            Some(text) => *text = crate::devices::appended(text, line),
            None => self.proj.add_file(helmsman_project::IO_MAP, line.to_string()),
        }
    }

    /// "Use this device": a read-only example is copied into a project of
    /// the visitor's own first, a project without an `io.map` gets the one
    /// its program's I/O implies (on the box's own channels, which the
    /// rewire question then moves), and the form opens filled in.
    fn use_device(&mut self, d: crate::devices::NewDevice) {
        if self.proj.is_example() {
            let from = self.proj.open.clone();
            let to = (1..=99)
                .map(|n| {
                    if n == 1 {
                        format!("my-{from}")
                    } else {
                        format!("my-{from}-{n}")
                    }
                })
                .map(|n| n.chars().take(48).collect::<String>())
                .find(|n| !self.proj.exists(n))
                .unwrap_or_default();
            self.proj.create(&to, Some(&from));
            self.reload();
            if self.proj.is_example() {
                return;
            }
            self.proj.note(
                format!("{from} is a read-only example, so it was copied as {to} to wire the device in."),
                false,
            );
        }
        if !self.proj.files.contains_key(helmsman_project::IO_MAP) {
            let map = crate::sim_projects::example_io_map(&self.proj.build_src());
            self.proj.add_file(helmsman_project::IO_MAP, map);
        }
        // The copy's program and map, for the form's channels.
        let map = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        self.devices_ui.map = helmsman_project::iomap::skim(&map);
        self.devices_ui.program = crate::device_form::ports_of(&self.proj.build_src());
        self.devices_ui.open(d);
    }

    /// The form's Add: new variables into the program file (refused, with
    /// nothing changed, if the program would not read after), then the
    /// device and its signals into io.map.
    fn apply_wiring(&mut self, w: &crate::device_form::Wiring) {
        if !w.declare.is_empty() {
            let Some(file) = crate::device_form::program_file(
                self.proj.files.iter().map(|(n, t)| (n.as_str(), t.as_str())),
            ) else {
                self.proj.note("No PROGRAM to add the new variables to.", true);
                return;
            };
            let mut src = self.proj.files[&file].clone();
            for (dir, name, ty) in &w.declare {
                let Some(s) = crate::device_form::declare_var(&src, *dir, name, *ty) else {
                    self.proj
                        .note(format!("{file} has no PROGRAM to add {name} to."), true);
                    return;
                };
                src = s;
            }
            if helmsman_iec::ast::parse(&self.proj.files[&file]).is_ok()
                && helmsman_iec::ast::parse(&src).is_err()
            {
                self.proj.note(
                    "A new variable's name is an ST keyword: rename it and Add again.",
                    true,
                );
                return;
            }
            if let Some(t) = self.proj.files.get_mut(&file) {
                *t = src;
            }
        }
        let map = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        let map = crate::device_form::apply_map(&map, w);
        match self.proj.files.get_mut(helmsman_project::IO_MAP) {
            Some(t) => *t = map,
            None => self.proj.add_file(helmsman_project::IO_MAP, map),
        }
        self.proj.note(w.summary(), false);
        self.edited(f64::NEG_INFINITY);
    }

    /// The inputs the diagram shows a control for.
    pub(crate) fn on_map(&self) -> Vec<String> {
        let Some(v) = &self.viewer else { return Vec::new() };
        v.reads_on_screen().into_iter().map(|(path, _)| path).collect()
    }
}

/// What an input's control on the diagram did this frame.
enum MapInput {
    Nothing,
    /// A push button: whether it is held.
    Held(bool),
    Set(Value),
}

/// The "what is this program called?" question: the code waiting to
/// land, the name typed so far, and why the last one was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramName {
    pub text: String,
    pub input: String,
    pub error: Option<String>,
}

/// A name for the Agent's answer: the `PROGRAM`, `FUNCTION_BLOCK` or
/// `FUNCTION` it declares, lower-cased, else `agent`.
#[must_use]
pub fn answer_name(code: &str) -> String {
    let mut words = code.split_whitespace();
    while let Some(w) = words.next() {
        if matches!(
            w.to_ascii_uppercase().as_str(),
            "PROGRAM" | "FUNCTION_BLOCK" | "FUNCTION"
        ) && let Some(name) = words.next()
        {
            let name: String = name
                .trim_end_matches(':')
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(40)
                .collect::<String>()
                .to_ascii_lowercase();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "agent".into()
}

/// One input's control filling its block at `rect`.
fn map_input_ui(ui: &mut egui::Ui, rect: egui::Rect, p: &Point, k: usize) -> MapInput {
    let label = p.name.to_ascii_lowercase();
    let size = (rect.height() * 0.4).clamp(7.0, 18.0);
    let builder = egui::UiBuilder::new().max_rect(rect).id_salt(("pg-map", k));
    let id = egui::Id::unique(("pg-map-number", k));
    ui.scope_builder(builder, |ui| match p.widget {
        Some(Widget::Push) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(RichText::new(&label).strong().size(size).color(if on {
                BLACK
            } else {
                WHITE
            }))
            .fill(if on { GREEN } else { OFF_RED })
            // Never wrapped: a narrow block at low zoom would stack the
            // letters one a row.
            .wrap_mode(egui::TextWrapMode::Truncate)
            .sense(Sense::click_and_drag());
            let r = ui
                .add_sized(rect.size(), b)
                .on_hover_text("push button: on while held");
            MapInput::Held(r.is_pointer_button_down_on())
        }
        Some(Widget::Toggle) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(format!("{label} {}", if on { "TRUE" } else { "FALSE" }))
                    .strong()
                    .size(size)
                    .color(if on { BLACK } else { WHITE }),
            )
            .fill(if on { GREEN } else { OFF_RED })
            .wrap_mode(egui::TextWrapMode::Truncate);
            if ui
                .add_sized(rect.size(), b)
                .on_hover_text("switch: click to flip")
                .clicked()
            {
                MapInput::Set(Value::Bool(!on))
            } else {
                MapInput::Nothing
            }
        }
        Some(Widget::Int { min, max }) => {
            let lo = i64::try_from(min.max(-1_000_000)).unwrap_or(i64::MIN);
            let hi = i64::try_from(max.min(1_000_000)).unwrap_or(i64::MAX);
            let v = match p.value {
                Value::Int(n) => i64::try_from(n).unwrap_or(0),
                _ => 0,
            };
            let c = crate::commit::number(ui, id, &v.to_string(), "", rect.size(), TYPE_HINT);
            match crate::commit::int(&c, lo, hi) {
                Some(n) if n != v => MapInput::Set(Value::Int(i128::from(n))),
                _ => MapInput::Nothing,
            }
        }
        Some(Widget::Real) => {
            let v = match p.value {
                Value::Real(r) => r,
                _ => 0.0,
            };
            let c = crate::commit::number(ui, id, &v.to_string(), "", rect.size(), TYPE_HINT);
            match crate::commit::real(&c) {
                Some(x) if x.to_bits() != v.to_bits() => MapInput::Set(Value::Real(x)),
                _ => MapInput::Nothing,
            }
        }
        Some(Widget::Time) => {
            let ms = match p.value {
                Value::Int(ns) => i64::try_from(ns / 1_000_000).unwrap_or(0),
                _ => 0,
            };
            let c = crate::commit::number(ui, id, &ms.to_string(), "ms", rect.size(), TYPE_HINT);
            match crate::commit::int(&c, 0, 86_400_000) {
                Some(n) if n != ms => MapInput::Set(Value::Int(i128::from(n) * 1_000_000)),
                _ => MapInput::Nothing,
            }
        }
        None => MapInput::Nothing,
    })
    .inner
}

const TYPE_HINT: &str = "type a value, then Enter (Esc puts it back)";

impl Workspace for PlaygroundView {
    fn dirty(&self) -> bool {
        self.proj.dirty()
    }

    fn new_project(&mut self, chrome: &mut Chrome) {
        self.files_ui.new_project();
        chrome.show_files();
    }

    fn copy_project(&mut self, chrome: &mut Chrome) {
        let open = self.proj.open.clone();
        self.files_ui.copy_project(&open);
        chrome.show_files();
    }

    fn layout_menu(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let can = self.proj.can_save_layout();
        if crate::arrange::menu_items(ui, chrome, &mut self.layers, self.viewer.as_mut(), can) {
            self.save_layout();
        }
    }

    fn zoom_to_fit(&mut self) {
        if let Some(v) = &mut self.viewer {
            v.refit();
        }
    }

    fn go_to_definition(&mut self, chrome: &mut Chrome) -> bool {
        let Some(line) = self.viewer.as_ref().and_then(Viewer::definition_line) else {
            return false;
        };
        match self.proj.locate(line) {
            Some((file, l)) => self.goto(&file, l, chrome),
            None => chrome.goto(line),
        }
        chrome.show_code();
        true
    }

    fn cross_references(&mut self) -> bool {
        self.viewer.as_mut().is_some_and(Viewer::cross_references)
    }

    fn force_picked(&mut self, ctx: &egui::Context) -> bool {
        let Some((k, path)) = self.viewer.as_ref().and_then(Viewer::picked_read) else {
            return false;
        };
        let Some(p) = self
            .session
            .inputs()
            .find(|p| p.name.eq_ignore_ascii_case(&path))
            .cloned()
        else {
            return false;
        };
        match (p.widget, p.value) {
            (Some(Widget::Toggle), Value::Bool(on)) => self.session.set_input(&p.name, Value::Bool(!on)),
            // A push button is on only while held.
            (Some(Widget::Push) | None, _) => false,
            _ => {
                ctx.memory_mut(|m| m.request_focus(egui::Id::unique(("pg-map-number", k))));
                true
            }
        }
    }

    fn step_into(&mut self) -> bool {
        let opened = self.viewer.as_mut().is_some_and(Viewer::step_into);
        if opened && !self.session.running {
            self.session.step();
        }
        opened
    }

    fn step_out(&mut self) -> bool {
        let up = self.viewer.as_mut().is_some_and(Viewer::step_out);
        if up && !self.session.running {
            self.session.step();
        }
        up
    }

    fn toggle_breakpoint(&mut self) -> bool {
        let Some(id) = self.viewer.as_ref().and_then(Viewer::picked_stmt) else {
            return false;
        };
        self.session.toggle_break(id);
        true
    }

    fn undo(&mut self, ctx: &egui::Context, redo: bool) -> bool {
        let id = self.source_id();
        let Some(text) = self.proj.current_text_mut() else {
            return false;
        };
        let done = editor::undo(ctx, id, text, redo);
        if done {
            let t = ctx.input(|i| i.time);
            self.edited(t);
        }
        done
    }

    fn writable(&self) -> Result<(), String> {
        Ok(())
    }

    fn save(&mut self) {
        self.proj.save();
    }

    /// Compiles the open project and runs it in the page: the one place
    /// an edit reaches the program running. One that does not compile is
    /// refused, its errors listed, and the program running is unchanged.
    fn deploy(&mut self) {
        // A red requirement test refuses the deploy, naming it.
        if let Some(why) = self.tests_refuse(None) {
            self.notice = Some(why.clone());
            self.log.error(
                self.session.now_ms as f64 / 1000.0,
                format!("Deploy of {} refused. {why}", self.proj.open),
            );
            return;
        }
        let src = self.proj.build_src();
        let carried = self.session.deploy(&src);
        self.pending = None;
        self.sync_viewer();
        if let Some(carried) = carried {
            self.session.set_running(true);
            self.seen.running = Some(true);
            self.seen.scans = self.session.scans;
            self.proj.running = Some(self.proj.open.clone());
            let program = self
                .viewer
                .as_ref()
                .and_then(|v| v.module.program())
                .map_or_else(String::new, |p| format!(": {}", p.name.to_ascii_lowercase()));
            let note = format!(
                "{} is running in this page{program}.{}",
                self.proj.open,
                carried.describe()
            );
            self.running_notice = Some(note.clone());
            self.notice = Some(note);
            self.log.info(
                self.session.now_ms as f64 / 1000.0,
                format!("Deployed {}: running in this page{program}.", self.proj.open),
            );
        } else {
            self.notice = Some(format!(
                "{} does not compile: {} problem(s), listed under the editor; click one to go there.",
                self.proj.open,
                self.session.diags.len()
            ));
            self.log.error(
                self.session.now_ms as f64 / 1000.0,
                format!(
                    "Deploy of {} refused: {} problem(s).",
                    self.proj.open,
                    self.session.diags.len()
                ),
            );
        }
    }

    fn busy(&self) -> bool {
        false
    }

    fn upload(&mut self, file: &str, bytes: Vec<u8>) {
        if crate::devices::is_eds(file) {
            self.import_eds(file, &bytes);
            return;
        }
        let before = (self.proj.open.clone(), self.proj.build_src());
        self.proj.upload(file, &bytes);
        if before.0 != self.proj.open {
            self.reload();
        } else if before.1 != self.proj.build_src() {
            self.session.src = self.proj.build_src();
            self.pending = Some(f64::NEG_INFINITY);
        }
    }

    fn download(&mut self) {
        match self.proj.zip() {
            Ok((name, bytes)) => {
                #[cfg(target_arch = "wasm32")]
                crate::web::download_bytes(&name, &bytes, "application/zip");
                #[cfg(not(target_arch = "wasm32"))]
                self.downloads.push((name, bytes));
            }
            Err(e) => self.proj.notice = Some((format!("Cannot download: {e}"), true)),
        }
    }
}

impl Default for PlaygroundView {
    fn default() -> Self {
        Self::new()
    }
}

/// One input's control; the value it sets, if it changed (a push button
/// reports every frame).
/// How many columns an I/O grid gets in `ui`, and each one's width: two
/// once the panel is wide enough for two controls, else one.
fn columns(ui: &egui::Ui) -> (usize, f32) {
    let avail = ui.available_width();
    let gap = ui.spacing().item_spacing.x;
    if avail >= 2.0 * TAP * 3.0 + gap {
        (2, (avail - gap) / 2.0)
    } else {
        (1, avail)
    }
}

fn input_ui(ui: &mut egui::Ui, p: &Point) -> Option<Value> {
    let t = theme(ui);
    let label = match p.address() {
        Some(a) => format!("{}  {a}", p.name.to_ascii_lowercase()),
        None => p.name.to_ascii_lowercase(),
    };
    let mut out = None;
    // Wrapped, with a slider that fits: a row wider than the panel grows it.
    ui.spacing_mut().slider_width = (ui.available_width() - 110.0).clamp(40.0, 100.0);
    ui.horizontal_wrapped(|ui| match p.widget {
        Some(Widget::Push) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(&label)
                    .strong()
                    .color(if on { BLACK } else { t.fg }),
            )
            .fill(if on { YELLOW } else { t.rule })
            .sense(Sense::click_and_drag())
            .wrap_mode(egui::TextWrapMode::Truncate)
            .min_size(Vec2::new(ui.available_width().min(TAP * 3.0), TAP * 0.8));
            let r = ui.add(b).tip("Push button: on while held.");
            out = Some(Value::Bool(r.is_pointer_button_down_on()));
        }
        Some(Widget::Toggle) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(format!("{label}: {}", if on { "ON" } else { "OFF" }))
                    .strong()
                    .color(if on { BLACK } else { t.fg }),
            )
            .fill(if on { YELLOW } else { t.rule })
            .wrap_mode(egui::TextWrapMode::Truncate)
            .min_size(Vec2::new(ui.available_width().min(TAP * 3.0), TAP * 0.8));
            if ui.add(b).tip("Switch: click to flip.").clicked() {
                out = Some(Value::Bool(!on));
            }
        }
        Some(Widget::Int { min, max }) => {
            ui.label(&label);
            let lo = i64::try_from(min.max(-1_000_000)).unwrap_or(i64::MIN);
            let hi = i64::try_from(max.min(1_000_000)).unwrap_or(i64::MAX);
            let (slo, shi) = (lo.max(-1000), hi.min(1000));
            let mut v = match p.value {
                Value::Int(n) => i64::try_from(n).unwrap_or(0),
                _ => 0,
            };
            let before = v;
            ui.add(
                egui::Slider::new(&mut v, slo..=shi)
                    .clamping(egui::SliderClamping::Edits)
                    .show_value(false),
            )
            .tip(format!(
                "Drag to set the input, {slo} to {shi}. Type any value in the box."
            ));
            if v != before {
                out = Some(Value::Int(i128::from(v)));
            }
            let id = egui::Id::unique(("io-int", &p.name));
            let c = crate::commit::number(
                ui,
                id,
                &before.to_string(),
                "",
                Vec2::new(84.0, TAP * 0.6),
                TYPE_HINT,
            );
            if let Some(n) = crate::commit::int(&c, lo, hi)
                && n != before
            {
                out = Some(Value::Int(i128::from(n)));
            }
        }
        Some(Widget::Real) => {
            ui.label(&label);
            let mut v = match p.value {
                Value::Real(r) => r,
                _ => 0.0,
            };
            let before = v;
            ui.add(
                egui::Slider::new(&mut v, -100.0..=100.0)
                    .clamping(egui::SliderClamping::Edits)
                    .show_value(false),
            )
            .tip("Drag to set the input, -100 to 100. Type any value in the box.");
            if v.to_bits() != before.to_bits() && v.is_finite() {
                out = Some(Value::Real(v));
            }
            let id = egui::Id::unique(("io-real", &p.name));
            let c = crate::commit::number(
                ui,
                id,
                &before.to_string(),
                "",
                Vec2::new(84.0, TAP * 0.6),
                TYPE_HINT,
            );
            if let Some(x) = crate::commit::real(&c)
                && x.to_bits() != before.to_bits()
            {
                out = Some(Value::Real(x));
            }
        }
        Some(Widget::Time) => {
            ui.label(&label);
            let mut ms = match p.value {
                Value::Int(ns) => i64::try_from(ns / 1_000_000).unwrap_or(0),
                _ => 0,
            };
            let before = ms;
            ui.add(
                egui::Slider::new(&mut ms, 0..=10_000)
                    .clamping(egui::SliderClamping::Edits)
                    .show_value(false),
            )
            .tip("Drag to set the time, up to 10 s.");
            ui.add(egui::DragValue::new(&mut ms).range(0..=86_400_000).suffix(" ms"))
                .tip("The time in ms. Drag, or double-click to type it, up to a day.");
            if ms != before {
                out = Some(Value::Int(i128::from(ms) * 1_000_000));
            }
        }
        None => {}
    });
    out
}

/// One output: a lamp (BOOL) or a readout. Returns whether it was clicked.
fn output_ui(ui: &mut egui::Ui, p: &Point) -> bool {
    let t = theme(ui);
    let on = p.value == Value::Bool(true);
    let name = match p.address() {
        Some(a) => format!("{}  {a}", p.name.to_ascii_lowercase()),
        None => p.name.to_ascii_lowercase(),
    };
    let text = if p.ty == Ty::Bool {
        format!("{} {name}", if on { "●" } else { "○" })
    } else {
        format!("{name} = {}", show(p.ty, p.value))
    };
    let b = egui::Button::new(
        RichText::new(text)
            .monospace()
            .size(crate::theme::MIN_TEXT)
            // On is green, as a TRUE value in the diagram and a lamp on
            // the Monitor are (it was the accent yellow here alone).
            .color(if on { t.ink(GREEN) } else { t.fg }),
    )
    .wrap_mode(egui::TextWrapMode::Truncate)
    .min_size(Vec2::new(ui.available_width(), TAP * 0.8));
    ui.add(b).tip("Click to pick its block in the diagram.").clicked()
}

/// A value for a readout (TIME in ms).
#[must_use]
pub fn show(ty: Ty, v: Value) -> String {
    match (ty, v) {
        (Ty::Time, Value::Int(ns)) => format!("{} ms", ns / 1_000_000),
        _ => v.to_string(),
    }
}
